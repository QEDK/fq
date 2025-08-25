/*!
A fast and simple ring-buffer-based single-producer, single-consumer queue with no dependencies. You can use this to write Rust programs with low-latency message passing.

## Installation
Add this to your `Cargo.toml`:
```TOML
[dependencies]
fq = "0.0.4"
```

## Quickstart
```rust
use fq::FastQueue;
use std::thread;

let (mut producer, mut consumer) = FastQueue::<String>::new(2);

let sender = thread::spawn(move || {
    producer.push("Hello, thread".to_owned())
        .expect("Unable to send to queue");
});

let receiver = thread::spawn(move || {
    while let Some(value) = consumer.next() {
        assert_eq!(value, "Hello, thread");
    }
});

sender.join().expect("The sender thread has panicked");
receiver.join().expect("The receiver thread has panicked");
```

## How does it work?
The ring buffer structure allows for a contiguous data structure. The idea is that if we are able to get extreme
cache locality, we can improve performance by reducing cache misses. This is also the reason why if you use
smart pointers like `Box<T>`, performance *may* degrade since cache locality gets degraded. For very large
`T` types, you are more limited by `memcpy()` performance and less from queue implementations. As such,
ring buffers can be considered strongly optimized for data of a few word sizes with some non-linear performance
degradation for larger sizes. Additional optimizations are provided for CPUs that support `sse` and `prfchw`
instructions. As and when Rust `std` provides more relevant instructions, they will be added. This is simply a
high-level explanation of some of the techniques employed by this crate, you can read the code to gain a better
understanding of what's happening under the hood.

## Profiles
The crate is fully synchronous and runtime-agnostic. We are heavily reliant on `std` for memory management, so
it's unlikely that we will support `#[no_std]` runtimes anytime soon. You should be using the `release` or
`maxperf` profiles for optimal performance.

## Principles
* This crate will always prioritize message throughput over memory usage.
* This crate will always support generic types.
* This crate will always provide a wait-free **and** lock-free API.
* This crate will use unsafe Rust where possible for maximal throughput.

## CPU Features
On `x86` and `x86_64` targets, prefetch instructions are available on the `stable` toolchain. To make use of prefetch instructions on the `aarch64` target, you should enable the `unstable` feature and use the `nightly`
toolchain.
```TOML
[dependencies]
fq = { version = "0.0.4", features = ["unstable"] }
```

## Benchmarks
Benchmarks are strictly difficult due to the nature of the program, it's somewhat simple to do a same-CPU
bench but performance will still be affected based on the core type and cache contention. Benchmarks are
provided in the [benches](benches/bench.rs) directory and can be run with `cargo bench`. Contributions via
PRs for additional benchmarks are welcome.
*/
#![cfg_attr(nightly, feature(stdarch_aarch64_prefetch))]
use core::alloc::Layout;
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::alloc::{alloc, dealloc, handle_alloc_error};
use std::sync::Arc;

/// Padding to prevent false sharing
#[cfg_attr(
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "arm64ec",
        target_arch = "powerpc64",
    ),
    repr(C, align(128))
)]
#[cfg_attr(
    any(
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "sparc",
        target_arch = "hexagon",
    ),
    repr(C, align(32))
)]
#[cfg_attr(
    not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "arm64ec",
        target_arch = "powerpc64",
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "sparc",
        target_arch = "hexagon",
    )),
    repr(C, align(64))
)]
struct CachePadded<T>(T);

#[cfg(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "arm64ec",
    target_arch = "powerpc64",
))]
const CACHE_LINE_SIZE: usize = 128;

#[cfg(any(
    target_arch = "arm",
    target_arch = "mips",
    target_arch = "mips32r6",
    target_arch = "mips64",
    target_arch = "mips64r6",
    target_arch = "sparc",
    target_arch = "hexagon",
))]
const CACHE_LINE_SIZE: usize = 32;

#[cfg(not(any(
    target_arch = "x86_64",
    target_arch = "aarch64",
    target_arch = "arm64ec",
    target_arch = "powerpc64",
    target_arch = "arm",
    target_arch = "mips",
    target_arch = "mips32r6",
    target_arch = "mips64",
    target_arch = "mips64r6",
    target_arch = "sparc",
    target_arch = "hexagon",
)))]
const CACHE_LINE_SIZE: usize = 64;

/// A fast lock-free single-producer, single-consumer queue
pub struct FastQueue<T> {
    /// Capacity mask (capacity - 1) for fast modulo
    mask: CachePadded<usize>,

    /// The actual capacity
    capacity: CachePadded<usize>,

    /// Buffer storing elements directly
    buffer: CachePadded<*mut MaybeUninit<T>>,

    /// Written by producer, read by consumer.
    head: CachePadded<AtomicUsize>,

    /// Written by consumer, read by producer.
    tail: CachePadded<AtomicUsize>,

    _pd: PhantomData<T>,
}

unsafe impl<T: Send> Send for FastQueue<T> {}
unsafe impl<T: Send> Sync for FastQueue<T> {}

impl<T> FastQueue<T> {
    /// Creates a SPSC queue with the given capacity. The allocated capacity may be higher.
    ///
    /// Capacity is rounded to the next power of two. The minimum allocated capacity is 2.
    ///
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// struct Message {
    ///     from: String,
    ///     value: usize,
    /// }
    /// let (producer, consumer) = FastQueue::<Message>::new(2);
    /// ```
    pub fn new(capacity: usize) -> (Producer<T>, Consumer<T>) {
        let capacity = capacity.next_power_of_two().max(2);
        let mask = capacity - 1;

        let layout =
            Layout::from_size_align(capacity * size_of::<MaybeUninit<T>>(), CACHE_LINE_SIZE)
                .expect("layout");
        let buffer = unsafe { alloc(layout) as *mut MaybeUninit<T> };

        if buffer.is_null() {
            handle_alloc_error(layout);
        }

        let queue = Arc::new(FastQueue {
            mask: CachePadded(mask),
            capacity: CachePadded(capacity),
            buffer: CachePadded(buffer),
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
            _pd: PhantomData,
        });

        let producer = Producer {
            queue: CachePadded(Arc::clone(&queue)),
            head: CachePadded(UnsafeCell::new(0)),
            cached_tail: CachePadded(UnsafeCell::new(0)),
            _pd: PhantomData,
        };

        let consumer = Consumer {
            queue: CachePadded(queue),
            tail: CachePadded(UnsafeCell::new(0)),
            cached_head: CachePadded(UnsafeCell::new(0)),
            _pd: PhantomData,
        };

        (producer, consumer)
    }
}

impl<T> Drop for FastQueue<T> {
    /// Drops all elements in the queue. This will only drop the elements, not the queue itself.
    fn drop(&mut self) {
        let head = self.head.0.load(Ordering::Relaxed);
        let mut tail = self.tail.0.load(Ordering::Relaxed);

        while tail != head {
            unsafe {
                let index = tail & self.mask.0;
                let slot = self.buffer.0.add(index);
                ptr::drop_in_place((*slot).as_mut_ptr());
            }
            tail = tail.wrapping_add(1);
        }

        unsafe {
            let layout = Layout::from_size_align(
                self.capacity.0 * size_of::<MaybeUninit<T>>(),
                CACHE_LINE_SIZE,
            )
            .expect("layout");
            dealloc(self.buffer.0 as *mut u8, layout);
        }
    }
}

/// A producer for the `FastQueue`. This is used to send elements to the queue.
pub struct Producer<T> {
    queue: CachePadded<Arc<FastQueue<T>>>,
    head: CachePadded<UnsafeCell<usize>>,
    cached_tail: CachePadded<UnsafeCell<usize>>,
    _pd: PhantomData<T>,
}

unsafe impl<T: Send> Send for Producer<T> {}

/// A producer for the `FastQueue`. This is used to send elements to the queue.
impl<T> Producer<T> {
    /// Pushes a value into the queue. Returns `Ok(())` on success or `Err(T)` if the queue is full.
    ///
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::new(2);
    /// producer.push(42).unwrap();
    /// assert_eq!(consumer.pop(), Some(42));
    /// ```
    #[inline(always)]
    pub fn push(&mut self, value: T) -> Result<(), T> {
        let head = unsafe { *self.head.0.get() };
        let next_head = head.wrapping_add(1);

        self.prefetch_write(next_head);

        let cached_tail = unsafe { *self.cached_tail.0.get() };

        if next_head.wrapping_sub(cached_tail) > self.queue.0.capacity.0 {
            // Reload actual tail (slow path)
            let tail = self.queue.0.tail.0.load(Ordering::Acquire);

            if tail != cached_tail {
                // Update cached tail
                unsafe {
                    *self.cached_tail.0.get() = tail;
                }
            }

            // Check again with fresh tail
            if next_head.wrapping_sub(tail) > self.queue.0.capacity.0 {
                return Err(value);
            }
        }

        unsafe {
            let index = head & self.queue.0.mask.0;
            let slot = self.queue.0.buffer.0.add(index);
            (*slot).as_mut_ptr().write(value);
            *self.head.0.get() = next_head;
        }

        self.queue.0.head.0.store(next_head, Ordering::Release);

        Ok(())
    }

    /// Returns the current number of elements in the queue (may be stale)
    ///
    /// This function may return stale data when holding a lock on the queue.
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::new(2);
    /// assert_eq!(consumer.len(), 0);
    /// producer.push(42).unwrap();
    /// assert_eq!(consumer.len(), 1);
    /// ```
    #[inline(always)]
    pub fn len(&self) -> usize {
        let head = unsafe { *self.head.0.get() };
        let tail = self.queue.0.tail.0.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }

    /// Checks if the queue is empty (may be stale). This function will return `true` if the queue is empty, and `false` otherwise.
    ///
    /// This function will return stale data when holding a lock on the queue.
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::new(2);
    /// assert!(consumer.is_empty());
    /// producer.push(42).unwrap();
    /// assert!(!consumer.is_empty());
    /// ```
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Checks if the queue is full (may be stale). This function will return `true` if the queue is full, and `false` otherwise.
    ///
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::<usize>::new(2);
    /// producer.push(42).unwrap(); // ⚠️ Prefer handling the error over using unwrap()
    /// assert_eq!(producer.is_full(), false);
    /// producer.push(43).unwrap();
    /// assert_eq!(producer.is_full(), true);
    /// ```
    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.len() >= self.queue.0.capacity.0
    }

    #[inline(always)]
    fn prefetch_write(&self, index: usize) {
        let slot_index = index & self.queue.0.mask.0;
        let _slot = unsafe { self.queue.0.buffer.0.add(slot_index) };

        #[cfg(all(target_arch = "x86_64", target_feature = "sse"))]
        unsafe {
            core::arch::x86_64::_mm_prefetch(_slot as *const i8, core::arch::x86_64::_MM_HINT_T0);
        }

        #[cfg(all(target_arch = "x86_64", target_feature = "prfchw"))]
        unsafe {
            core::arch::x86_64::_mm_prefetch(_slot as *const i8, core::arch::x86_64::_MM_HINT_ET0);
        }

        #[cfg(all(target_arch = "x86"))]
        unsafe {
            core::arch::x86::_mm_prefetch(_slot as *const i8, core::arch::x86::_MM_HINT_ET0);
        }

        #[cfg(all(feature = "unstable", nightly, target_arch = "aarch64"))]
        unsafe {
            core::arch::aarch64::_prefetch::<
                { core::arch::aarch64::_PREFETCH_WRITE },
                { core::arch::aarch64::_PREFETCH_LOCALITY0 },
            >(_slot as *const i8);
        }
    }
}

/// A consumer for the `FastQueue`. This is used to receive items from the queue.
pub struct Consumer<T> {
    queue: CachePadded<Arc<FastQueue<T>>>,
    tail: CachePadded<UnsafeCell<usize>>,
    cached_head: CachePadded<UnsafeCell<usize>>,
    _pd: PhantomData<T>,
}

unsafe impl<T: Send> Send for Consumer<T> {}

/// A consumer for the `FastQueue`. This is used to receive items from the queue.
impl<T> Consumer<T> {
    /// Pops a value from the queue. Returns `Some(T)` on success or `None` if the queue is empty.
    ///
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::new(2);
    /// producer.push(42).unwrap();
    /// assert_eq!(consumer.pop(), Some(42));
    /// ```
    #[inline(always)]
    pub fn pop(&mut self) -> Option<T> {
        let tail = unsafe { *self.tail.0.get() };

        self.prefetch_read(tail.wrapping_add(1));

        // Check cached head first (fast path)
        let cached_head = unsafe { *self.cached_head.0.get() };

        if tail == cached_head {
            // Reload actual head (slow path)
            let head = self.queue.0.head.0.load(Ordering::Acquire);

            if head != cached_head {
                // Update cached head
                unsafe {
                    *self.cached_head.0.get() = head;
                }
            }

            // Check if still empty
            if tail == head {
                return None;
            }
        }

        let value = unsafe {
            let index = tail & self.queue.0.mask.0;
            let slot = self.queue.0.buffer.0.add(index);
            (*slot).as_ptr().read()
        };

        let next_tail = tail.wrapping_add(1);
        unsafe { *self.tail.0.get() = next_tail };
        self.queue.0.tail.0.store(next_tail, Ordering::Release);

        Some(value)
    }

    /// Peeks at the front element without removing it.
    ///
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::new(2);
    /// producer.push(42).unwrap();
    /// assert_eq!(consumer.peek(), Some(&42));
    /// ```
    #[inline(always)]
    pub fn peek(&self) -> Option<&T> {
        let tail = unsafe { *self.tail.0.get() };
        let head = self.queue.0.head.0.load(Ordering::Acquire);

        if tail == head {
            return None;
        }

        unsafe {
            let index = tail & self.queue.0.mask.0;
            let slot = self.queue.0.buffer.0.add(index);
            Some(&*(*slot).as_ptr())
        }
    }

    /// Returns the current size of the queue (may be stale).
    ///
    /// This function will return stale data when holding a lock on the queue.
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::new(2);
    /// assert_eq!(consumer.len(), 0);
    /// producer.push(42).unwrap();
    /// assert_eq!(consumer.len(), 1);
    /// ```
    #[inline(always)]
    pub fn len(&self) -> usize {
        let head = self.queue.0.head.0.load(Ordering::Relaxed);
        let tail = unsafe { *self.tail.0.get() };
        head.wrapping_sub(tail)
    }

    /// Checks if the queue is empty (may be stale). Returns `true` if the queue is empty, and `false` otherwise.
    ///
    /// This function will return stale data when holding a lock on the queue.
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::new(2);
    /// assert_eq!(consumer.is_empty(), true);
    /// producer.push(42).unwrap();
    /// assert_eq!(consumer.is_empty(), false);
    /// ```
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline(always)]
    fn prefetch_read(&self, index: usize) {
        let slot_index = index & self.queue.0.mask.0;
        let _slot = unsafe { self.queue.0.buffer.0.add(slot_index) };

        #[cfg(any(all(target_arch = "x86_64", target_feature = "sse")))]
        unsafe {
            core::arch::x86_64::_mm_prefetch(_slot as *const i8, core::arch::x86_64::_MM_HINT_T0);
        }

        #[cfg(all(nightly, target_arch = "x86"))]
        unsafe {
            core::arch::x86::_mm_prefetch(_slot as *const i8, core::arch::x86::_MM_HINT_T0);
        }

        #[cfg(all(feature = "unstable", nightly, target_arch = "aarch64"))]
        unsafe {
            core::arch::aarch64::_prefetch::<
                { core::arch::aarch64::_PREFETCH_READ },
                { core::arch::aarch64::_PREFETCH_LOCALITY0 },
            >(_slot as *const i8);
        }
    }
}

impl<T> Iterator for Consumer<T> {
    type Item = T;

    /// Pops the next value from the queue. This is equivalent to calling `pop()`.
    ///
    /// # Example
    /// ```
    /// use fq::FastQueue;
    /// let (mut producer, mut consumer) = FastQueue::new(4);
    /// producer.push(42).unwrap();
    /// producer.push(42).unwrap();
    /// producer.push(42).unwrap();
    /// while let Some(value) = consumer.next() {
    ///     assert_eq!(value, 42);
    /// }
    /// ```
    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        self.pop()
    }

    /// Provides a size hint (may be stale)
    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        // (lower bound, upper bound)
        (self.len(), None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    #[test]
    fn test_basic_push_pop() {
        let (mut producer, mut consumer) = FastQueue::<usize>::new(2);

        assert!(producer.push(42).is_ok());
        assert_eq!(consumer.pop(), Some(42));
        assert_eq!(consumer.pop(), None);
    }

    #[test]
    fn test_capacity() {
        let (mut producer, mut consumer) = FastQueue::<usize>::new(4);

        assert!(producer.push(1).is_ok());
        assert!(producer.push(2).is_ok());
        assert!(producer.push(3).is_ok());
        assert!(producer.push(4).is_ok());
        assert!(producer.push(5).is_err()); // Full

        assert_eq!(consumer.pop(), Some(1));
        assert!(producer.push(5).is_ok()); // Space available now
        assert_eq!(consumer.pop(), Some(2));
        assert_eq!(consumer.pop(), Some(3));
        assert_eq!(consumer.pop(), Some(4));
        assert_eq!(consumer.pop(), Some(5));
    }

    #[test]
    fn test_concurrent() {
        const COUNT: usize = 1_000_000;
        let (mut producer, mut consumer) = FastQueue::<usize>::new(1024);

        let done = Arc::new(AtomicBool::new(false));
        let done_clone = Arc::clone(&done);

        // Producer thread
        let producer_thread = thread::spawn(move || {
            for i in 0..COUNT {
                while producer.push(i).is_err() {
                    std::hint::spin_loop();
                }
            }
            done_clone.store(true, Ordering::Release);
        });

        // Consumer thread
        let consumer_thread = thread::spawn(move || {
            let mut count = 0;
            while count < COUNT {
                if let Some(val) = consumer.pop() {
                    assert_eq!(val, count);
                    count += 1;
                } else if done.load(Ordering::Acquire) && consumer.is_empty() {
                    break;
                } else {
                    std::hint::spin_loop();
                }
            }
            assert_eq!(count, COUNT);
        });

        producer_thread.join().unwrap();
        consumer_thread.join().unwrap();
    }

    #[test]
    fn test_wraparound() {
        let (mut producer, mut consumer) = FastQueue::<usize>::new(4);

        // Fill queue
        for i in 0..4 {
            assert!(producer.push(i).is_ok());
        }

        // Consume half
        for i in 0..2 {
            assert_eq!(consumer.pop(), Some(i));
        }

        // Fill again (wraps around)
        for i in 4..6 {
            assert!(producer.push(i).is_ok());
        }

        // Consume all
        for i in 2..6 {
            assert_eq!(consumer.pop(), Some(i));
        }

        assert!(consumer.pop().is_none());
    }
}
