//! The bounded queue between capture and the pipeline.
//!
//! The queue is the only thing standing between a capture loop that must never
//! wait and a pipeline that decodes and publishes. Two properties are load
//! bearing and are tested here:
//!
//! - it is **bounded**, and when it is full it drops the **oldest** frame, so
//!   the freshest frame is the one that survives;
//! - a slow consumer therefore causes **drops**, not growth: the depth stays at
//!   capacity, which is what bounds latency.
//!
//! The producer path is a push that never waits for the consumer, which is what
//! makes "the capture path never blocks on decode" a property of the shape
//! rather than of a promise.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use camera_core::bounded_queue::{BoundedQueue, PopOutcome, PushOutcome};

#[test]
fn a_new_queue_is_empty() {
    let queue: BoundedQueue<u32> = BoundedQueue::new(4);

    assert_eq!(queue.depth(), 0);
    assert_eq!(queue.stats().capacity(), 4);
    assert_eq!(queue.stats().pushed(), 0);
    assert_eq!(queue.stats().dropped(), 0);
}

#[test]
fn a_full_queue_drops_the_oldest_frame_rather_than_the_newest() {
    let queue = BoundedQueue::new(3);

    for frame in 1..=3 {
        assert_eq!(queue.push(frame), PushOutcome::Accepted);
    }
    assert_eq!(queue.push(4), PushOutcome::DroppedOldest);

    assert_eq!(queue.depth(), 3, "the queue must stay at its capacity");
    assert_eq!(queue.stats().dropped(), 1);
    assert_eq!(queue.pop(), PopOutcome::Frame(2), "frame 1 was the oldest");
    assert_eq!(queue.pop(), PopOutcome::Frame(3));
    assert_eq!(queue.pop(), PopOutcome::Frame(4));
    assert_eq!(queue.pop(), PopOutcome::Empty);
}

#[test]
fn the_newest_frame_survives_a_long_run_of_pushes() {
    let queue = BoundedQueue::new(4);

    for frame in 0..100 {
        queue.push(frame);
    }

    assert_eq!(queue.depth(), 4);
    assert_eq!(queue.stats().pushed(), 100);
    assert_eq!(queue.stats().dropped(), 96);
    let surviving = (0..4).filter_map(|_| queue.try_pop()).collect::<Vec<_>>();
    assert_eq!(surviving, vec![96, 97, 98, 99]);
}

#[test]
fn popping_an_empty_queue_reports_that_it_is_empty() {
    let queue: BoundedQueue<u32> = BoundedQueue::new(1);

    assert_eq!(queue.pop(), PopOutcome::Empty);
    assert_eq!(queue.try_pop(), None);
}

#[test]
fn a_pop_waits_for_a_frame_until_the_deadline() {
    let queue: BoundedQueue<u32> = BoundedQueue::new(1);

    let started = std::time::Instant::now();
    assert_eq!(
        queue.pop_timeout(Duration::from_millis(50)),
        PopOutcome::TimedOut
    );
    assert!(started.elapsed() >= Duration::from_millis(40));
}

#[test]
fn closing_the_queue_wakes_a_waiting_consumer() {
    let queue: Arc<BoundedQueue<u32>> = Arc::new(BoundedQueue::new(1));
    let consumer = Arc::clone(&queue);

    let waiting = thread::spawn(move || consumer.pop_timeout(Duration::from_secs(30)));
    thread::sleep(Duration::from_millis(50));
    queue.close();

    assert_eq!(
        waiting.join().expect("the consumer thread must not panic"),
        PopOutcome::Closed
    );
    assert!(queue.is_closed());
}

#[test]
fn a_slow_consumer_causes_drops_while_the_depth_stays_at_capacity() {
    const CAPACITY: usize = 4;
    const FRAMES: usize = 200;

    let queue: Arc<BoundedQueue<usize>> = Arc::new(BoundedQueue::new(CAPACITY));
    let producer_queue = Arc::clone(&queue);
    let producer = thread::spawn(move || {
        for frame in 0..FRAMES {
            producer_queue.push(frame);
            // A capture loop runs flat out; the point is that it never waits.
            thread::sleep(Duration::from_micros(200));
        }
        producer_queue.close();
    });

    let observed_max_depth = AtomicUsize::new(0);
    let mut delivered = Vec::new();
    loop {
        match queue.pop_timeout(Duration::from_millis(200)) {
            PopOutcome::Frame(frame) => {
                delivered.push(frame);
                observed_max_depth.fetch_max(queue.depth(), Ordering::Relaxed);
                // The slow consumer: it takes far longer per frame than the
                // producer needs to make one.
                thread::sleep(Duration::from_millis(2));
            }
            PopOutcome::TimedOut | PopOutcome::Closed => break,
            PopOutcome::Empty => {}
        }
    }
    producer.join().expect("the producer thread must not panic");

    let stats = queue.stats();
    assert!(stats.dropped() > 0, "a slow consumer must cause drops");
    assert!(
        observed_max_depth.load(Ordering::Relaxed) <= CAPACITY,
        "the depth must never exceed the capacity, so latency cannot grow"
    );
    assert!(
        delivered.len() < FRAMES,
        "some frames must have been dropped rather than queued"
    );
    assert_eq!(
        delivered.last().copied(),
        Some(FRAMES - 1),
        "the newest frame must survive: drop-oldest, never drop-newest"
    );
    assert_eq!(
        stats.pushed(),
        u64::try_from(FRAMES).expect("fits"),
        "every push is counted, including the dropped ones"
    );
    assert_eq!(
        stats.pushed(),
        u64::try_from(delivered.len()).expect("fits") + stats.dropped(),
        "every frame is either delivered or dropped, so nothing is lost silently"
    );
}

#[test]
fn the_producer_never_waits_for_the_consumer() {
    // With no consumer at all and a full queue, 10_000 pushes must still
    // complete promptly: the producer path never blocks on a downstream stage.
    const FRAMES: usize = 10_000;
    let queue = BoundedQueue::new(2);
    let done = AtomicBool::new(false);

    let started = std::time::Instant::now();
    for frame in 0..FRAMES {
        queue.push(frame);
    }
    done.store(true, Ordering::Release);

    assert!(done.load(Ordering::Acquire));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "pushing into a full queue must not wait, took {:?}",
        started.elapsed()
    );
    assert_eq!(queue.depth(), 2);
    assert_eq!(
        queue.stats().dropped(),
        u64::try_from(FRAMES - 2).expect("fits")
    );
}
