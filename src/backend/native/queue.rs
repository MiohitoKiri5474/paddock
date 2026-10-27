//! An unbounded queue to a thread of its own that takes its items, which
//! knows how many bytes wait in it: the native server's frames to its
//! client and bytes to a program go through one each, so queuing never
//! waits, and whoever queues decides how much is too much.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

pub struct Queue<T> {
    tx: Sender<T>,
    size: fn(&T) -> usize,
    /// Bytes queued and not yet taken.
    waiting: Arc<AtomicUsize>,
    /// The thread has ended: nothing queued is taken any more.
    ended: Arc<AtomicBool>,
}

impl<T: Send + 'static> Queue<T> {
    /// Start a thread passing each item (`size` bytes of it) to `take`,
    /// until `take` says no more (false) or the queue is dropped.
    pub fn start(
        size: fn(&T) -> usize,
        mut take: impl FnMut(T) -> bool + Send + 'static,
    ) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<T>();
        let waiting = Arc::new(AtomicUsize::new(0));
        let ended = Arc::new(AtomicBool::new(false));
        let (left, done) = (Arc::clone(&waiting), Arc::clone(&ended));
        let thread = std::thread::spawn(move || {
            for item in rx {
                let n = size(&item);
                let more = take(item);
                left.fetch_sub(n, Ordering::Relaxed);
                if !more {
                    break;
                }
            }
            done.store(true, Ordering::Relaxed);
        });
        let queue = Self {
            tx,
            size,
            waiting,
            ended,
        };
        (queue, thread)
    }

    /// Bytes queued and not yet taken.
    pub fn waiting(&self) -> usize {
        self.waiting.load(Ordering::Relaxed)
    }

    /// Whether the thread has ended, so nothing queued is taken.
    pub fn ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    /// Queue `item`: the bytes then waiting, or `None` if the thread has
    /// ended.
    pub fn push(&self, item: T) -> Option<usize> {
        if self.ended() {
            return None;
        }
        let n = (self.size)(&item);
        // Counted before it is sent, so the thread never takes off more
        // than was added.
        let waiting = self.waiting.fetch_add(n, Ordering::Relaxed) + n;
        if self.tx.send(item).is_err() {
            self.waiting.fetch_sub(n, Ordering::Relaxed);
            return None;
        }
        Some(waiting)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_what_waits_and_stops_once_its_thread_ends() {
        let (gate_tx, gate) = mpsc::channel::<()>();
        let (queue, thread) = Queue::start(Vec::len, move |item: Vec<u8>| {
            let _ = gate.recv();
            item.len() < 3
        });
        assert_eq!(queue.push(vec![0; 2]), Some(2));
        assert_eq!(queue.push(vec![0; 5]), Some(7));
        gate_tx.send(()).unwrap();
        gate_tx.send(()).unwrap();
        thread.join().unwrap();
        assert!(queue.ended());
        assert_eq!(queue.waiting(), 0);
        assert_eq!(queue.push(vec![0; 1]), None, "nothing takes it");
    }
}
