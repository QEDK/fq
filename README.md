# FastQueue (fq)
A fast and simple ring-buffer-based single-producer, single-consumer queue with no dependencies. You can use
this to write Rust programs with low-latency message passing. 

> [!IMPORTANT]
> This crate is highly experimental.

## Installation
Add this to your `Cargo.toml`:
```TOML
[dependencies]
fq = "0.0.2"
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

## Examples
See the [examples](examples) directory for more usage examples.

## How does it work?
The ring buffer structure allows for a contigous data structure. The idea is that if we are able to get extreme
cache locality, we can improve performance by reducing cache misses. This is also the reason why if you use
smart pointers like `Box<T>`, performance *may* degrade since cache locality gets affected. For very large
`T` types, you are more limited by `memcpy()` performance and less from queue implementations. As such,
ring buffers can be considered strongly optimized for data of a few word sizes with some non-linear
degradation for larger sizes. Additional optimizations are provided for CPUs that support `sse` and `prfchw`
flags. As and when Rust `std` provides more relevant instructions, they will be added. This is simply a
high-level explanation of some of the techniques employed by this crate, you can read the code to gain a better
understanding of what's happening under the hood.

## Principles
* This crate will always prioritize message throughput over memory usage.
* This crate will always support generic types only.
* This crate will always provide a wait-free **and** lock-free API.
* This crate will use unsafe Rust where possible for maximal throughput.

## Benchmarks
Benchmarks are strictly difficult due to the nature of the program, it's somewhat simple to do a same-CPU
bench but performance will still be affected based on the core type and cache contention. Benchmarks are
provided in the [benchmark](benchmark) directory and can be run with `cargo bench`. Contributions via PRs for
additional benchmarks are welcome.

## License
Licensed under either of:
 * MIT license ([LICENSE-MIT](LICENSE-MIT))
 * Lesser General Public license v3.0 or later ([LICENSE-LGPL](LICENSE-LGPL))
at your option.

### Contribution
Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the LGPL-3.0 license, shall be dual licensed as above, without any
additional terms or conditions.

## Acknowledgements
Inspired from previous works like [fastqueue2](https://github.com/andersc/fastqueue2), [rigtorp](https://rigtorp.se/ringbuffer) and [rtrb](https://github.com/mgeier/rtrb).
