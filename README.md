# FastQueue (fq)
A fast and simple ring-buffer-based single-producer, single-consumer queue with no dependencies. You can use this to write Rust programs with low-latency message passing.

> [!IMPORTANT]
> This crate is highly experimental.

## Installation
Add this to your `Cargo.toml`:
```toml
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
