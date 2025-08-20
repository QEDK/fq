use fq;
use tokio;
use anyhow::Error;

#[tokio::main]
async fn main() -> Result<(), Error> {
    const COUNT: usize = 1_000_000;
    let (mut producer, mut consumer) = fq::FastQueue::<usize>::new(COUNT);

    let instant = tokio::time::Instant::now();

    let sender = tokio::spawn(async move {
        for i in 0..COUNT {
            producer.push(i)
                .expect("Unable to send to queue");
        }
        println!("Sent {COUNT} messages in {:?}", instant.elapsed());
    });

    let receiver = tokio::spawn(async move {
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

    tokio::join!(sender, receiver);

    let elapsed = instant.elapsed();
    println!("Completed in {:?}, average message latency: {:?}", elapsed, elapsed / COUNT as u32);

    Ok(())
}
