# FastQueue (fq)
A fast and simple ring-buffer-based single-producer, single-consumer queue with no dependencies. You can use this to write Rust programs with low-latency message passing.

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

## License

Licensed under either of:
 * MIT license ([LICENSE-MIT](LICENSE-MIT))
 * Lesser General Public license v3.0 or later ([LICENSE-LGPL](LICENSE-LGPL))
at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the LGPL-3.0 license, shall be dual licensed as above, without any
additional terms or conditions.
