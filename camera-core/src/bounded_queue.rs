//! The **bounded queue** between capture and the pipeline.
//!
//! Capture must never wait for decoding or for output. That is a property of the
//! shape of this queue rather than a promise made by the capture loop: a
//! producer only ever calls [`BoundedQueue::push`], which takes the lock for as
//! long as it takes to append and evict, and then returns. It never waits for a
//! consumer, so a stalled pipeline can only cost frames, never time.
//!
//! When the queue is full it drops the **oldest** frame. Dropping the newest
//! would deliver stale pictures while hiding the fact that the pipeline is
//! behind; dropping the oldest keeps the freshest frame and bounds the latency at
//! `capacity * frame period`.
//!
//! Counters are kept here so every dropped frame is observable rather than
//! inferred: `pushed == delivered + dropped` always holds.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// What a push did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// The frame was appended and the queue was not full.
    Accepted,
    /// The queue was full, so its oldest frame was discarded and the new frame
    /// appended.
    DroppedOldest,
}

impl PushOutcome {
    /// Whether this push discarded a frame.
    #[must_use]
    pub fn dropped(self) -> bool {
        matches!(self, Self::DroppedOldest)
    }
}

/// What a pop found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PopOutcome<T> {
    /// A frame.
    Frame(T),
    /// Nothing was available, and the queue is still open.
    Empty,
    /// The deadline passed with nothing available.
    TimedOut,
    /// The queue was closed and drained.
    Closed,
}

/// Counters describing what the queue has done, so drops are observable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueStats {
    capacity: usize,
    depth: usize,
    pushed: u64,
    dropped: u64,
}

impl QueueStats {
    /// The bound on how many frames the queue may hold.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many frames are waiting right now.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Every frame ever pushed, including the ones evicted at the back.
    #[must_use]
    pub fn pushed(&self) -> u64 {
        self.pushed
    }

    /// Every frame evicted because it was the oldest while the queue was full.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Every frame a consumer took, which is `pushed - dropped - depth`.
    #[must_use]
    pub fn popped(&self) -> u64 {
        self.pushed
            .saturating_sub(self.dropped)
            .saturating_sub(u64::try_from(self.depth).unwrap_or(u64::MAX))
    }
}

/// A fixed-capacity queue that drops the oldest frame instead of growing.
///
/// Cheap to share: every method takes `&self`, so a producer thread and a
/// consumer thread need only an `Arc` around it.
#[derive(Debug)]
pub struct BoundedQueue<T> {
    inner: Mutex<QueueInner<T>>,
    available: Condvar,
    capacity: usize,
}

#[derive(Debug)]
struct QueueInner<T> {
    frames: VecDeque<T>,
    pushed: u64,
    dropped: u64,
    closed: bool,
}

impl<T> BoundedQueue<T> {
    /// Creates a queue holding at most `capacity` frames.
    ///
    /// A capacity of zero is treated as one: a queue that can hold nothing could
    /// never deliver a frame, and silently dropping every frame is not a
    /// configuration anybody means.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            inner: Mutex::new(QueueInner {
                frames: VecDeque::with_capacity(capacity),
                pushed: 0,
                dropped: 0,
                closed: false,
            }),
            available: Condvar::new(),
            capacity,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueInner<T>> {
        // A poisoned lock means a consumer panicked while holding it. The queue
        // still holds a consistent `VecDeque` and the counters are plain data,
        // so recovering is honest here and keeps a capture thread alive.
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Appends a frame, evicting the oldest if the queue is full. Never waits.
    pub fn push(&self, frame: T) -> PushOutcome {
        let mut inner = self.lock();
        let outcome = if inner.frames.len() >= self.capacity {
            inner.frames.pop_front();
            inner.dropped += 1;
            PushOutcome::DroppedOldest
        } else {
            PushOutcome::Accepted
        };
        inner.frames.push_back(frame);
        inner.pushed += 1;
        drop(inner);
        self.available.notify_one();
        outcome
    }

    /// Takes the oldest frame if one is waiting.
    pub fn try_pop(&self) -> Option<T> {
        self.lock().frames.pop_front()
    }

    /// Takes the oldest frame, reporting rather than blocking when there is none.
    pub fn pop(&self) -> PopOutcome<T> {
        match self.try_pop() {
            Some(frame) => PopOutcome::Frame(frame),
            None => PopOutcome::Empty,
        }
    }

    /// Waits up to `timeout` for a frame.
    ///
    /// Returns [`PopOutcome::TimedOut`] if the deadline passed with nothing
    /// available, and [`PopOutcome::Closed`] as soon as the queue is closed and
    /// drained, which is how a consumer notices that capture has stopped.
    pub fn pop_timeout(&self, timeout: Duration) -> PopOutcome<T> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.lock();

        loop {
            if let Some(frame) = inner.frames.pop_front() {
                return PopOutcome::Frame(frame);
            }
            if inner.closed {
                return PopOutcome::Closed;
            }

            let now = Instant::now();
            if now >= deadline {
                return PopOutcome::TimedOut;
            }
            let remaining = deadline.saturating_duration_since(now);
            inner = match self.available.wait_timeout(inner, remaining) {
                Ok((inner, _)) => inner,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }

    /// Closes the queue, waking any waiting consumer.
    ///
    /// Frames already queued stay available: a consumer drains them before it
    /// sees [`PopOutcome::Closed`].
    pub fn close(&self) {
        self.lock().closed = true;
        self.available.notify_all();
    }

    /// Whether [`BoundedQueue::close`] has been called.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// How many frames are waiting.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.lock().frames.len()
    }

    /// The bound on how many frames the queue may hold.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// A consistent snapshot of the counters.
    #[must_use]
    pub fn stats(&self) -> QueueStats {
        let inner = self.lock();
        QueueStats {
            capacity: self.capacity,
            depth: inner.frames.len(),
            pushed: inner.pushed,
            dropped: inner.dropped,
        }
    }
}

impl<T> Default for BoundedQueue<T> {
    fn default() -> Self {
        Self::new(1)
    }
}

/// The default depth of the capture queue: four frames, about a sixth of a
/// second at the frame rate an M50 produces.
///
/// Small on purpose. Every extra slot is latency a consumer application sees,
/// and a pipeline that needs more than four frames of slack is behind rather
/// than busy.
pub const DEFAULT_QUEUE_CAPACITY: usize = 4;
