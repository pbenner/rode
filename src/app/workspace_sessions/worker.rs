//! One bounded filesystem worker per window. Snapshot submissions coalesce;
//! ownership-changing requests stay ordered and return explicit acknowledgements.
use super::{
    model::*,
    store::{Lease, Store},
};
use anyhow::Result;
use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex, mpsc},
    time::Duration,
};

pub(super) enum Command {
    List,
    Create {
        serial: u64,
        snapshot: Box<Snapshot>,
    },
    Claim {
        key: Key,
        takeover: bool,
    },
    Release(Key),
    Rename(Key, String),
    Remove(Key, u64),
    Focus(Key),
    Stop,
}
struct Save {
    key: Key,
    serial: u64,
    snapshot: Box<Snapshot>,
}
pub(super) enum Event {
    Listed {
        rows: Vec<Row>,
        warnings: Vec<String>,
    },
    Created {
        record: Box<Record>,
        serial: u64,
    },
    Claimed {
        record: Box<Record>,
        takeover: bool,
    },
    Saved {
        key: Key,
        serial: u64,
    },
    Revoked(Key),
    Focus,
    Error {
        operation: Operation,
        message: String,
    },
    Changed,
}
#[derive(Clone)]
pub(super) enum Operation {
    List,
    Create(u64),
    Claim,
    Save(Key, u64),
    Change,
    Verify,
}
#[derive(Default)]
struct Progress {
    requested: u64,
    durable: u64,
    failed: bool,
}
#[derive(Default)]
pub(in crate::app) struct Barrier {
    state: Mutex<Progress>,
    ready: Condvar,
}
impl Barrier {
    pub fn flush(&self, timeout: Duration) -> bool {
        let state = self.state.lock().unwrap();
        let target = state.requested;
        let (state, _) = self
            .ready
            .wait_timeout_while(state, timeout, |s| s.durable < target && !s.failed)
            .unwrap();
        state.durable >= target
    }
    fn requested(&self, serial: u64) {
        let mut s = self.state.lock().unwrap();
        s.requested = serial;
        s.failed = false;
    }
    fn complete(&self, serial: u64, success: bool) {
        let mut s = self.state.lock().unwrap();
        if success {
            s.durable = s.durable.max(serial);
        } else if serial >= s.requested {
            s.failed = true;
        }
        self.ready.notify_all();
    }
}
pub(super) struct Worker {
    commands: mpsc::SyncSender<Command>,
    latest: Arc<Mutex<Option<Save>>>,
    pub events: mpsc::Receiver<Event>,
    pub barrier: Arc<Barrier>,
}
impl Worker {
    pub fn start(root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let store = Store::open(root)?;
        let (commands, input) = mpsc::sync_channel(16);
        let (output, events) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let barrier = Arc::new(Barrier::default());
        let pending = latest.clone();
        let progress = barrier.clone();
        std::thread::Builder::new()
            .name("workspace-sessions".into())
            .spawn(move || {
                run(
                    store,
                    || input.recv_timeout(Duration::from_millis(250)),
                    pending,
                    progress,
                    |event| {
                        let _ = output.send(event);
                        wake();
                    },
                );
            })?;
        Ok(Self {
            commands,
            latest,
            events,
            barrier,
        })
    }
    pub fn command(&self, command: Command) -> Result<()> {
        self.commands
            .try_send(command)
            .map_err(|_| anyhow::anyhow!("Workspace session operations are busy; try again"))
    }
    pub fn create(&self, serial: u64, snapshot: Snapshot) -> Result<()> {
        self.command(Command::Create {
            serial,
            snapshot: Box::new(snapshot),
        })?;
        self.barrier.requested(serial);
        Ok(())
    }
    pub fn save(&self, key: Key, serial: u64, snapshot: Snapshot) {
        self.barrier.requested(serial);
        *self.latest.lock().unwrap() = Some(Save {
            key,
            serial,
            snapshot: Box::new(snapshot),
        });
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.command(Command::Stop);
        let _ = self.barrier.flush(Duration::from_secs(1));
    }
}
fn save_pending(
    store: &Store,
    lease: &mut Option<Lease>,
    latest: &Mutex<Option<Save>>,
    progress: &Barrier,
    send: &impl Fn(Event),
) {
    let Some(save) = latest.lock().unwrap().take() else {
        return;
    };
    let result = lease
        .as_ref()
        .filter(|l| l.key == save.key)
        .ok_or_else(|| anyhow::anyhow!("Workspace session ownership was released"))
        .and_then(|lease| store.save(lease, *save.snapshot));
    progress.complete(save.serial, result.is_ok());
    match result {
        Ok(_) => send(Event::Saved {
            key: save.key,
            serial: save.serial,
        }),
        Err(error) => {
            if lease
                .as_ref()
                .is_some_and(|l| store.owns(l).is_ok_and(|yes| !yes))
            {
                lease.take();
                send(Event::Revoked(save.key.clone()));
            }
            send(Event::Error {
                operation: Operation::Save(save.key, save.serial),
                message: format!("{error:#}"),
            });
        }
    }
}
fn run(
    store: Store,
    mut receive: impl FnMut() -> Result<Command, mpsc::RecvTimeoutError>,
    latest: Arc<Mutex<Option<Save>>>,
    progress: Arc<Barrier>,
    send: impl Fn(Event),
) {
    let mut lease: Option<Lease> = None;
    let mut verification_error = String::new();
    loop {
        let command = receive();
        // A save can arrive while recv_timeout is waiting. Drain it before any
        // ownership-changing command, including Create and Claim.
        save_pending(&store, &mut lease, &latest, &progress, &send);
        let operation = match &command {
            Ok(Command::List) => Operation::List,
            Ok(Command::Create { serial, .. }) => Operation::Create(*serial),
            Ok(Command::Claim { .. }) => Operation::Claim,
            _ => Operation::Change,
        };
        let result = match command {
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(()),
            Ok(Command::List) => store.list().map(|catalogue| {
                send(Event::Listed {
                    rows: catalogue.rows,
                    warnings: catalogue.warnings,
                })
            }),
            Ok(Command::Create { serial, snapshot }) => {
                let result = store.create(*snapshot);
                progress.complete(serial, result.is_ok());
                result.map(|(record, owner)| {
                    lease = Some(owner);
                    send(Event::Created {
                        record: Box::new(record),
                        serial,
                    });
                })
            }
            Ok(Command::Claim { key, takeover }) => {
                store.claim(&key, takeover).map(|(record, owner)| {
                    lease = Some(owner);
                    send(Event::Claimed {
                        record: Box::new(record),
                        takeover,
                    });
                })
            }
            Ok(Command::Release(key)) => {
                if lease.as_ref().is_some_and(|l| l.key == key) {
                    lease.take();
                }
                send(Event::Changed);
                Ok(())
            }
            Ok(Command::Rename(key, name)) => {
                store.rename(&key, name).map(|_| send(Event::Changed))
            }
            Ok(Command::Remove(key, revision)) => {
                store.remove(&key, revision).map(|_| send(Event::Changed))
            }
            Ok(Command::Focus(key)) => store.focus(&key),
        };
        if let Err(error) = result {
            send(Event::Error {
                operation,
                message: format!("{error:#}"),
            });
        }
        if let Some(owner) = &lease {
            match store.owns(owner) {
                Ok(false) => {
                    verification_error.clear();
                    let key = owner.key.clone();
                    lease.take();
                    send(Event::Revoked(key));
                }
                Ok(true) => {
                    verification_error.clear();
                    if store.requested_focus(owner).unwrap_or(false) {
                        send(Event::Focus);
                    }
                }
                Err(error) => {
                    let message = format!("Cannot verify workspace ownership: {error:#}");
                    if message != verification_error {
                        verification_error = message.clone();
                        send(Event::Error {
                            operation: Operation::Verify,
                            message,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::workspace_sessions::store::tests::snapshot;

    #[test]
    fn release_drains_the_latest_checkpoint_before_releasing_ownership() {
        let temp = tempfile::tempdir().unwrap();
        let worker = Worker::start(temp.path().to_owned(), Arc::new(|| {})).unwrap();
        worker.create(1, snapshot()).unwrap();
        let Event::Created { record, .. } =
            worker.events.recv_timeout(Duration::from_secs(3)).unwrap()
        else {
            panic!("Creation failed")
        };
        let mut changed = snapshot();
        changed.draft = "Latest accepted draft".into();
        worker.save(record.key(), 2, changed);
        worker.command(Command::Release(record.key())).unwrap();
        loop {
            match worker.events.recv_timeout(Duration::from_secs(3)).unwrap() {
                Event::Saved { serial: 2, .. } => {}
                Event::Changed => break,
                _ => panic!("Unexpected checkpoint result"),
            }
        }
        assert!(worker.barrier.flush(Duration::from_millis(1)));
        let saved: Record = serde_json::from_slice(
            &std::fs::read(temp.path().join(format!("{}.json", record.id))).unwrap(),
        )
        .unwrap();
        assert_eq!(saved.snapshot.draft, "Latest accepted draft");
        assert!(
            !Store::open(temp.path().to_owned())
                .unwrap()
                .list()
                .unwrap()
                .rows[0]
                .active
        );
    }
    #[test]
    fn a_save_arriving_with_a_claim_is_durable_before_the_old_lease_is_replaced() {
        use std::cell::RefCell;
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().to_owned()).unwrap();
        let (destination, lease) = store.create(snapshot()).unwrap();
        drop(lease);
        let latest = Arc::new(Mutex::new(None));
        let progress = Arc::new(Barrier::default());
        let first = RefCell::new(None::<Key>);
        let mut turn = 0;
        let receive = || {
            turn += 1;
            Ok(match turn {
                1 => Command::Create {
                    serial: 1,
                    snapshot: Box::new(snapshot()),
                },
                2 => {
                    let mut changed = snapshot();
                    changed.draft = "Accepted before switching".into();
                    progress.requested(2);
                    *latest.lock().unwrap() = Some(Save {
                        key: first.borrow().clone().unwrap(),
                        serial: 2,
                        snapshot: Box::new(changed),
                    });
                    Command::Claim {
                        key: destination.key(),
                        takeover: false,
                    }
                }
                _ => Command::Stop,
            })
        };
        run(store, receive, latest.clone(), progress.clone(), |event| {
            if let Event::Created { record, .. } = event {
                *first.borrow_mut() = Some(record.key());
            }
        });
        let saved: Record = serde_json::from_slice(
            &std::fs::read(
                temp.path()
                    .join(format!("{}.json", first.into_inner().unwrap().id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved.snapshot.draft, "Accepted before switching");
        assert!(progress.flush(Duration::from_millis(1)));
    }
}
