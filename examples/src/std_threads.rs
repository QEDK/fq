use fq;
use anyhow::Error;

fn main() -> Result<(), Error> {
    const COUNT: usize = 1_000_000;
    let (mut producer, mut consumer) = fq::FastQueue::<usize>::new(COUNT);

    let instant = std::time::Instant::now();

    let sender = std::thread::spawn(move || {
        for i in 0..COUNT {
            producer.push(i)
                .expect("Unable to send to queue");
        }
        println!("Sent {COUNT} messages in {:?}", instant.elapsed());
    });

    let receiver = std::thread::spawn(move || {
        let mut count: usize = 0;
        loop {
            if let Some(value) = consumer.next() {
                std::hint::black_box(value); // prevent the compiler from optimizing away popped value
                count += 1;
                if count == COUNT {
                    println!("Received {COUNT} messages in {:?}", instant.elapsed());
                    break;
                }
            } else {
                std::hint::spin_loop();
            }
        }
    });

    sender.join().unwrap();
    receiver.join().unwrap();

    let elapsed = instant.elapsed();
    println!("Completed in {:?}, average message latency: {:?}", elapsed, elapsed / COUNT as u32);

    Ok(())
}
