pub mod actor;
pub mod commands;
pub mod data_dir;
pub mod domain;
pub mod events;
pub mod live;
pub mod platform;
pub mod provider;
pub mod reconcile;
pub mod store;
#[cfg(test)]
mod tests;

use actor::{Actor, EngineMsg};
use commands::{Ack, Command, CommandError};
use data_dir::{DataDir, LockError};
use domain::ThreadId;
use events::{ThreadSnapshot, UiEvent};
use provider::Spawner;
use std::{fs::OpenOptions, sync::Arc, time::Duration};
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot},
};

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("{0}")]
    Lock(#[from] LockError),
    #[error("database: {0}")]
    Store(#[from] store::StoreError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}
struct EngineInner {
    sender: mpsc::Sender<EngineMsg>,
    schema_version: u32,
}
impl Drop for EngineInner {
    fn drop(&mut self) {
        let _ = self.sender.try_send(EngineMsg::Shutdown);
    }
}
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}
impl Engine {
    pub fn start(runtime: &Handle, data: DataDir) -> Result<Self, StartError> {
        let lock = data.lock()?;
        platform::private_file(
            &data.path().join("veyra.db"),
            OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .truncate(false),
        )?;
        let db = store::open(&data.path().join("veyra.db"))?;
        Self::launch(runtime, db, Spawner::Cli, Some(lock)).map_err(StartError::Store)
    }
    fn launch(
        runtime: &Handle,
        db: rusqlite::Connection,
        spawner: Spawner,
        lock: Option<data_dir::InstanceLock>,
    ) -> Result<Self, store::StoreError> {
        let version = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let (sender, receiver) = mpsc::channel(256);
        let actor = Actor::new(db, sender.clone(), spawner)?;
        let handle = runtime.clone();
        let tick_sender = sender.clone();
        let tick = runtime.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if tick_sender.send(EngineMsg::Tick).await.is_err() {
                    break;
                }
            }
        });
        std::thread::Builder::new()
            .name("veyra-engine".into())
            .spawn(move || {
                let _lock = lock;
                actor.run(receiver, handle);
                tick.abort();
            })
            .map_err(|e| store::StoreError::Invalid(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(EngineInner {
                sender,
                schema_version: version,
            }),
        })
    }
    pub fn schema_version(&self) -> u32 {
        self.inner.schema_version
    }
    pub fn send(&self, command: Command) -> oneshot::Receiver<Result<Ack, CommandError>> {
        let (reply, rx) = oneshot::channel();
        match self
            .inner
            .sender
            .try_send(EngineMsg::Command { command, reply })
        {
            Ok(()) => {}
            Err(error) => {
                if let EngineMsg::Command { reply, .. } = error.into_inner() {
                    let _ = reply.send(Err(CommandError::Busy("engine command queue".into())));
                }
            }
        }
        rx
    }
    pub fn subscribe(&self) -> mpsc::UnboundedReceiver<UiEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = self.inner.sender.try_send(EngineMsg::Subscribe(tx));
        rx
    }
    pub fn snapshot(&self, thread: ThreadId) -> oneshot::Receiver<ThreadSnapshot> {
        let (reply, rx) = oneshot::channel();
        let _ = self
            .inner
            .sender
            .try_send(EngineMsg::Snapshot { thread, reply });
        rx
    }
    #[cfg(test)]
    pub(crate) fn scripted(
        runtime: &Handle,
        db: rusqlite::Connection,
        script: provider::scripted::Scripted,
    ) -> Result<Self, store::StoreError> {
        Self::launch(runtime, db, Spawner::Scripted(script), None)
    }
}
