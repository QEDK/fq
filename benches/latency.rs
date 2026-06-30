//! Cross-core round-trip latency benchmarks. A persistent echo thread bounces
//! values back over a second queue, so each iteration measures one full round
//! trip (one-way latency is roughly RTT / 2). Threads are spawned outside
//! `b.iter` to keep spawn cost out of the measurement.

use criterion::{Criterion, criterion_group, criterion_main};
use fq::FastQueue;
use std::hint::black_box;
use std::thread;
use std::time::Instant;

const STOP: u64 = u64::MAX;

fn bench_ping_pong(c: &mut Criterion) {
    let mut group = c.benchmark_group("ping_pong");

    group.bench_function("rtt_single", |b| {
        let (mut ping_p, mut ping_c) = FastQueue::<u64>::new(8);
        let (mut pong_p, mut pong_c) = FastQueue::<u64>::new(8);

        let echo = thread::spawn(move || {
            loop {
                if let Some(v) = ping_c.pop() {
                    if v == STOP {
                        break;
                    }
                    while pong_p.push(v).is_err() {
                        std::hint::spin_loop();
                    }
                } else {
                    std::hint::spin_loop();
                }
            }
        });

        b.iter_custom(|iters| {
            let start = Instant::now();
            for i in 0..iters {
                while ping_p.push(black_box(i)).is_err() {
                    std::hint::spin_loop();
                }
                loop {
                    if let Some(v) = pong_c.pop() {
                        black_box(v);
                        break;
                    }
                    std::hint::spin_loop();
                }
            }
            start.elapsed()
        });

        while ping_p.push(STOP).is_err() {
            std::hint::spin_loop();
        }
        echo.join().unwrap();
    });

    group.finish();
}

criterion_group!(latency, bench_ping_pong);
criterion_main!(latency);
