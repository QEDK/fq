use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use fq::FastQueue;
use std::thread;

fn bench_single_threaded_push_pop(c: &mut Criterion) {
    let mut group = c.benchmark_group("single_threaded_push_pop");

    for size in [16, 64, 256, 1024, 4096].iter() {
        group.throughput(Throughput::Elements(*size as u64));

        group.bench_with_input(BenchmarkId::new("push_pop", size), size, |b, &size| {
            b.iter(|| {
                let (mut producer, mut consumer) = FastQueue::<u64>::new(size);

                for i in 0..size {
                    producer.push(black_box(i as u64)).unwrap();
                }

                for _ in 0..size {
                    black_box(consumer.pop().unwrap());
                }
            });
        });
    }
    group.finish();
}

fn bench_push_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("push_only");

    for capacity in [64, 256, 1024, 4096, 16384].iter() {
        group.throughput(Throughput::Elements(*capacity as u64));

        group.bench_with_input(
            BenchmarkId::new("push", capacity),
            capacity,
            |b, &capacity| {
                b.iter(|| {
                    let (mut producer, _consumer) = FastQueue::<u64>::new(capacity);

                    for i in 0..capacity {
                        producer.push(black_box(i as u64)).unwrap();
                    }
                });
            },
        );
    }
    group.finish();
}

fn bench_pop_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("pop_only");

    for capacity in [64, 256, 1024, 4096, 16384].iter() {
        group.throughput(Throughput::Elements(*capacity as u64));

        group.bench_with_input(
            BenchmarkId::new("pop", capacity),
            capacity,
            |b, &capacity| {
                b.iter_batched(
                    || {
                        let (mut producer, consumer) = FastQueue::<u64>::new(capacity);
                        for i in 0..capacity {
                            producer.push(i as u64).unwrap();
                        }
                        consumer
                    },
                    |mut consumer| {
                        for _ in 0..capacity {
                            black_box(consumer.pop().unwrap());
                        }
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_concurrent_spsc(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_spsc");

    for messages in [1000, 10000, 100000].iter() {
        group.throughput(Throughput::Elements(*messages as u64));

        group.bench_with_input(
            BenchmarkId::new("producer_consumer", messages),
            messages,
            |b, &messages| {
                b.iter(|| {
                    let (mut producer, mut consumer) = FastQueue::<u64>::new(1024);

                    let producer_handle = thread::spawn(move || {
                        for i in 0..messages {
                            while producer.push(black_box(i as u64)).is_err() {
                                std::hint::spin_loop();
                            }
                        }
                    });

                    let consumer_handle = thread::spawn(move || {
                        let mut count = 0;
                        while count < messages {
                            if let Some(val) = consumer.pop() {
                                black_box(val);
                                count += 1;
                            } else {
                                std::hint::spin_loop();
                            }
                        }
                    });

                    producer_handle.join().unwrap();
                    consumer_handle.join().unwrap();
                });
            },
        );
    }
    group.finish();
}

fn bench_concurrent_spsc_large_messages(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_spsc_large_messages");

    struct LargeMessage {
        val1: u128,
        val2: String,
    }

    for messages in [1000, 10000, 100000].iter() {
        group.throughput(Throughput::Elements(*messages as u64));

        group.bench_with_input(
            BenchmarkId::new("producer_consumer", messages),
            messages,
            |b, &messages| {
                b.iter(|| {
                    let (mut producer, mut consumer) = FastQueue::<LargeMessage>::new(1024);

                    let producer_handle = thread::spawn(move || {
                        for i in 0..messages {
                            while producer
                                .push(black_box(LargeMessage {
                                    val1: i as u128,
                                    val2: format!("Message {i}"),
                                }))
                                .is_err()
                            {
                                std::hint::spin_loop();
                            }
                        }
                    });

                    let consumer_handle = thread::spawn(move || {
                        let mut count = 0;
                        while count < messages {
                            if let Some(val) = consumer.pop() {
                                black_box(val);
                                count += 1;
                            } else {
                                std::hint::spin_loop();
                            }
                        }
                    });

                    producer_handle.join().unwrap();
                    consumer_handle.join().unwrap();
                });
            },
        );
    }
    group.finish();
}

fn bench_burst_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("burst_operations");

    group.bench_function("burst_push_pop_16", |b| {
        b.iter(|| {
            let (mut producer, mut consumer) = FastQueue::<u64>::new(32);

            // Burst push 16 items
            for i in 0..16 {
                producer.push(black_box(i)).unwrap();
            }

            // Burst pop 16 items
            for _ in 0..16 {
                black_box(consumer.pop().unwrap());
            }
        });
    });

    group.bench_function("alternating_push_pop", |b| {
        b.iter(|| {
            let (mut producer, mut consumer) = FastQueue::<u64>::new(16);

            for i in 0..1000 {
                producer.push(black_box(i)).unwrap();
                black_box(consumer.pop().unwrap());
            }
        });
    });

    group.finish();
}

fn bench_wraparound(c: &mut Criterion) {
    let mut group = c.benchmark_group("wraparound");

    group.bench_function("wraparound_operations", |b| {
        b.iter_batched(
            || {
                let (mut producer, mut consumer) = FastQueue::<u64>::new(64);
                // Fill queue to near capacity
                for i in 0..60 {
                    producer.push(i).unwrap();
                }
                // Consume some to create space for wraparound
                for _ in 0..30 {
                    consumer.pop().unwrap();
                }
                (producer, consumer)
            },
            |(mut producer, mut consumer)| {
                // This will cause wraparound in the ring buffer
                for i in 60..120 {
                    while producer.push(black_box(i)).is_err() {
                        black_box(consumer.pop());
                    }
                }

                // Consume remaining
                while let Some(val) = consumer.pop() {
                    black_box(val);
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_capacity_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("capacity_scaling");

    for capacity in [16, 32, 64, 128, 256, 512, 1024, 2048, 4096].iter() {
        group.bench_with_input(
            BenchmarkId::new("creation_overhead", capacity),
            capacity,
            |b, &capacity| {
                b.iter(|| {
                    let (_producer, _consumer) = FastQueue::<u64>::new(black_box(capacity));
                });
            },
        );
    }

    group.finish();
}

fn bench_peek_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("peek_operations");

    group.bench_function("peek_vs_pop", |b| {
        b.iter_batched(
            || {
                let (mut producer, consumer) = FastQueue::<u64>::new(1024);
                for i in 0..500 {
                    producer.push(i).unwrap();
                }
                consumer
            },
            |consumer| {
                // Peek at all elements multiple times
                for _ in 0..100 {
                    if let Some(val) = consumer.peek() {
                        black_box(val);
                    }
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

fn bench_len_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("len_operations");

    group.bench_function("len_check_overhead", |b| {
        b.iter_batched(
            || {
                let (mut producer, consumer) = FastQueue::<u64>::new(1024);
                for i in 0..500 {
                    producer.push(i).unwrap();
                }
                (producer, consumer)
            },
            |(producer, consumer)| {
                for _ in 0..1000 {
                    black_box(producer.len());
                    black_box(consumer.len());
                    black_box(producer.is_empty());
                    black_box(producer.is_full());
                    black_box(consumer.is_empty());
                }
            },
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_single_threaded_push_pop,
    bench_push_only,
    bench_pop_only,
    bench_concurrent_spsc,
    bench_concurrent_spsc_large_messages,
    bench_burst_operations,
    bench_wraparound,
    bench_capacity_scaling,
    bench_peek_operations,
    bench_len_operations
);
criterion_main!(benches);
