//! Confirmed mutations have their own bounded worker, independent of discovery.
use crate::{runtime, sandbox, ssh};
use anyhow::{Result, ensure};
use rode_runtime_core::service::Info;
use std::{collections::BTreeSet, sync::mpsc};

const LIMIT: usize = 32;

#[derive(Clone)]
pub(super) struct Request {
    pub info: Info,
    pub plan: sandbox::Plan,
    pub remote: Option<ssh::Remote>,
}

pub(super) struct Queue {
    requests: mpsc::SyncSender<Request>,
    results: mpsc::Receiver<(String, Result<(), String>)>,
    pending: BTreeSet<String>,
    pub error: String,
}
impl Queue {
    pub fn new(wake: impl Fn() + Send + 'static) -> Self {
        Self::with_runner(
            |request| runtime::end(&request.plan, request.remote.as_ref(), &request.info),
            wake,
        )
    }
    fn with_runner(
        run: impl Fn(Request) -> Result<()> + Send + 'static,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (requests, queued) = mpsc::sync_channel::<Request>(LIMIT);
        let (finished, results) = mpsc::channel();
        std::thread::spawn(move || {
            // Dropping a browser must still drain every approved request, even
            // when nobody remains to receive its result.
            for request in queued {
                let id = request.info.id.clone();
                let result = run(request).map_err(|e| format!("{e:#}"));
                let _ = finished.send((id, result));
                wake();
            }
        });
        Self {
            requests,
            results,
            pending: BTreeSet::new(),
            error: String::new(),
        }
    }
    pub fn contains(&self, id: &str) -> bool {
        self.pending.contains(id)
    }
    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }
    pub fn push(&mut self, request: Request) -> Result<()> {
        let id = request.info.id.clone();
        ensure!(!self.contains(&id), "This runtime is already queued to end");
        ensure!(
            self.pending.len() < LIMIT,
            "Runtime shutdown queue is full; wait for an existing shutdown"
        );
        self.requests
            .try_send(request)
            .map_err(|e| anyhow::anyhow!("Could not queue runtime shutdown: {e}"))?;
        self.pending.insert(id);
        self.error.clear();
        Ok(())
    }
    /// Report new completion/failure once so a waiting exit can be cancelled.
    /// The last failure also remains visible across discovery refreshes.
    pub fn poll(&mut self) -> Result<bool, String> {
        let mut changed = false;
        let mut failure = None;
        loop {
            match self.results.try_recv() {
                Ok((id, result)) => {
                    self.pending.remove(&id);
                    changed = true;
                    if let Err(error) = result {
                        self.error = error.clone();
                        failure = Some(error);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.busy() {
                        self.error = "Runtime shutdown worker disconnected; inspect Sessions before releasing its allocation".into();
                        failure = Some(self.error.clone());
                        self.pending.clear();
                        changed = true;
                    }
                    break;
                }
            }
        }
        failure.map_or(Ok(changed), Err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn request(id: &str) -> Request {
        Request {
            info: serde_json::from_value(serde_json::json!({
                "id":id, "name":"test", "kind":"notebook", "root":"/captured",
                "policy":"original", "mode":"off", "document":"test.ipynb",
                "persistent":true, "attached":false, "alive":true, "busy":false,
                "status":"idle", "created":0, "python":"off", "configuration":"python3"
            }))
            .unwrap(),
            plan: sandbox::Plan::default(),
            remote: None,
        }
    }

    #[test]
    fn approved_shutdown_runs_while_discovery_is_pending_and_errors_survive() {
        let (_discovery, pending_discovery) = mpsc::channel::<()>();
        let (done, completed) = mpsc::channel();
        let mut queue = Queue::with_runner(
            move |request| {
                assert_eq!(request.info.root, "/captured");
                assert_eq!(request.info.policy, "original");
                done.send(()).unwrap();
                anyhow::bail!("Host refused shutdown")
            },
            || {},
        );
        queue.push(request("first")).unwrap();
        completed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            pending_discovery.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        let until = Instant::now() + Duration::from_secs(5);
        while queue.busy() {
            let _ = queue.poll();
            assert!(Instant::now() < until);
            std::thread::yield_now();
        }
        assert_eq!(queue.error, "Host refused shutdown");
        assert!(!queue.poll().unwrap());
        assert_eq!(queue.error, "Host refused shutdown");
    }

    #[test]
    fn project_reset_keeps_approved_shutdowns_and_their_original_context() {
        let (release, gate) = mpsc::channel();
        let mut queue = Queue::with_runner(
            move |request| {
                gate.recv().unwrap();
                assert_eq!(request.info.root, "/captured");
                assert_eq!(request.info.policy, "original");
                Ok(())
            },
            || {},
        );
        queue.push(request("original-host")).unwrap();
        let mut browser = super::super::Browser {
            endings: Some(queue),
            ..Default::default()
        };
        let mut replacement = browser.take_shutdowns();
        assert!(!browser.shutdown_pending());
        assert!(replacement.shutdown_pending());
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while replacement.shutdown_pending() {
            replacement.endings.as_mut().unwrap().poll().unwrap();
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }

    #[test]
    fn pending_exit_waits_for_shutdown_and_aborts_on_failure() {
        use crate::app::{Dialog, EditorApp, Startup};
        for succeeds in [false, true] {
            let ctx = eframe::egui::Context::default();
            let cc = eframe::CreationContext::_new_kittest(ctx.clone());
            let mut app = EditorApp::new(&cc, Startup::default());
            let (release, gate) = mpsc::channel();
            let mut queue = Queue::with_runner(
                move |_| {
                    gate.recv().unwrap();
                    ensure!(
                        succeeds,
                        "Host connection lost; inspect the retained runtime"
                    );
                    Ok(())
                },
                || {},
            );
            queue.push(request("closing")).unwrap();
            app.sessions.runtimes.endings = Some(queue);
            app.quit_terminals(&ctx);
            assert!(app.sessions.quit_waiting);
            assert!(!app.quitting);
            release.send(()).unwrap();
            let until = Instant::now() + Duration::from_secs(5);
            while app.sessions.quit_waiting && !app.quitting {
                app.session_tick(&ctx);
                assert!(Instant::now() < until);
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(app.quitting, succeeds);
            if !succeeds {
                assert!(!app.sessions.quit_waiting);
                assert!(app.dialog == Dialog::None);
                assert!(app.notice.as_ref().is_some_and(
                    |(message, error, _)| *error && message.contains("Host connection lost")
                ));
                assert!(
                    !app.sessions
                        .runtimes
                        .endings
                        .as_mut()
                        .unwrap()
                        .poll()
                        .unwrap()
                );
            }
        }
    }

    #[test]
    fn shutdown_queue_is_bounded_deduplicated_and_drains_after_browser_drop() {
        let (release, gate) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let mut queue = Queue::with_runner(
            move |request| {
                if request.info.id == "0" {
                    gate.recv().unwrap();
                }
                done.send(request.info.id).unwrap();
                Ok(())
            },
            || {},
        );
        queue.push(request("0")).unwrap();
        assert!(queue.push(request("0")).is_err());
        for i in 1..LIMIT {
            queue.push(request(&i.to_string())).unwrap();
        }
        assert!(queue.push(request("overflow")).is_err());
        drop(queue);
        release.send(()).unwrap();
        for i in 0..LIMIT {
            assert_eq!(
                completed.recv_timeout(Duration::from_secs(5)).unwrap(),
                i.to_string()
            );
        }
    }
}
