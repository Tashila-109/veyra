use std::sync::Mutex;
#[cfg(debug_assertions)]
use std::time::Instant;
pub mod actor;
pub mod commands;
pub mod data_dir;
pub mod domain;
pub mod events;
pub mod fixture;
pub mod live;
pub mod platform;
pub mod process;
pub mod provider;
pub mod reconcile;
pub mod record;
pub mod store;
#[cfg(test)]
mod tests;

use actor::{Actor, EngineMsg};
use commands::{Ack, Command, CommandError};
use data_dir::{DataDir, LockError};
use domain::ThreadId;
use events::{ThreadSnapshot, UiEvent};
use provider::Spawner;
use std::{
    fs::OpenOptions,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
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
    state: Arc<RuntimeState>,
    runtime: Handle,
    groups: provider::env::Groups,
}
#[derive(Default)]
pub(crate) struct RuntimeState {
    pub awake: AtomicBool,
    pub closing: Arc<LaunchGate>,
    pub window_threads: Mutex<std::collections::BTreeMap<ThreadId, WindowThread>>,
    #[cfg(debug_assertions)]
    pub counters: Mutex<ProbeCounters>,
    #[cfg(debug_assertions)]
    pub processes: Mutex<std::collections::BTreeMap<ThreadId, u32>>,
}
#[derive(Debug, Default)]
pub(crate) struct LaunchGate {
    closing: AtomicBool,
    lock: Mutex<()>,
    #[cfg(test)]
    worst_hold_ns: std::sync::atomic::AtomicU64,
}
impl LaunchGate {
    fn enter(&self) -> LaunchGuard<'_> {
        let guard = self.lock.lock().unwrap();
        LaunchGuard {
            _guard: guard,
            #[cfg(test)]
            gate: self,
            #[cfg(test)]
            started: Instant::now(),
        }
    }
    fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }
    fn close(&self) {
        let _guard = self.enter();
        self.closing.store(true, Ordering::Release);
    }
}
struct LaunchGuard<'a> {
    _guard: std::sync::MutexGuard<'a, ()>,
    #[cfg(test)]
    gate: &'a LaunchGate,
    #[cfg(test)]
    started: Instant,
}
#[cfg(test)]
impl Drop for LaunchGuard<'_> {
    fn drop(&mut self) {
        self.gate.worst_hold_ns.fetch_max(
            self.started.elapsed().as_nanos().min(u64::MAX as u128) as u64,
            Ordering::Relaxed,
        );
    }
}
#[cfg(test)]
impl Drop for LaunchGate {
    fn drop(&mut self) {
        eprintln!(
            "launch_gate_worst_hold_ns={}",
            self.worst_hold_ns.load(Ordering::Relaxed)
        );
    }
}
#[derive(Clone, Copy)]
pub struct WindowThread {
    pub wait_reason: Option<domain::WaitReason>,
    pub busy: bool,
    pub pending_turn: Option<domain::TurnId>,
}
#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct ProbeCounters {
    pub commits: u64,
    pub actor_max_ms: f64,
    pub started: Option<Instant>,
}
#[cfg(debug_assertions)]
#[derive(serde::Serialize)]
pub struct Perf {
    pub commits: u64,
    pub actor_max_ms: f64,
    pub actor_busy_ms: f64,
}
#[cfg(debug_assertions)]
#[derive(serde::Serialize)]
pub struct Process {
    pub thread: ThreadId,
    pub pid: u32,
    pub resident_bytes: Option<u64>,
}
impl Drop for EngineInner {
    fn drop(&mut self) {
        self.state.closing.close();
        let _ = self.sender.try_send(EngineMsg::Shutdown);
    }
}
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}
impl Engine {
    pub fn start(runtime: &Handle, data: DataDir) -> Result<Self, StartError> {
        Self::start_with(runtime, data, |env| env)
    }
    pub fn start_with_processes(
        runtime: &Handle,
        data: DataDir,
        pids: Vec<u32>,
    ) -> Result<Self, StartError> {
        Self::start_with(runtime, data, |mut env| {
            env.reaper_pids = Some(pids.into_iter().collect());
            env
        })
    }
    fn start_with(
        runtime: &Handle,
        data: DataDir,
        prepare: impl FnOnce(provider::env::ChildEnv) -> provider::env::ChildEnv,
    ) -> Result<Self, StartError> {
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
        let env = prepare(provider::env::ChildEnv::load());
        process::start(store::live_processes(&db)?, &env);
        Self::launch(runtime, db, Spawner::Cli(env), Some(lock))
    }
    fn launch(
        runtime: &Handle,
        db: rusqlite::Connection,
        spawner: Spawner,
        lock: Option<data_dir::InstanceLock>,
    ) -> Result<Self, StartError> {
        let version = db
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(store::StoreError::from)?;
        let (sender, receiver) = mpsc::channel(256);
        let actor = Actor::new(db, sender.clone(), spawner)?;
        let state = actor.state.clone();
        let groups = match &actor.spawner {
            Spawner::Cli(env) => env.groups.clone(),
            #[cfg(test)]
            Spawner::Scripted(_) => Default::default(),
        };
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
        let tick_abort = tick.abort_handle();
        std::thread::Builder::new()
            .name("veyra-engine".into())
            .spawn(move || {
                let _lock = lock;
                actor.run(receiver, handle);
                tick.abort();
            })
            .inspect_err(|_| tick_abort.abort())?;
        Ok(Self {
            inner: Arc::new(EngineInner {
                sender,
                schema_version: version,
                state,
                runtime: runtime.clone(),
                groups,
            }),
        })
    }
    pub fn shutdown(&self) {
        self.inner.state.closing.close();
        if let Err(mpsc::error::TrySendError::Full(message)) =
            self.inner.sender.try_send(EngineMsg::Shutdown)
        {
            let sender = self.inner.sender.clone();
            self.inner.runtime.spawn(async move {
                let _ = sender.send(message).await;
            });
        }
    }
    pub async fn shutdown_wait(&self) {
        self.shutdown();
        let sender = self.inner.sender.clone();
        let _ = self
            .inner
            .runtime
            .spawn(async move {
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(3), sender.closed()).await;
            })
            .await;
    }
    pub async fn on_app_quit(&self) {
        self.shutdown();
        for &group in self.inner.groups.lock().unwrap().iter() {
            let _ = platform::signal_group(group, platform::Signal::Terminate);
        }
        let _ = self
            .inner
            .runtime
            .spawn(async {
                tokio::time::sleep(Duration::from_millis(100)).await;
            })
            .await;
    }
    pub fn child_env(&self) -> provider::env::ChildEnv {
        let mut env = provider::env::ChildEnv::load();
        env.closing = self.inner.state.closing.clone();
        env.groups = self.inner.groups.clone();
        env
    }
    pub fn window_threads(&self) -> std::collections::BTreeMap<ThreadId, WindowThread> {
        self.inner.state.window_threads.lock().unwrap().clone()
    }
    pub fn is_closing(&self) -> bool {
        self.inner.state.closing.is_closing()
    }
    pub fn awake(&self) -> bool {
        self.inner.state.awake.load(Ordering::Acquire)
    }
    #[cfg(debug_assertions)]
    pub fn perf(&self) -> Perf {
        let counters = self.inner.state.counters.lock().unwrap();
        Perf {
            commits: counters.commits,
            actor_max_ms: counters.actor_max_ms,
            actor_busy_ms: counters
                .started
                .map_or(0.0, |at| at.elapsed().as_secs_f64() * 1000.0),
        }
    }
    #[cfg(debug_assertions)]
    pub fn processes(&self) -> Vec<Process> {
        self.inner
            .state
            .processes
            .lock()
            .unwrap()
            .iter()
            .map(|(&thread, &pid)| Process {
                thread,
                pid,
                resident_bytes: platform::resident_bytes(pid).ok(),
            })
            .collect()
    }
    pub fn schema_version(&self) -> u32 {
        self.inner.schema_version
    }
    pub fn send(&self, command: Command) -> oneshot::Receiver<Result<Ack, CommandError>> {
        let (reply, rx) = oneshot::channel();
        if self.inner.state.closing.is_closing() {
            let _ = reply.send(Err(CommandError::Closed));
            return rx;
        }
        match self
            .inner
            .sender
            .try_send(EngineMsg::Command { command, reply })
        {
            Ok(()) => {}
            Err(error) => {
                let cause = match &error {
                    mpsc::error::TrySendError::Full(_) => CommandError::QueueFull,
                    mpsc::error::TrySendError::Closed(_) => CommandError::Closed,
                };
                if let EngineMsg::Command { reply, .. } = error.into_inner() {
                    let _ = reply.send(Err(cause));
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
    ) -> Result<Self, StartError> {
        Self::launch(runtime, db, Spawner::Scripted(script), None)
    }
}
