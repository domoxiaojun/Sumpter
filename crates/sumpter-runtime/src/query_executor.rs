//! Async boundary for SQLite queries and administrative mutations.
//! Four dedicated blocking workers are shared by both adapters. Waiting for a
//! slot is asynchronous; a dropped waiter cancels its SQL/cursor work as well.
use std::{
    cell::RefCell,
    sync::{Arc, OnceLock},
};
use tokio::sync::{Semaphore, watch};

const CONCURRENCY: usize = 4;
thread_local! {
    static CANCEL: RefCell<Option<watch::Receiver<bool>>> = const { RefCell::new(None) };
}

pub(crate) fn cancellation() -> Option<watch::Receiver<bool>> {
    CANCEL.with(|value| value.borrow().clone())
}

/// Rollback/connection close must run even when the query was cancelled.
pub(crate) fn without_cancellation<T>(work: impl FnOnce() -> T) -> T {
    struct Restore(Option<watch::Receiver<bool>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CANCEL.with(|value| *value.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CANCEL.with(|value| value.borrow_mut().take()));
    work()
}

fn executor() -> &'static tokio::runtime::Runtime {
    static EXECUTOR: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    EXECUTOR.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(CONCURRENCY)
            .thread_name("runtime-query")
            .enable_all()
            .build()
            .expect("runtime query executor")
    })
}

struct CancelOnDrop(watch::Sender<bool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}
struct ResetContext;
impl Drop for ResetContext {
    fn drop(&mut self) {
        CANCEL.with(|value| *value.borrow_mut() = None);
    }
}

/// Internal sync SQL implementations stay on runtime-owned workers. Adapters
/// call typed async APIs instead of spawning unbounded blocking jobs.
pub async fn run<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    execute(work, true).await
}

/// Once admitted, a mutation completes even if its HTTP client disconnects;
/// cancelling between DB commit and in-memory bookkeeping would split state.
pub async fn run_mutation<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    execute(work, false).await
}

async fn execute<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    cancellable: bool,
) -> Result<T, String> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    let permit = SLOTS
        .get_or_init(|| Arc::new(Semaphore::new(CONCURRENCY)))
        .clone()
        .acquire_owned()
        .await
        .map_err(|error| error.to_string())?;
    let (sender, receiver) = watch::channel(false);
    let _cancel = CancelOnDrop(sender);
    executor()
        .spawn_blocking(move || {
            let _permit = permit;
            CANCEL.with(|value| *value.borrow_mut() = cancellable.then_some(receiver));
            let _reset = ResetContext;
            work()
        })
        .await
        .map_err(|error| format!("runtime query worker: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(flavor = "current_thread")]
    async fn queries_are_bounded_responsive_and_cancel_sql_waits() {
        let mut releases = Vec::new();
        let mut tasks = Vec::new();
        for _ in 0..CONCURRENCY {
            let (release, wait) = std::sync::mpsc::channel();
            let (started, ready) = tokio::sync::oneshot::channel();
            tasks.push(tokio::spawn(run(move || {
                let _ = started.send(());
                wait.recv().unwrap();
            })));
            tokio::time::timeout(Duration::from_secs(2), ready)
                .await
                .unwrap()
                .unwrap();
            releases.push(release);
        }
        let (fifth_started, mut fifth_ready) = tokio::sync::oneshot::channel();
        let fifth = tokio::spawn(run(move || {
            let _ = fifth_started.send(());
        }));
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(matches!(
            fifth_ready.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        fifth.abort(); // queued cancellation never starts the closure
        for release in releases {
            release.send(()).unwrap();
        }
        for task in tasks {
            task.await.unwrap().unwrap();
        }
        let _ = fifth.await;

        // A cancelled async caller aborts the ORM future instead of retaining
        // a query worker forever. This requires no production-size database.
        let (started, ready) = tokio::sync::oneshot::channel();
        let (finished, done) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(run(move || {
            let connection = crate::database::Connection::open_in_memory().unwrap();
            let result = connection.orm(move |_| async move {
                let _ = started.send(());
                futures_util::future::pending::<()>().await;
                Ok(())
            });
            assert!(result.unwrap_err().to_string().contains("cancelled"));
            drop(connection);
            let _ = finished.send(());
        }));
        tokio::time::timeout(Duration::from_secs(2), ready)
            .await
            .unwrap()
            .unwrap();
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(2), done)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(run(|| 42).await.unwrap(), 42);
    }
}
