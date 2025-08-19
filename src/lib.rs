use std::alloc::{Layout, alloc, dealloc};
use std::cell::UnsafeCell;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Padding to prevent false sharing
#[repr(C, align(64))]
struct CachePadded<T>(T);

pub struct FastQueue<T> {
    /// Capacity mask (capacity - 1) for fast modulo
    mask: CachePadded<usize>,

    /// The actual capacity
    capacity: CachePadded<usize>,

    /// Buffer storing elements directly
    buffer: CachePadded<*mut MaybeUninit<T>>,

    /// Producer cache line - head and cached tail together
    producer: CachePadded<ProducerCache>,

    /// Consumer cache line - tail and cached head together  
    consumer: CachePadded<ConsumerCache>,

    _phantom: PhantomData<T>,
}

struct ProducerCache {
    /// Write position
    head: AtomicUsize,
    /// Cached read position to avoid loading tail
    cached_tail: UnsafeCell<usize>,
}

struct ConsumerCache {
    /// Read position
    tail: AtomicUsize,
    /// Cached write position to avoid loading head
    cached_head: UnsafeCell<usize>,
}

unsafe impl<T: Send> Send for FastQueue<T> {}
unsafe impl<T: Send> Sync for FastQueue<T> {}

impl<T> FastQueue<T> {
    /// Capacity will be rounded up to the next power of two
    pub fn new(capacity: usize) -> (Producer<T>, Consumer<T>) {
        let capacity = capacity.next_power_of_two();
        let mask = capacity - 1;

        let layout = Layout::array::<MaybeUninit<T>>(capacity).expect("Layout calculation failed");
        let buffer = unsafe { alloc(layout) as *mut MaybeUninit<T> };

        if buffer.is_null() {
            panic!("Failed to allocate buffer");
        }

        let queue = Arc::new(FastQueue {
            mask: CachePadded(mask),
            capacity: CachePadded(capacity),
            buffer: CachePadded(buffer),
            producer: CachePadded(ProducerCache {
                head: AtomicUsize::new(0),
                cached_tail: UnsafeCell::new(0),
            }),
            consumer: CachePadded(ConsumerCache {
                tail: AtomicUsize::new(0),
                cached_head: UnsafeCell::new(0),
            }),
            _phantom: PhantomData,
        });

        let producer = Producer {
            queue: Arc::clone(&queue),
        };

        let consumer = Consumer { queue };

        (producer, consumer)
    }
}

impl<T> Drop for FastQueue<T> {
    fn drop(&mut self) {
        let head = self.producer.0.head.load(Ordering::Relaxed);
        let mut tail = self.consumer.0.tail.load(Ordering::Relaxed);

        while tail != head {
            unsafe {
                let index = tail & self.mask.0;
                let slot = self.buffer.0.add(index);
                ptr::drop_in_place((*slot).as_mut_ptr());
            }
            tail = tail.wrapping_add(1);
        }

        unsafe {
            let layout = Layout::array::<MaybeUninit<T>>(self.capacity.0)
                .expect("Layout calculation failed");
            dealloc(self.buffer.0 as *mut u8, layout);
        }
    }
}

pub struct Producer<T> {
    queue: Arc<FastQueue<T>>,
}

unsafe impl<T: Send> Send for Producer<T> {}

impl<T> Producer<T> {
    /// Returns Ok(()) on success, Err(value) if queue is full
    #[inline(always)]
    pub fn push(&mut self, value: T) -> Result<(), T> {
        let head = self.queue.producer.0.head.load(Ordering::Relaxed);
        let next_head = head.wrapping_add(1);

        #[cfg(any(
            target_arch = "x86",
            all(target_arch = "x86_64", target_feature = "sse")
        ))]
        unsafe {
            let next_index = next_head & self.queue.mask.0;
            let next_slot = self.queue.buffer.0.add(next_index);
            prefetch_write(next_slot as *const u8);
        }

        let cached_tail = unsafe { *self.queue.producer.0.cached_tail.get() };

        if next_head.wrapping_sub(cached_tail) > self.queue.capacity.0 {
            // Reload actual tail (slow path)
            let tail = self.queue.consumer.0.tail.load(Ordering::Acquire);
            unsafe {
                *self.queue.producer.0.cached_tail.get() = tail;
            }

            // Check again with fresh tail
            if next_head.wrapping_sub(tail) > self.queue.capacity.0 {
                return Err(value);
            }
        }

        unsafe {
            let index = head & self.queue.mask.0;
            let slot = self.queue.buffer.0.add(index);
            (*slot).as_mut_ptr().write(value);
        }

        self.queue
            .producer
            .0
            .head
            .store(next_head, Ordering::Release);

        Ok(())
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        let head = self.queue.producer.0.head.load(Ordering::Relaxed);
        let tail = self.queue.consumer.0.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline(always)]
    pub fn is_full(&self) -> bool {
        self.len() >= self.queue.capacity.0
    }
}

pub struct Consumer<T> {
    queue: Arc<FastQueue<T>>,
}

unsafe impl<T: Send> Send for Consumer<T> {}

impl<T> Consumer<T> {
    /// Returns None if queue is empty or Some(T)
    #[inline(always)]
    pub fn pop(&mut self) -> Option<T> {
        let tail = self.queue.consumer.0.tail.load(Ordering::Relaxed);

        #[cfg(any(
            target_arch = "x86",
            all(target_arch = "x86_64", target_feature = "sse")
        ))]
        unsafe {
            let next_index = (tail + 1) & self.queue.mask.0;
            let next_slot = self.queue.buffer.0.add(next_index);
            prefetch_read(next_slot as *const u8);
        }

        // Check cached head first (fast path)
        let cached_head = unsafe { *self.queue.consumer.0.cached_head.get() };

        if tail == cached_head {
            // Reload actual head (slow path)
            let head = self.queue.producer.0.head.load(Ordering::Acquire);
            unsafe {
                *self.queue.consumer.0.cached_head.get() = head;
            }

            // Check if still empty
            if tail == head {
                return None;
            }
        }

        let value = unsafe {
            let index = tail & self.queue.mask.0;
            let slot = self.queue.buffer.0.add(index);
            (*slot).as_ptr().read()
        };

        let next_tail = tail.wrapping_add(1);
        self.queue
            .consumer
            .0
            .tail
            .store(next_tail, Ordering::Release);

        Some(value)
    }

    /// Peek at the front element without removing it
    #[inline(always)]
    pub fn peek(&self) -> Option<&T> {
        let tail = self.queue.consumer.0.tail.load(Ordering::Relaxed);
        let head = self.queue.producer.0.head.load(Ordering::Acquire);

        if tail == head {
            return None;
        }

        unsafe {
            let index = tail & self.queue.mask.0;
            let slot = self.queue.buffer.0.add(index);
            #[cfg(any(
                target_arch = "x86",
                all(target_arch = "x86_64", target_feature = "sse")
            ))]
            {
                prefetch_read(slot as *const u8);
            }
            Some(&*(*slot).as_ptr())
        }
    }

    /// Get the current size of the queue (may be stale)
    #[inline(always)]
    pub fn len(&self) -> usize {
        let head = self.queue.producer.0.head.load(Ordering::Relaxed);
        let tail = self.queue.consumer.0.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }

    /// Check if the queue is empty (may be stale)
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(any(
    target_arch = "x86",
    all(target_arch = "x86_64", target_feature = "sse")
))]
#[inline(always)]
fn prefetch_read(p: *const u8) {
    unsafe {
        #[cfg(target_arch = "x86")]
        use std::arch::x86::_mm_prefetch;
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::_mm_prefetch;

        const _MM_HINT_T0: i32 = 3; // Prefetch to all cache levels as read
        _mm_prefetch(p as *const i8, _MM_HINT_T0);
    }
}

#[cfg(any(
    target_arch = "x86",
    all(target_arch = "x86_64", target_feature = "sse")
))]
#[inline(always)]
fn prefetch_write(p: *const u8) {
    unsafe {
        #[cfg(target_arch = "x86")]
        use std::arch::x86::_mm_prefetch;
        #[cfg(target_arch = "x86_64")]
        use std::arch::x86_64::_mm_prefetch;

        const _MM_HINT_ET0: i32 = 7; // Prefetch to all cache levels as write
        _mm_prefetch(p as *const i8, _MM_HINT_ET0);
    }
}

impl<T> Iterator for Consumer<T> {
    type Item = T;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        self.pop()
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
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
