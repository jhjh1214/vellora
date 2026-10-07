//! The tile queue and the deadline watchdog (M0 task 18).
//!
//! [`Queue`] holds the tiles the UI has asked for and hands them to the single render worker in
//! priority order: [`Priority::Visible`] before [`Priority::Prefetch`] before
//! [`Priority::Thumbnail`], first in first out within one priority. A cancelled request that is
//! still queued is dropped; one already rendering is allowed to finish (PDFium cannot be
//! interrupted), but [`Queue::finish`] tells the worker to discard the result, so a cancelled
//! request never gets an answer.
//!
//! [`Watchdog`] times the job being rendered. Crossing the soft deadline is logged; crossing the
//! hard deadline calls a hook once. The hook is where the process-level kill (M0 task 19) is
//! attached: a stuck PDFium call cannot be stopped from inside the process.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use vellora_ipc::{Priority, RequestId};

/// Soft and hard time allowed for one tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deadlines {
    /// A tile still rendering after this is logged as slow.
    pub soft: Duration,
    /// A tile still rendering after this is reported to the hard-deadline hook.
    pub hard: Duration,
}

impl Default for Deadlines {
    fn default() -> Self {
        Self {
            soft: Duration::from_secs(2),
            hard: Duration::from_secs(10),
        }
    }
}

/// What [`Queue::cancel`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cancelled {
    /// It was waiting and has been removed.
    Queued,
    /// It is rendering now; its result will be discarded.
    Running,
    /// Nothing by that id (never sent, already answered or already cancelled).
    Unknown,
}

struct QueueState<T> {
    /// One FIFO per priority, indexed by [`lane`].
    lanes: [VecDeque<(RequestId, T)>; 3],
    running: Option<RequestId>,
    running_cancelled: bool,
    closed: bool,
}

/// Locks a mutex, carrying on after a panic elsewhere: the state is plain data that every
/// operation leaves consistent, and the engine must keep serving after a handler panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const fn lane(priority: Priority) -> usize {
    match priority {
        Priority::Visible => 0,
        Priority::Prefetch => 1,
        Priority::Thumbnail => 2,
    }
}

/// Priority queue between the reader (which pushes and cancels) and the worker (which takes).
pub(crate) struct Queue<T> {
    state: Mutex<QueueState<T>>,
    ready: Condvar,
}

impl<T> Queue<T> {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(QueueState {
                lanes: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
                running: None,
                running_cancelled: false,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    /// Queues `job`. Ignored once the queue is closed.
    pub(crate) fn push(&self, priority: Priority, req_id: RequestId, job: T) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        state.lanes[lane(priority)].push_back((req_id, job));
        drop(state);
        self.ready.notify_one();
    }

    /// Drops queued work for `req_id`, or marks the running job so its result is discarded.
    pub(crate) fn cancel(&self, req_id: RequestId) -> Cancelled {
        let mut state = lock(&self.state);
        let mut removed = false;
        for lane in &mut state.lanes {
            let before = lane.len();
            lane.retain(|(id, _)| *id != req_id);
            removed |= lane.len() != before;
        }
        if removed {
            Cancelled::Queued
        } else if state.running == Some(req_id) {
            state.running_cancelled = true;
            Cancelled::Running
        } else {
            Cancelled::Unknown
        }
    }

    /// Blocks until a job is available and marks it running. `None` once the queue is closed;
    /// work still queued then is dropped.
    pub(crate) fn next(&self) -> Option<(RequestId, T)> {
        let mut state = lock(&self.state);
        loop {
            if state.closed {
                return None;
            }
            if let Some(job) = state.lanes.iter_mut().find_map(VecDeque::pop_front) {
                state.running = Some(job.0);
                state.running_cancelled = false;
                return Some(job);
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Marks the running job done. `true` if its result should be sent, `false` if it was
    /// cancelled meanwhile.
    pub(crate) fn finish(&self) -> bool {
        let mut state = lock(&self.state);
        state.running = None;
        !std::mem::take(&mut state.running_cancelled)
    }

    /// Stops the worker after its current job and drops what is queued.
    pub(crate) fn close(&self) {
        let mut state = lock(&self.state);
        state.closed = true;
        for lane in &mut state.lanes {
            lane.clear();
        }
        drop(state);
        self.ready.notify_all();
    }
}

struct Running {
    req_id: RequestId,
    since: Instant,
    soft_logged: bool,
    hard_fired: bool,
}

struct WatchState {
    running: Option<Running>,
    closed: bool,
}

/// Times the job being rendered against [`Deadlines`].
pub(crate) struct Watchdog {
    state: Mutex<WatchState>,
    changed: Condvar,
}

impl Watchdog {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(WatchState {
                running: None,
                closed: false,
            }),
            changed: Condvar::new(),
        }
    }

    /// The worker starts rendering `req_id`.
    pub(crate) fn begin(&self, req_id: RequestId) {
        lock(&self.state).running = Some(Running {
            req_id,
            since: Instant::now(),
            soft_logged: false,
            hard_fired: false,
        });
        self.changed.notify_all();
    }

    /// The worker is done with its job.
    pub(crate) fn end(&self) {
        lock(&self.state).running = None;
        self.changed.notify_all();
    }

    /// Ends [`run`](Self::run).
    pub(crate) fn close(&self) {
        lock(&self.state).closed = true;
        self.changed.notify_all();
    }

    /// The watchdog thread's body: returns after [`close`](Self::close). Each deadline fires at
    /// most once per job; `on_hard` runs without the lock held.
    pub(crate) fn run(&self, deadlines: Deadlines, on_hard: &(dyn Fn(RequestId) + Sync)) {
        let mut state = lock(&self.state);
        loop {
            if state.closed {
                return;
            }
            let mut fire = None;
            let mut wait = None;
            if let Some(job) = &mut state.running {
                let elapsed = job.since.elapsed();
                if !job.soft_logged {
                    if elapsed >= deadlines.soft {
                        job.soft_logged = true;
                        tracing::warn!(
                            req_id = job.req_id.0,
                            ?elapsed,
                            "a tile is taking longer than the soft deadline"
                        );
                    } else {
                        wait = Some(deadlines.soft.saturating_sub(elapsed));
                    }
                }
                if !job.hard_fired {
                    if elapsed >= deadlines.hard {
                        job.hard_fired = true;
                        fire = Some(job.req_id);
                    } else {
                        let left = deadlines.hard.saturating_sub(elapsed);
                        wait = Some(wait.map_or(left, |w: Duration| w.min(left)));
                    }
                }
            }
            if let Some(req_id) = fire {
                drop(state);
                on_hard(req_id);
                state = lock(&self.state);
                continue;
            }
            state = match wait {
                Some(timeout) => {
                    self.changed
                        .wait_timeout(state, timeout)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                }
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use super::*;

    fn id(n: u64) -> RequestId {
        RequestId(n)
    }

    fn drain(queue: &Queue<&'static str>) -> Vec<u64> {
        let mut order = Vec::new();
        for _ in 0..10 {
            let Some((req, _)) = queue.next() else { break };
            order.push(req.0);
            queue.finish();
            if lock(&queue.state).lanes.iter().all(VecDeque::is_empty) {
                break;
            }
        }
        order
    }

    #[test]
    fn visible_before_prefetch_before_thumbnail_and_fifo_within_a_priority() {
        let queue = Queue::new();
        // Queued in the worst order for the policy.
        queue.push(Priority::Thumbnail, id(1), "t1");
        queue.push(Priority::Prefetch, id(2), "p1");
        queue.push(Priority::Thumbnail, id(3), "t2");
        queue.push(Priority::Visible, id(4), "v1");
        queue.push(Priority::Prefetch, id(5), "p2");
        queue.push(Priority::Visible, id(6), "v2");
        assert_eq!(drain(&queue), [4, 6, 2, 5, 1, 3]);
    }

    #[test]
    fn priority_holds_under_load_when_work_keeps_arriving() {
        let queue = Queue::new();
        for n in 0..1000 {
            queue.push(Priority::Thumbnail, id(n), "thumb");
        }
        // The worker takes one; visible work arriving afterwards still jumps the 999 others.
        let (first, _) = queue.next().unwrap();
        assert_eq!(first, id(0));
        queue.finish();
        queue.push(Priority::Prefetch, id(5000), "prefetch");
        queue.push(Priority::Visible, id(6000), "visible");
        assert_eq!(queue.next().unwrap().0, id(6000));
        queue.finish();
        assert_eq!(queue.next().unwrap().0, id(5000));
        queue.finish();
        assert_eq!(queue.next().unwrap().0, id(1));
    }

    #[test]
    fn a_queued_request_that_is_cancelled_is_never_handed_out() {
        let queue = Queue::new();
        queue.push(Priority::Visible, id(1), "a");
        queue.push(Priority::Visible, id(2), "b");
        queue.push(Priority::Prefetch, id(3), "c");
        assert_eq!(queue.cancel(id(1)), Cancelled::Queued);
        assert_eq!(queue.cancel(id(3)), Cancelled::Queued);
        assert_eq!(queue.cancel(id(1)), Cancelled::Unknown);
        assert_eq!(queue.cancel(id(99)), Cancelled::Unknown);
        assert_eq!(drain(&queue), [2]);
    }

    #[test]
    fn a_running_request_that_is_cancelled_has_its_result_discarded() {
        let queue = Queue::new();
        queue.push(Priority::Visible, id(1), "a");
        queue.push(Priority::Visible, id(2), "b");
        assert_eq!(queue.next().unwrap().0, id(1));
        assert_eq!(queue.cancel(id(1)), Cancelled::Running);
        assert!(!queue.finish(), "cancelled work must not be answered");
        // The next job is not affected by the earlier cancel.
        assert_eq!(queue.next().unwrap().0, id(2));
        assert!(queue.finish());
        // And after it finished, cancelling it finds nothing.
        assert_eq!(queue.cancel(id(2)), Cancelled::Unknown);
    }

    #[test]
    fn close_wakes_a_waiting_worker_and_drops_queued_work() {
        let queue = Arc::new(Queue::<&'static str>::new());
        let worker = {
            let queue = Arc::clone(&queue);
            thread::spawn(move || queue.next())
        };
        thread::sleep(Duration::from_millis(50));
        queue.close();
        assert!(worker.join().unwrap().is_none());

        let queue = Queue::new();
        queue.push(Priority::Visible, id(1), "a");
        queue.close();
        assert!(queue.next().is_none());
        queue.push(Priority::Visible, id(2), "b");
        assert!(queue.next().is_none(), "nothing is accepted after close");
    }

    #[test]
    fn a_waiting_worker_is_woken_by_a_push() {
        let queue = Arc::new(Queue::<&'static str>::new());
        let worker = {
            let queue = Arc::clone(&queue);
            thread::spawn(move || queue.next().map(|(req, _)| req))
        };
        thread::sleep(Duration::from_millis(50));
        queue.push(Priority::Prefetch, id(7), "x");
        assert_eq!(worker.join().unwrap(), Some(id(7)));
    }

    fn watched(deadlines: Deadlines, job: impl FnOnce(&Watchdog)) -> (usize, Vec<RequestId>) {
        let watchdog = Arc::new(Watchdog::new());
        let fired = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let thread = {
            let (watchdog, fired, count) = (
                Arc::clone(&watchdog),
                Arc::clone(&fired),
                Arc::clone(&count),
            );
            thread::spawn(move || {
                watchdog.run(deadlines, &|req| {
                    count.fetch_add(1, Ordering::SeqCst);
                    lock(&fired).push(req);
                });
            })
        };
        job(&watchdog);
        watchdog.close();
        thread.join().unwrap();
        let fired = lock(&fired).clone();
        (count.load(Ordering::SeqCst), fired)
    }

    #[test]
    fn the_hard_deadline_fires_once_for_a_job_that_overruns() {
        let deadlines = Deadlines {
            soft: Duration::from_millis(10),
            hard: Duration::from_millis(40),
        };
        let (count, fired) = watched(deadlines, |watchdog| {
            watchdog.begin(id(5));
            thread::sleep(Duration::from_millis(300));
            watchdog.end();
        });
        assert_eq!((count, fired), (1, vec![id(5)]));
    }

    #[test]
    fn a_job_inside_its_deadline_never_fires_and_each_job_is_timed_afresh() {
        let deadlines = Deadlines {
            soft: Duration::from_secs(30),
            hard: Duration::from_secs(60),
        };
        let (count, _) = watched(deadlines, |watchdog| {
            for n in 0..20 {
                watchdog.begin(id(n));
                watchdog.end();
            }
        });
        assert_eq!(count, 0);

        // A slow job followed by a quick one: only the slow one is reported.
        let deadlines = Deadlines {
            soft: Duration::from_millis(5),
            hard: Duration::from_millis(30),
        };
        let (count, fired) = watched(deadlines, |watchdog| {
            watchdog.begin(id(1));
            thread::sleep(Duration::from_millis(250));
            watchdog.end();
            watchdog.begin(id(2));
            watchdog.end();
        });
        assert_eq!((count, fired), (1, vec![id(1)]));
    }
}
