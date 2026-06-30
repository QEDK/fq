//! Tail-latency histogram for cross-core round trips.
//!
//! Records every cross-core round trip individually and prints tail-latency
//! percentiles. Note that `Instant` is quantized to ~41.7 ns ticks on Apple
//! Silicon, so individual samples are coarse but the percentiles hold up.
//!
//! Run with: cargo run --release --bin latency-hist

use fq::FastQueue;
use std::hint::black_box;
use std::thread;
use std::time::Instant;

const STOP: u64 = u64::MAX;
const WARMUP: usize = 50_000;
const SAMPLES: usize = 1_000_000;

fn main() {
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

    let mut round_trip = |i: u64| {
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
    };

    for i in 0..WARMUP {
        round_trip(i as u64);
    }

    let mut samples_ns: Vec<u64> = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let start = Instant::now();
        round_trip(i as u64);
        samples_ns.push(start.elapsed().as_nanos() as u64);
    }

    while ping_p.push(STOP).is_err() {
        std::hint::spin_loop();
    }
    echo.join().expect("echo thread panicked");

    samples_ns.sort_unstable();
    let pct =
        |p: f64| samples_ns[((samples_ns.len() as f64 * p) as usize).min(samples_ns.len() - 1)];

    println!("round-trip latency over {SAMPLES} samples (ns):");
    println!("  p50:   {}", pct(0.50));
    println!("  p90:   {}", pct(0.90));
    println!("  p99:   {}", pct(0.99));
    println!("  p99.9: {}", pct(0.999));
    println!("  max:   {}", samples_ns[samples_ns.len() - 1]);
    println!("(one-way queue latency ~ RTT / 2)");
}
