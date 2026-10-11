use super::{ProviderError, ProviderKind};
#[cfg(debug_assertions)]
use crate::domain::ThreadId;
use crate::platform;
#[cfg(debug_assertions)]
use std::collections::BTreeMap;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::process::{Child, Command};

pub(crate) type Groups = Arc<Mutex<std::collections::BTreeSet<u32>>>;

struct LoginPath {
    path: OsString,
    failure: Option<std::io::Error>,
}
static LOGIN_PATH: LazyLock<LoginPath> = LazyLock::new(|| match platform::login_shell_path() {
    Ok(path) => LoginPath {
        path,
        failure: None,
    },
    Err(error) => LoginPath {
        path: std::env::var_os("PATH").unwrap_or_default(),
        failure: Some(error),
    },
});

#[derive(Clone, Debug)]
pub struct ChildEnv {
    pub(crate) groups: Groups,
    pub(crate) closing: Arc<crate::LaunchGate>,
    #[cfg(test)]
    pub(crate) launches: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    pub(crate) close_before_spawn: Arc<AtomicBool>,
    #[cfg(test)]
    pub(crate) close_before_user_write: Arc<AtomicBool>,
    pub path: OsString,
    pub home: Option<PathBuf>,
    #[cfg(debug_assertions)]
    lanes: Arc<Mutex<Lanes>>,
    #[cfg(debug_assertions)]
    pub claude_bin: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub fixtures: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub record: Option<PathBuf>,
    pub reaper_pids: Option<std::collections::BTreeSet<u32>>,
}
impl ChildEnv {
    pub fn load() -> Self {
        if let Some(error) = &LOGIN_PATH.failure {
            tracing::warn!(%error, "login shell PATH unavailable; using inherited PATH");
        }
        Self {
            groups: Default::default(),
            closing: Default::default(),
            #[cfg(test)]
            launches: Default::default(),
            #[cfg(test)]
            close_before_spawn: Default::default(),
            #[cfg(test)]
            close_before_user_write: Default::default(),
            path: LOGIN_PATH.path.clone(),
            home: std::env::var_os("HOME").map(PathBuf::from),
            #[cfg(debug_assertions)]
            lanes: Arc::default(),
            #[cfg(debug_assertions)]
            claude_bin: std::env::var_os("VEYRA_CLAUDE_BIN").map(PathBuf::from),
            #[cfg(debug_assertions)]
            fixtures: std::env::var_os("VEYRA_FAKE_FIXTURES").map(PathBuf::from),
            #[cfg(debug_assertions)]
            record: std::env::var_os("VEYRA_RECORD").map(PathBuf::from),
            #[cfg(debug_assertions)]
            reaper_pids: std::env::var("VEYRA_REAP_PIDS")
                .ok()
                .map(|text| text.split(',').filter_map(|pid| pid.parse().ok()).collect()),
            #[cfg(not(debug_assertions))]
            reaper_pids: None,
        }
    }
    pub fn resolve(&self, provider: ProviderKind) -> Result<PathBuf, ProviderError> {
        #[cfg(debug_assertions)]
        if provider == ProviderKind::Claude
            && let Some(path) = &self.claude_bin
        {
            return executable(path)
                .ok_or_else(|| ProviderError::NotInstalled(path.display().to_string()));
        }
        let name = provider.binary();
        let gui = std::env::var_os("PATH").unwrap_or_default();
        let known = platform::KNOWN_BIN_DIRS.iter().map(|dir| {
            if dir.starts_with('.') {
                self.home.as_deref().unwrap_or(Path::new("")).join(dir)
            } else {
                PathBuf::from(dir)
            }
        });
        std::env::split_paths(&gui)
            .chain(std::env::split_paths(&self.path))
            .chain(known)
            .filter(|dir| dir.is_absolute())
            .find_map(|dir| executable(&dir.join(name)))
            .ok_or_else(|| ProviderError::NotInstalled(name.into()))
    }
    pub fn command(&self, binary: &Path, args: &[String], cwd: &Path) -> Command {
        let mut cmd = Command::new(binary);
        cmd.args(args)
            .current_dir(cwd)
            .env("PATH", &self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("VEYRA_")
                || [
                    "CLAUDECODE",
                    "CLAUDE_CODE_ENTRYPOINT",
                    "CLAUDE_CODE_SSE_PORT",
                    "CLAUDE_AGENT_SDK_VERSION",
                ]
                .contains(&name.as_ref())
            {
                cmd.env_remove(key);
            }
        }
        cmd.env("VEYRA_PARENT_PID", std::process::id().to_string());
        platform::own_group(&mut cmd);
        cmd
    }
    pub fn spawn(
        &self,
        binary: &Path,
        args: &[String],
        cwd: &Path,
        fixture: Option<&Path>,
    ) -> Result<Child, ProviderError> {
        if self.closing.is_closing() {
            return Err(ProviderError::Cancelled);
        }
        let mut cmd = self.command(binary, args, cwd);
        #[cfg(debug_assertions)]
        if let Some(fixture) = fixture {
            if !fixture.is_file() {
                return Err(ProviderError::FixtureMissing(fixture.to_owned()));
            }
            cmd.env("VEYRA_FAKE_FIXTURE", std::fs::canonicalize(fixture)?);
        }
        #[cfg(not(debug_assertions))]
        let _ = fixture;
        #[cfg(test)]
        if self.close_before_spawn.swap(false, Ordering::AcqRel) {
            self.closing.close();
        }
        let admission = self.closing.enter();
        if self.closing.is_closing() {
            return Err(ProviderError::Cancelled);
        }
        let child = cmd.spawn()?;
        self.groups
            .lock()
            .unwrap()
            .insert(child.id().expect("new child"));
        drop(admission);
        #[cfg(test)]
        self.launches
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        if let Err(error) = platform::lower_priority(child.id().expect("new child")) {
            tracing::warn!(%error, "could not lower agent group priority");
        }
        Ok(child)
    }
}
fn executable(path: &Path) -> Option<PathBuf> {
    platform::is_executable(path)
        .then(|| std::fs::canonicalize(path).ok())
        .flatten()
}

#[cfg(debug_assertions)]
#[derive(Debug, Default)]
struct Lanes {
    threads: BTreeMap<ThreadId, (usize, u32)>,
}
#[cfg(debug_assertions)]
impl ChildEnv {
    pub(crate) fn register_thread(&self, thread: ThreadId) {
        let mut lanes = self.lanes.lock().unwrap();
        let next = lanes.threads.len() + 1;
        lanes.threads.entry(thread).or_insert((next, 0));
    }
    pub(crate) fn next_lane(&self, thread: ThreadId) -> (usize, u32) {
        let mut lanes = self.lanes.lock().unwrap();
        let next = lanes.threads.len() + 1;
        let entry = lanes.threads.entry(thread).or_insert((next, 0));
        entry.1 += 1;
        *entry
    }
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::ChildEnv;
    use crate::domain::ThreadId;
    #[test]
    fn resolution_skips_relative_and_empty_path_entries() {
        if let Some(root) = std::env::var_os("VEYRA_TEST_RESOLUTION") {
            let env = super::ChildEnv {
                groups: Default::default(),
                closing: Default::default(),
                launches: Default::default(),
                #[cfg(test)]
                close_before_spawn: Default::default(),
                #[cfg(test)]
                close_before_user_write: Default::default(),
                path: "bin::.".into(),
                home: None,
                lanes: Default::default(),
                claude_bin: None,
                fixtures: None,
                record: None,
                #[cfg(debug_assertions)]
                reaper_pids: None,
            };
            let selected = env.resolve(super::ProviderKind::Claude).ok();
            let planted =
                std::fs::canonicalize(std::path::PathBuf::from(root).join("bin/claude")).unwrap();
            assert_ne!(selected, Some(planted.clone()));
            let absolute = super::ChildEnv {
                path: planted.parent().unwrap().as_os_str().into(),
                ..env
            };
            assert_eq!(
                absolute.resolve(super::ProviderKind::Claude).unwrap(),
                planted
            );
            return;
        }
        let root = std::env::temp_dir().join(format!("veyra-resolution-{}", ThreadId::new()));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::copy("/bin/echo", root.join("bin/claude")).unwrap();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider::env::tests::resolution_skips_relative_and_empty_path_entries",
            ])
            .current_dir(&root)
            .env("PATH", "bin::.")
            .env("VEYRA_TEST_RESOLUTION", &root)
            .output()
            .unwrap();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
    #[test]
    fn recording_lanes_follow_creation_not_spawn_order() {
        let env = ChildEnv::load();
        let first = ThreadId::new();
        let second = ThreadId::new();
        env.register_thread(first);
        env.register_thread(second);
        let second_first = env.next_lane(second);
        let first_first = env.next_lane(first);
        assert!(first_first.0 < second_first.0);
        assert_eq!(first_first.1, 1);
        assert_eq!(second_first.1, 1);
        assert_eq!(env.next_lane(second), (second_first.0, 2));
    }

    #[tokio::test]
    async fn closing_state_refuses_every_provider_spawn() {
        let env = ChildEnv::load();
        let cwd = std::path::Path::new("/tmp");
        let binary = std::path::Path::new("/usr/bin/true");
        let mut positive = env.spawn(binary, &[], cwd, None).unwrap();
        positive.wait().await.unwrap();
        assert_eq!(env.launches.load(super::Ordering::Acquire), 1);
        env.closing.close();
        assert!(matches!(
            env.clone().spawn(binary, &[], cwd, None),
            Err(super::ProviderError::Cancelled)
        ));
        assert!(matches!(
            env.spawn(
                binary,
                &[],
                cwd,
                Some(std::path::Path::new("/nonexistent/fixture"))
            ),
            Err(super::ProviderError::Cancelled)
        ));
        assert_eq!(env.launches.load(super::Ordering::Acquire), 1);
    }
    #[tokio::test]
    async fn shutdown_between_final_launch_check_and_spawn_starts_no_process() {
        let env = ChildEnv::load();
        let cwd = std::path::Path::new("/tmp");
        let binary = std::path::Path::new("/usr/bin/true");
        let mut positive = env.spawn(binary, &[], cwd, None).unwrap();
        positive.wait().await.unwrap();
        assert_eq!(env.launches.load(super::Ordering::Acquire), 1);
        env.close_before_spawn.store(true, super::Ordering::Release);
        let result = env.spawn(binary, &[], cwd, None);
        let cancelled = matches!(result, Err(super::ProviderError::Cancelled));
        if let Ok(mut child) = result {
            child.wait().await.unwrap();
        }
        assert!(env.closing.is_closing());
        assert!(
            cancelled,
            "provider process launched after closing was set between final check and spawn; launch count={}",
            env.launches.load(super::Ordering::Acquire)
        );
    }

    #[test]
    fn launch_in_flight_at_quit_is_signalled_or_refused() {
        use std::{path::Path, time::Duration};
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut escaped = 0;
        let mut launched = 0;
        for _ in 0..20 {
            let mut env = ChildEnv::load();
            env.groups = Default::default();
            env.closing = Default::default();
            let launcher = {
                let env = env.clone();
                let handle = runtime.handle().clone();
                std::thread::spawn(move || {
                    let _entered = handle.enter();
                    let mut children = Vec::new();
                    while let Ok(child) = env.spawn(
                        Path::new("/bin/sleep"),
                        &["5".into()],
                        Path::new("/tmp"),
                        None,
                    ) {
                        env.groups.lock().unwrap().insert(child.id().unwrap());
                        children.push(child);
                    }
                    children
                })
            };
            std::thread::sleep(Duration::from_millis(5));
            env.closing.close();
            let signalled = env.groups.lock().unwrap().clone();
            let mut children = launcher.join().unwrap();
            launched += children.len();
            escaped += children
                .iter()
                .filter(|child| !signalled.contains(&child.id().unwrap()))
                .count();
            runtime.block_on(async {
                for child in &mut children {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            });
        }
        eprintln!("launched={launched} escaped_signal_pass={escaped}");
        assert!(launched > 0, "positive control launched no owned children");
        assert_eq!(
            escaped, 0,
            "launches finished after closing was set, outside the quit signal pass"
        );
    }
    #[tokio::test]
    async fn native_quit_signals_registered_provider_groups() {
        let env = ChildEnv::load();
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&mut db).unwrap();
        let engine = crate::Engine::launch(
            &tokio::runtime::Handle::current(),
            db,
            super::super::Spawner::Cli(env.clone()),
            None,
        )
        .unwrap();
        let mut child = env
            .spawn(
                std::path::Path::new("/bin/sleep"),
                &["5".into()],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let pid = child.id().unwrap();
        assert!(env.groups.lock().unwrap().contains(&pid));
        engine.on_app_quit().await;
        let exited = tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await;
        if exited.is_err() {
            let _ = child.start_kill();
            child.wait().await.unwrap();
        }
        assert_eq!(
            crate::platform::exit_signal(&exited.unwrap().unwrap()),
            Some("SIGTERM")
        );
        env.groups.lock().unwrap().remove(&pid);
        assert!(matches!(
            env.spawn(
                std::path::Path::new("/usr/bin/true"),
                &[],
                std::path::Path::new("/tmp"),
                None
            ),
            Err(super::ProviderError::Cancelled)
        ));
        engine.shutdown_wait().await;
    }
}
