//! The write gate: one process-wide lock that serializes every store write.
//!
//! Oxigraph transactions on RocksDB are not isolated from each other and do
//! no conflict detection: two concurrent commits are both written and the
//! last one wins silently. The gate is the only isolation between writers.
//!
//! It is a `Mutex` + `Condvar` binary lock rather than a tokio mutex, so it
//! needs no runtime and cannot panic when called on an async worker. A waiter
//! blocks its own thread, so store work should still run on a blocking thread
//! (`spawn_blocking`); the server does that.
//!
//! The guard lives on the thread that runs the store work. Dropping a request
//! future therefore never releases the gate while a write is still running.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::application::limits::ToolError;

/// Who holds the gate, for `write_busy` errors.
#[derive(Debug, Clone)]
pub struct GateHolder {
    pub label: &'static str,
    pub since: Instant,
}

struct State {
    held: Option<GateHolder>,
    /// Waiters in arrival order. The gate is handed out first come, first
    /// served, so a loop that releases and immediately re-acquires (a chunked
    /// load) cannot starve a writer that is already waiting.
    queue: VecDeque<u64>,
    next_ticket: u64,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    wait: Duration,
}

/// Cheap to clone; every clone shares the same lock.
#[derive(Clone)]
pub struct WriteGate {
    shared: Arc<Shared>,
}

thread_local! {
    static THIS_THREAD_HOLDS_GATE: Cell<bool> = const { Cell::new(false) };
}

struct GateGuard<'a> {
    gate: &'a WriteGate,
}

impl Drop for GateGuard<'_> {
    fn drop(&mut self) {
        THIS_THREAD_HOLDS_GATE.with(|c| c.set(false));
        let mut state = self
            .gate
            .shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.held = None;
        self.gate.shared.changed.notify_all();
    }
}

impl WriteGate {
    pub fn new(wait: Duration) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    held: None,
                    queue: VecDeque::new(),
                    next_ticket: 0,
                }),
                changed: Condvar::new(),
                wait,
            }),
        }
    }

    /// The current holder, if any.
    pub fn holder(&self) -> Option<GateHolder> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .held
            .clone()
    }

    fn acquire(&self, label: &'static str) -> anyhow::Result<GateGuard<'_>> {
        // The gate is not reentrant. Waiting for a lock this thread already
        // holds would block until the timeout, so refuse at once.
        if THIS_THREAD_HOLDS_GATE.with(Cell::get) {
            anyhow::bail!(
                "write gate is not reentrant: `{label}` was called while this thread already \
                 holds the gate (a gated store method was called from inside another)"
            );
        }
        let deadline = Instant::now() + self.shared.wait;
        let mut state = self.shared.state.lock().unwrap_or_else(|e| e.into_inner());
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.queue.push_back(ticket);
        loop {
            if state.held.is_none() && state.queue.front() == Some(&ticket) {
                state.queue.pop_front();
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                let holder = state
                    .held
                    .as_ref()
                    .map_or("queued writers", |h| h.label)
                    .to_string();
                state.queue.retain(|t| *t != ticket);
                // The waiter behind us may now be at the front.
                self.shared.changed.notify_all();
                return Err(ToolError::WriteBusy {
                    holder,
                    waited_secs: self.shared.wait.as_secs(),
                }
                .into_anyhow());
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        state.held = Some(GateHolder {
            label,
            since: Instant::now(),
        });
        drop(state);
        THIS_THREAD_HOLDS_GATE.with(|c| c.set(true));
        Ok(GateGuard { gate: self })
    }

    /// Run `f` while holding the gate, on the current thread. Never call this
    /// around a `GatedStore` / `PrivilegedStore` method: they take the gate
    /// themselves, and the gate is not reentrant.
    pub fn run_blocking<T>(
        &self,
        label: &'static str,
        f: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _guard = self.acquire(label)?;
        f()
    }

    /// Like [`run_blocking`](Self::run_blocking), for callers that hold no
    /// store handle: the lock is taken, and `f` runs, on a blocking thread.
    pub async fn run<T: Send + 'static>(
        &self,
        label: &'static str,
        f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> anyhow::Result<T> {
        let gate = self.clone();
        tokio::task::spawn_blocking(move || gate.run_blocking(label, f))
            .await
            .map_err(|e| anyhow::anyhow!("write task failed: {e}"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn serializes_writers() {
        let gate = WriteGate::new(Duration::from_secs(5));
        let inside = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let (gate, inside, peak) = (gate.clone(), inside.clone(), peak.clone());
                std::thread::spawn(move || {
                    gate.run_blocking("w", || {
                        let n = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(n, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        inside.fetch_sub(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_waiting_writer_is_not_starved_by_a_loop_that_reacquires() {
        use std::sync::atomic::AtomicBool;
        let gate = WriteGate::new(Duration::from_secs(5));
        let stop = Arc::new(AtomicBool::new(false));
        let (g2, s2) = (gate.clone(), stop.clone());
        // A chunked load: take the gate, work, release, and immediately again.
        let looper = std::thread::spawn(move || {
            while !s2.load(Ordering::SeqCst) {
                g2.run_blocking("chunk", || {
                    std::thread::sleep(Duration::from_millis(20));
                    Ok(())
                })
                .unwrap();
            }
        });
        std::thread::sleep(Duration::from_millis(60));
        let started = Instant::now();
        gate.run_blocking("other", || Ok(())).unwrap();
        let waited = started.elapsed();
        stop.store(true, Ordering::SeqCst);
        looper.join().unwrap();
        assert!(
            waited < Duration::from_millis(250),
            "a waiting writer was starved for {waited:?} by a re-acquiring loop"
        );
    }

    #[test]
    fn times_out_with_write_busy_naming_the_holder() {
        let gate = WriteGate::new(Duration::from_millis(50));
        let g2 = gate.clone();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            g2.run_blocking("slow_write", || {
                held_tx.send(()).unwrap();
                done_rx.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        held_rx.recv().unwrap();
        let err = gate.run_blocking("other", || Ok(())).unwrap_err();
        match err.downcast_ref::<ToolError>() {
            Some(ToolError::WriteBusy { holder, .. }) => assert_eq!(holder, "slow_write"),
            other => panic!("expected write_busy, got {other:?}"),
        }
        done_tx.send(()).unwrap();
        t.join().unwrap();
        gate.run_blocking("after", || Ok(())).unwrap();
    }

    #[test]
    fn refuses_reentrant_use_at_once() {
        let gate = WriteGate::new(Duration::from_secs(60));
        let g2 = gate.clone();
        let started = Instant::now();
        let err = gate
            .run_blocking("outer", || g2.run_blocking("inner", || Ok(())))
            .unwrap_err();
        assert!(err.to_string().contains("not reentrant"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn async_run_keeps_the_gate_if_the_future_is_dropped() {
        let gate = WriteGate::new(Duration::from_millis(100));
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        let g2 = gate.clone();
        let fut = g2.run("slow", move || {
            held_tx.send(()).unwrap();
            done_rx.recv().unwrap();
            Ok(())
        });
        // Drop the request future shortly after the work started.
        let _ = tokio::time::timeout(Duration::from_millis(30), fut).await;
        held_rx.recv().unwrap();
        // The write is still running, so the gate is still held.
        assert!(gate.holder().is_some());
        let err = gate.run_blocking("second", || Ok(())).unwrap_err();
        assert!(matches!(
            err.downcast_ref::<ToolError>(),
            Some(ToolError::WriteBusy { .. })
        ));
        done_tx.send(()).unwrap();
        // Released once the work ends.
        for _ in 0..50 {
            if gate.holder().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(gate.holder().is_none());
    }
}
