use super::{ProviderError, ProviderKind};
#[cfg(debug_assertions)]
use crate::domain::ThreadId;
use crate::platform;
#[cfg(debug_assertions)]
use std::collections::BTreeMap;
#[cfg(not(test))]
use std::sync::LazyLock;
#[cfg(test)]
use std::sync::atomic::AtomicBool;
#[cfg(all(test, debug_assertions))]
use std::sync::atomic::Ordering;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, Once},
};
use tokio::process::Command;

pub(crate) type Groups = Arc<Mutex<std::collections::BTreeSet<u32>>>;

#[derive(Clone, Debug)]
struct LoginPath {
    path: OsString,
    mise_diff: Option<OsString>,
}

#[derive(Clone, Debug)]
enum LoginPathState {
    Pending,
    Loaded(LoginPath),
    Fallback(LoginPath),
}
impl LoginPathState {
    fn ready(&self) -> bool {
        !matches!(self, Self::Pending)
    }
}

#[derive(Debug)]
struct LoginPathCell {
    started: Once,
    value: tokio::sync::watch::Sender<LoginPathState>,
    inherited: LoginPath,
    #[cfg(test)]
    shell: Option<OsString>,
}
impl LoginPathCell {
    fn new() -> Self {
        Self {
            started: Once::new(),
            value: tokio::sync::watch::channel(LoginPathState::Pending).0,
            inherited: LoginPath {
                path: std::env::var_os("PATH").unwrap_or_default(),
                mise_diff: std::env::var_os("__MISE_DIFF"),
            },
            #[cfg(test)]
            shell: None,
        }
    }
    fn start(self: &Arc<Self>) {
        self.started.call_once(|| {
            let cell = self.clone();
            let result = std::thread::Builder::new()
                .name("veyra-login-path".into())
                .spawn(move || {
                    #[cfg(test)]
                    let result = if let Some(shell) = &cell.shell {
                        platform::bounded_shell_path(shell, std::time::Duration::from_secs(2))
                    } else {
                        platform::login_shell_path()
                    };
                    #[cfg(not(test))]
                    let result = platform::login_shell_path();
                    let value = match result {
                        Ok((path, mise_diff)) => LoginPathState::Loaded(LoginPath {
                            path,
                            mise_diff: mise_diff.or_else(|| cell.inherited.mise_diff.clone()),
                        }),
                        Err(error) => {
                            tracing::warn!(%error, "login shell PATH unavailable; using inherited PATH");
                            LoginPathState::Fallback(cell.inherited.clone())
                        }
                    };
                    cell.value.send_replace(value);
                });
            if let Err(error) = result {
                tracing::warn!(%error, "login shell thread unavailable; using inherited PATH");
                self.value.send_replace(LoginPathState::Fallback(self.inherited.clone()));
            }
        });
    }
}
#[cfg(not(test))]
static LOGIN_PATH: LazyLock<Arc<LoginPathCell>> = LazyLock::new(|| Arc::new(LoginPathCell::new()));

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct SpawnPause {
    reached: std::sync::mpsc::SyncSender<()>,
    resume: std::sync::mpsc::Receiver<()>,
}

#[derive(Clone, Debug)]
enum Resolution {
    Ambient,
    Explicit(std::collections::BTreeMap<ProviderKind, PathBuf>),
}

#[derive(Clone, Debug)]
pub struct ChildEnv {
    pub(crate) groups: Groups,
    pub mcp: Option<Arc<crate::mcp::Server>>,
    pub relay_command: Option<PathBuf>,
    pub(crate) catalog: Arc<Mutex<std::collections::BTreeMap<ProviderKind, super::Probe>>>,
    pub(crate) closing: Arc<crate::LaunchGate>,
    #[cfg(test)]
    pub(crate) launches: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    pub(crate) before_spawn: Arc<Mutex<Option<SpawnPause>>>,
    #[cfg(test)]
    pub(crate) terminal_controlling_tty: bool,
    #[cfg(test)]
    pub(crate) close_before_user_write: Arc<AtomicBool>,
    resolution: Resolution,
    login_path: Option<Arc<LoginPathCell>>,
    variables: std::collections::BTreeMap<OsString, OsString>,
    pub path: OsString,
    pub home: Option<PathBuf>,
    pub(crate) xai_api_key_set: bool,
    #[cfg(debug_assertions)]
    lanes: Arc<Mutex<Lanes>>,
    #[cfg(debug_assertions)]
    pub claude_bin: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub codex_bin: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub grok_bin: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub fixtures: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub record: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub shell_fixture: Option<PathBuf>,
    pub reaper_pids: Option<std::collections::BTreeSet<u32>>,
}
impl ChildEnv {
    pub fn load() -> Self {
        #[cfg(test)]
        {
            Self::isolated()
        }
        #[cfg(not(test))]
        {
            let mut env = Self::isolated();
            env.resolution = Resolution::Ambient;
            env.login_path = Some(LOGIN_PATH.clone());
            env.reaper_pids = None;
            env.home = std::env::var_os("HOME").map(PathBuf::from);
            if let Some(root) = std::env::var_os("CLAUDE_CONFIG_DIR") {
                env.variables.insert("CLAUDE_CONFIG_DIR".into(), root);
            }
            env.xai_api_key_set = std::env::var_os("XAI_API_KEY").is_some();
            #[cfg(debug_assertions)]
            {
                env.claude_bin = std::env::var_os("VEYRA_CLAUDE_BIN").map(PathBuf::from);
                env.codex_bin = std::env::var_os("VEYRA_CODEX_BIN").map(PathBuf::from);
                env.grok_bin = std::env::var_os("VEYRA_GROK_BIN").map(PathBuf::from);
                env.fixtures = std::env::var_os("VEYRA_FAKE_FIXTURES").map(PathBuf::from);
                env.record = std::env::var_os("VEYRA_RECORD").map(PathBuf::from);
                env.shell_fixture =
                    std::env::var_os("VEYRA_DEBUG_SHELL_FIXTURE").map(PathBuf::from);
                env.reaper_pids = std::env::var("VEYRA_REAP_PIDS")
                    .ok()
                    .map(|text| text.split(',').filter_map(|pid| pid.parse().ok()).collect());
            }
            env
        }
    }
    pub fn isolated() -> Self {
        Self {
            groups: Default::default(),
            mcp: None,
            relay_command: None,
            catalog: Default::default(),
            closing: Default::default(),
            #[cfg(test)]
            launches: Default::default(),
            #[cfg(test)]
            before_spawn: Default::default(),
            #[cfg(test)]
            terminal_controlling_tty: true,
            #[cfg(test)]
            close_before_user_write: Default::default(),
            resolution: Resolution::Explicit(Default::default()),
            login_path: None,
            variables: Default::default(),
            path: std::env::var_os("PATH").unwrap_or_default(),
            home: None,
            xai_api_key_set: false,
            #[cfg(debug_assertions)]
            lanes: Arc::default(),
            #[cfg(debug_assertions)]
            claude_bin: None,
            #[cfg(debug_assertions)]
            codex_bin: None,
            #[cfg(debug_assertions)]
            grok_bin: None,
            #[cfg(debug_assertions)]
            fixtures: None,
            #[cfg(debug_assertions)]
            record: None,
            #[cfg(debug_assertions)]
            shell_fixture: None,
            reaper_pids: Some(Default::default()),
        }
    }
    pub(crate) fn start_login_path(&self) {
        if let Some(cell) = &self.login_path {
            cell.start();
        }
    }
    pub async fn wait_for_login_path(&self) {
        if let Some(cell) = &self.login_path {
            cell.start();
            let mut receiver = cell.value.subscribe();
            receiver
                .wait_for(LoginPathState::ready)
                .await
                .expect("login PATH cell retains its sender");
        }
    }
    fn login_path(&self) -> Option<LoginPath> {
        match &*self.login_path.as_ref()?.value.borrow() {
            LoginPathState::Pending => None,
            LoginPathState::Loaded(value) | LoginPathState::Fallback(value) => Some(value.clone()),
        }
    }
    pub fn login_path_status(&self) -> &'static str {
        match self.login_path.as_ref().map(|cell| cell.value.borrow()) {
            Some(value) => match &*value {
                LoginPathState::Pending => "pending",
                LoginPathState::Loaded(_) => "loaded",
                LoginPathState::Fallback(_) => "fallback",
            },
            None => "pending",
        }
    }
    pub(crate) fn for_probe(&self) -> Self {
        if matches!(self.resolution, Resolution::Explicit(_)) {
            self.clone()
        } else {
            Self::load()
        }
    }
    pub(crate) fn claude_managed_mcp(&self, cwd: &Path) -> PathBuf {
        #[cfg(debug_assertions)]
        if self.fixtures.is_some() {
            return cwd.join("managed-mcp.json");
        }
        match self.resolution {
            Resolution::Ambient => platform::CLAUDE_MANAGED_MCP.into(),
            Resolution::Explicit(_) => cwd.join("managed-mcp.json"),
        }
    }
    pub fn with_binary(mut self, provider: ProviderKind, binary: PathBuf) -> Self {
        if matches!(self.resolution, Resolution::Ambient) {
            self.resolution = Resolution::Explicit(Default::default());
        }
        if let Resolution::Explicit(binaries) = &mut self.resolution {
            binaries.insert(provider, binary);
        }
        self
    }
    pub(crate) fn variable(&self, name: &str) -> Option<&std::ffi::OsStr> {
        self.variables
            .get(std::ffi::OsStr::new(name))
            .map(OsString::as_os_str)
    }
    pub fn with_variable(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.variables.insert(key.into(), value.into());
        self
    }
    pub(crate) fn shell_environment(
        &self,
        parent: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> std::collections::BTreeMap<OsString, OsString> {
        let mut variables: std::collections::BTreeMap<_, _> = parent.into_iter().collect();
        variables.extend(self.variables.clone());
        variables.retain(|key, _| {
            let name = key.to_string_lossy();
            !name.starts_with("VEYRA_")
                && ![
                    "CLAUDECODE",
                    "CLAUDE_CODE_ENTRYPOINT",
                    "CLAUDE_CODE_SSE_PORT",
                    "CLAUDE_AGENT_SDK_VERSION",
                ]
                .contains(&name.as_ref())
                && ![
                    "ITERM_",
                    "GHOSTTY_",
                    "KITTY_",
                    "WEZTERM_",
                    "ALACRITTY_",
                    "VTE_",
                ]
                .iter()
                .any(|prefix| name.starts_with(prefix))
                && ![
                    "TERM_PROGRAM",
                    "TERM_PROGRAM_VERSION",
                    "TERM_SESSION_ID",
                    "NO_COLOR",
                    "FORCE_COLOR",
                    "CLICOLOR_FORCE",
                    "COLORFGBG",
                ]
                .contains(&name.as_ref())
        });
        variables.insert("TERM".into(), "xterm-256color".into());
        variables.insert("COLORTERM".into(), "truecolor".into());
        variables
    }
    fn environment(
        &self,
        parent: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> std::collections::BTreeMap<OsString, OsString> {
        let mut variables: std::collections::BTreeMap<_, _> = parent.into_iter().collect();
        variables.extend(self.variables.clone());
        let login = self.login_path();
        if let Some(login) = &login {
            variables.remove(std::ffi::OsStr::new("__MISE_DIFF"));
            if let Some(mise_diff) = &login.mise_diff {
                variables.insert("__MISE_DIFF".into(), mise_diff.clone());
            }
        }
        variables.retain(|key, _| {
            let name = key.to_string_lossy();
            !name.starts_with("VEYRA_")
                && !(matches!(self.resolution, Resolution::Explicit(_))
                    && [
                        "__MISE_DIFF",
                        "CLAUDE_CODE_AUTO_CONNECT_IDE",
                        "ENABLE_CLAUDEAI_MCP_SERVERS",
                    ]
                    .contains(&name.as_ref())
                    && !self.variables.contains_key(key)
                    && !(name == "__MISE_DIFF" && login.is_some()))
                && ![
                    "CLAUDECODE",
                    "CLAUDE_CODE_ENTRYPOINT",
                    "CLAUDE_CODE_SSE_PORT",
                    "CLAUDE_AGENT_SDK_VERSION",
                ]
                .contains(&name.as_ref())
        });
        if !self.xai_api_key_set {
            variables.remove(std::ffi::OsStr::new("XAI_API_KEY"));
        }
        variables.insert(
            "PATH".into(),
            login.map_or_else(|| self.path.clone(), |login| login.path),
        );
        variables.insert(
            "VEYRA_PARENT_PID".into(),
            std::process::id().to_string().into(),
        );
        variables
    }
    pub fn resolve(&self, provider: ProviderKind) -> Result<PathBuf, ProviderError> {
        #[cfg(debug_assertions)]
        if let Some(path) = match provider {
            ProviderKind::Claude => self.claude_bin.as_ref(),
            ProviderKind::Codex => self.codex_bin.as_ref(),
            ProviderKind::Grok => self.grok_bin.as_ref(),
        } {
            return executable(path)
                .ok_or_else(|| ProviderError::NotInstalled(path.display().to_string()));
        }
        if let Resolution::Explicit(binaries) = &self.resolution {
            return binaries
                .get(&provider)
                .and_then(|path| executable(path))
                .ok_or_else(|| ProviderError::NotInstalled(provider.binary().into()));
        }
        let name = provider.binary();
        let login = self.login_path();
        let path = login.as_ref().map_or(&self.path, |login| &login.path);
        let gui = std::env::var_os("PATH").unwrap_or_default();
        let known = platform::KNOWN_BIN_DIRS.iter().map(|dir| {
            if dir.starts_with('.') {
                self.home.as_deref().unwrap_or(Path::new("")).join(dir)
            } else {
                PathBuf::from(dir)
            }
        });
        resolve_from_dirs(
            name,
            std::env::split_paths(&gui)
                .chain(std::env::split_paths(path))
                .chain(known),
        )
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
        cmd.env_clear().envs(self.environment(std::env::vars_os()));
        platform::own_group(&mut cmd);
        cmd
    }
    pub(crate) fn spawn(
        &self,
        binary: &Path,
        args: &[String],
        cwd: &Path,
        fixture: Option<&Path>,
    ) -> Result<super::session::ProcessGroup, ProviderError> {
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
        let admission = self.closing.enter();
        if self.closing.is_closing() {
            return Err(ProviderError::Cancelled);
        }
        #[cfg(test)]
        if let Some(pause) = self.before_spawn.lock().unwrap().take() {
            pause.reached.send(()).unwrap();
            pause
                .resume
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
        }
        let child = super::session::ProcessGroup::tracked(cmd.spawn()?, self);
        drop(admission);
        #[cfg(test)]
        self.launches
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        if let Err(error) = platform::lower_priority(child.pid()) {
            tracing::warn!(%error, "could not lower agent group priority");
        }
        Ok(child)
    }
}
fn resolve_from_dirs(name: &str, dirs: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    dirs.into_iter()
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| executable(&dir.join(name)))
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
            let selected = super::resolve_from_dirs(
                "claude",
                std::env::split_paths(std::ffi::OsStr::new("bin::.")),
            );
            let planted =
                std::fs::canonicalize(std::path::PathBuf::from(root).join("bin/claude")).unwrap();
            assert_ne!(selected, Some(planted.clone()));
            assert_eq!(
                super::resolve_from_dirs("claude", [planted.parent().unwrap().to_owned()]).unwrap(),
                planted
            );
            return;
        }
        let root = std::env::temp_dir().join(format!("veyra-resolution-{}", ThreadId::new()));
        let _scratch = crate::tests::Scratch(root.clone());
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
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }
    #[test]
    fn grok_known_install_folder_and_isolated_home() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/grok-home-{}", ThreadId::new()));
        let _scratch = crate::tests::Scratch(root.clone());
        std::fs::create_dir_all(root.join(".grok/bin")).unwrap();
        let planted = root.join(".grok/bin/grok");
        std::fs::copy("/bin/echo", &planted).unwrap();
        let resolved = super::resolve_from_dirs(
            "grok",
            crate::platform::KNOWN_BIN_DIRS
                .iter()
                .filter(|dir| dir.starts_with('.'))
                .map(|dir| root.join(dir)),
        );
        assert_eq!(resolved, Some(std::fs::canonicalize(&planted).unwrap()));
        let mut isolated = ChildEnv::isolated();
        isolated.home = Some(root);
        assert!(matches!(
            isolated.resolve(super::ProviderKind::Grok),
            Err(super::ProviderError::NotInstalled(_))
        ));
        let explicit = isolated.with_binary(super::ProviderKind::Grok, planted.clone());
        assert_eq!(
            explicit.resolve(super::ProviderKind::Grok).unwrap(),
            std::fs::canonicalize(planted).unwrap()
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
    async fn dropping_spawn_result_reaps_and_unregisters_its_group() {
        use std::{path::Path, time::Duration};
        let env = ChildEnv::load();
        let child = env
            .spawn(
                Path::new("/bin/sleep"),
                &["5".into()],
                Path::new("/tmp"),
                None,
            )
            .unwrap();
        let pid = child.pid();
        assert!(env.groups.lock().unwrap().contains(&pid));
        drop(child);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while env.groups.lock().unwrap().contains(&pid) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let stale = env.groups.lock().unwrap().contains(&pid);
        if crate::platform::group_exists(pid).unwrap() {
            let _ = crate::platform::signal_group(pid, crate::platform::Signal::Kill);
        }
        env.groups.lock().unwrap().remove(&pid);
        assert!(
            !stale,
            "dropping the spawn result left an entry for a dead owned group"
        );
        assert!(!crate::platform::group_exists(pid).unwrap());
    }

    #[tokio::test]
    async fn closing_state_refuses_every_provider_spawn() {
        let env = ChildEnv::load();
        let cwd = std::path::Path::new("/tmp");
        let binary = std::path::Path::new("/usr/bin/true");
        let mut positive = env.spawn(binary, &[], cwd, None).unwrap();
        positive.wait().await.unwrap();
        positive.stop().await.unwrap();
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
    #[test]
    fn launch_past_final_check_at_quit_is_registered_and_signalled() {
        use std::{path::Path, time::Duration};
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let env = ChildEnv::load();
        let mut db = rusqlite::Connection::open_in_memory().unwrap();
        crate::store::initialize(&mut db).unwrap();
        let engine = crate::Engine::launch(
            runtime.handle(),
            db,
            super::super::Spawner::Cli(Box::new(env.clone())),
            None,
        )
        .unwrap();
        let (reached, paused) = std::sync::mpsc::sync_channel(0);
        let (resume, resumed) = std::sync::mpsc::sync_channel(0);
        *env.before_spawn.lock().unwrap() = Some(super::SpawnPause {
            reached,
            resume: resumed,
        });
        let launcher = {
            let env = env.clone();
            let handle = runtime.handle().clone();
            std::thread::spawn(move || {
                let _entered = handle.enter();
                env.spawn(
                    Path::new("/bin/sleep"),
                    &["5".into()],
                    Path::new("/tmp"),
                    None,
                )
                .unwrap()
            })
        };
        paused.recv_timeout(Duration::from_secs(2)).unwrap();
        let (started, starting) = std::sync::mpsc::sync_channel(0);
        let (finished, done) = std::sync::mpsc::sync_channel(1);
        let quit = {
            let handle = runtime.handle().clone();
            let engine = engine.clone();
            std::thread::spawn(move || {
                started.send(()).unwrap();
                handle.block_on(engine.on_app_quit());
                finished
                    .send(engine.inner.groups.lock().unwrap().clone())
                    .unwrap();
            })
        };
        starting.recv_timeout(Duration::from_secs(2)).unwrap();
        let signalled = done.recv_timeout(Duration::from_millis(500)).ok();
        resume.send(()).unwrap();
        let mut child = launcher.join().unwrap();
        let pid = child.pid();
        let signalled =
            signalled.unwrap_or_else(|| done.recv_timeout(Duration::from_secs(2)).unwrap());
        quit.join().unwrap();
        let status = runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(1), child.wait()).await });
        runtime.block_on(child.stop()).unwrap();
        runtime.block_on(engine.shutdown_wait());
        assert!(env.groups.lock().unwrap().is_empty());
        assert!(
            signalled.contains(&pid),
            "a launch committed after quit's group signal pass"
        );
        assert_eq!(
            crate::platform::exit_signal(&status.unwrap().unwrap()),
            Some("SIGTERM")
        );
        assert!(!crate::platform::group_exists(pid).unwrap());
        let _entered = runtime.enter();
        assert!(matches!(
            env.spawn(Path::new("/usr/bin/true"), &[], Path::new("/tmp"), None),
            Err(super::ProviderError::Cancelled)
        ));
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
                        children.push(child);
                    }
                    children
                })
            };
            std::thread::sleep(Duration::from_millis(5));
            env.closing.close();
            let signalled = env.groups.lock().unwrap().clone();
            let children = launcher.join().unwrap();
            launched += children.len();
            escaped += children
                .iter()
                .filter(|child| !signalled.contains(&child.pid()))
                .count();
            runtime.block_on(async {
                for child in children {
                    let _ = child.stop().await;
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
            super::super::Spawner::Cli(Box::new(env.clone())),
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
        let pid = child.pid();
        assert!(env.groups.lock().unwrap().contains(&pid));
        engine.on_app_quit().await;
        let exited = tokio::time::timeout(std::time::Duration::from_secs(1), child.wait()).await;
        child.stop().await.unwrap();
        assert_eq!(
            crate::platform::exit_signal(&exited.unwrap().unwrap()),
            Some("SIGTERM")
        );
        assert!(env.groups.lock().unwrap().is_empty());
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

#[cfg(test)]
mod hermetic_tests {
    use super::*;
    #[test]
    fn test_environment_never_resolves_ambient_providers() {
        if std::env::var_os("VEYRA_TEST_HERMETIC_CHILD").is_some() {
            let env = ChildEnv::load();
            for provider in ProviderKind::ALL {
                assert!(
                    matches!(env.resolve(provider), Err(ProviderError::NotInstalled(_))),
                    "test environment resolved ambient {}",
                    provider.binary()
                );
            }
            return;
        }
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/provider-ambient-{}",
            crate::domain::ThreadId::new()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        for name in ["claude", "codex", "grok"] {
            std::fs::copy(std::env::current_exe().unwrap(), root.join(name)).unwrap();
        }
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider::env::hermetic_tests::test_environment_never_resolves_ambient_providers",
                "--nocapture",
            ])
            .env("VEYRA_TEST_HERMETIC_CHILD", "1")
            .env("PATH", &root)
            .env_remove("VEYRA_CLAUDE_BIN")
            .env_remove("VEYRA_CODEX_BIN")
            .env_remove("VEYRA_GROK_BIN")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[cfg(test)]
#[test]
fn isolated_resolution_uses_only_explicit_binaries_in_every_build() {
    let binary = std::env::current_exe().unwrap().canonicalize().unwrap();
    let mut env = ChildEnv::isolated();
    for provider in ProviderKind::ALL {
        assert!(matches!(
            env.resolve(provider),
            Err(ProviderError::NotInstalled(_))
        ));
        env = env.with_binary(provider, binary.clone());
        assert_eq!(env.resolve(provider).unwrap(), binary);
    }
}

#[cfg(test)]
#[test]
fn selecting_an_explicit_binary_disables_ambient_resolution() {
    let mut env = ChildEnv::isolated();
    env.resolution = Resolution::Ambient;
    let binary = std::env::current_exe().unwrap().canonicalize().unwrap();
    let env = env.with_binary(ProviderKind::Codex, binary.clone());
    assert_eq!(env.resolve(ProviderKind::Codex).unwrap(), binary);
    assert!(matches!(
        env.resolve(ProviderKind::Claude),
        Err(ProviderError::NotInstalled(_))
    ));
}

#[cfg(test)]
#[test]
fn shell_env_keeps_login_inputs_and_strips_host() {
    let stripped = [
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "TERM_SESSION_ID",
        "ITERM_SESSION_ID",
        "GHOSTTY_RESOURCES_DIR",
        "KITTY_WINDOW_ID",
        "WEZTERM_PANE",
        "ALACRITTY_SOCKET",
        "VTE_VERSION",
        "NO_COLOR",
        "FORCE_COLOR",
        "CLICOLOR_FORCE",
        "COLORFGBG",
        "VEYRA_MCP_TOKEN",
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SSE_PORT",
        "CLAUDE_AGENT_SDK_VERSION",
    ];
    let mut parent: Vec<(OsString, OsString)> = stripped
        .iter()
        .map(|key| ((*key).into(), "host-value".into()))
        .collect();
    parent.extend([
        ("TERM".into(), "wrong-terminal".into()),
        ("COLORTERM".into(), "wrong-color".into()),
        ("KEEP_ME".into(), "kept".into()),
    ]);
    let env = ChildEnv::isolated().with_variable("CLAUDE_CONFIG_DIR", "/tmp/owned-claude-config");
    let actual = env.shell_environment(parent);
    assert!(!actual.contains_key(std::ffi::OsStr::new("VEYRA_PARENT_PID")));
    assert_eq!(
        actual.get(std::ffi::OsStr::new("TERM")),
        Some(&OsString::from("xterm-256color"))
    );
    assert_eq!(
        actual.get(std::ffi::OsStr::new("COLORTERM")),
        Some(&OsString::from("truecolor"))
    );
    assert_eq!(
        actual.get(std::ffi::OsStr::new("KEEP_ME")),
        Some(&OsString::from("kept"))
    );
    assert_eq!(
        actual.get(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR")),
        Some(&OsString::from("/tmp/owned-claude-config"))
    );
    for key in stripped {
        assert!(
            !actual.contains_key(std::ffi::OsStr::new(key)),
            "inherited {key}"
        );
    }
}

#[cfg(all(test, debug_assertions))]
#[test]
fn claude_probe_replay_uses_owned_managed_policy_path() {
    let mut env = ChildEnv::isolated();
    env.resolution = Resolution::Ambient;
    env.fixtures = Some("/owned/fixtures".into());
    assert_eq!(
        env.claude_managed_mcp(Path::new("/owned/probe")),
        PathBuf::from("/owned/probe/managed-mcp.json")
    );
}

#[cfg(test)]
mod login_path_tests {
    use super::*;
    use crate::domain::{PermissionMode, ThreadConfig, ThreadId};
    use std::time::{Duration, Instant};

    fn delayed_environment(failing: bool) -> (crate::tests::Scratch, ChildEnv) {
        let root = PathBuf::from("/tmp").join(format!("veyra-path-{}", ThreadId::new()));
        std::fs::create_dir(&root).unwrap();
        let scratch = crate::tests::Scratch(root.clone());
        let shell = root.join("shell");
        let binary = root.join("provider");
        let script = if failing {
            "#!/bin/sh\n/bin/sleep 1.5\nexit 7\n".to_owned()
        } else {
            format!(
                "#!/bin/sh\n/bin/sleep 1.5\n/bin/ln -s /bin/cat '{}'\nprintf '%s\\0%s' /loaded-path loaded-diff\n",
                binary.display()
            )
        };
        let script = script.replacen(
            "#!/bin/sh\n",
            "#!/bin/sh\nif [ \"$1\" = --warmup ]; then exit 0; fi\nprintf x >> \"$0.runs\"\n",
            1,
        );
        std::fs::write(&shell, script).unwrap();
        assert!(
            std::process::Command::new("/bin/chmod")
                .arg("700")
                .arg(&shell)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new(&shell)
                .arg("--warmup")
                .status()
                .unwrap()
                .success()
        );
        let mut cell = LoginPathCell::new();
        cell.shell = Some(shell.into_os_string());
        cell.inherited.path = "/gui-path".into();
        cell.inherited.mise_diff = Some("gui-diff".into());
        let mut env = ChildEnv::isolated();
        for provider in ProviderKind::ALL {
            env = env.with_binary(provider, binary.clone());
        }
        env.path = "/gui-path".into();
        env.login_path = Some(Arc::new(cell));
        (scratch, env)
    }

    #[tokio::test]
    async fn launch_does_not_wait_for_login_shell() {
        let (scratch, env) = delayed_environment(false);
        let status = env.clone();
        let data = crate::data_dir::DataDir::at(scratch.0.join("data")).unwrap();
        let start = Instant::now();
        let engine =
            crate::Engine::start_with_env(&tokio::runtime::Handle::current(), data, env).unwrap();
        assert!(start.elapsed() < Duration::from_millis(500));
        assert_eq!(status.login_path_status(), "pending");
        assert_eq!(engine.child_env().login_path_status(), "pending");
        status.wait_for_login_path().await;
        let loaded = &status;
        assert!(start.elapsed() >= Duration::from_millis(1400));
        assert_eq!(
            loaded.login_path().unwrap().path,
            OsString::from("/loaded-path")
        );
        assert_eq!(loaded.login_path_status(), "loaded");
        tokio::time::timeout(Duration::from_millis(100), async {
            for _ in 0..8 {
                status.wait_for_login_path().await;
            }
        })
        .await
        .expect("already loaded PATH must not wait for another change");
        assert_eq!(std::fs::read(scratch.0.join("shell.runs")).unwrap(), b"x");
        engine.shutdown_wait().await;
    }

    #[tokio::test]
    async fn session_path_wait_honours_stop_and_quit() {
        for quit in [false, true] {
            let (_scratch, env) = delayed_environment(false);
            let (out, mut events) = tokio::sync::mpsc::channel(256);
            let handle = super::super::session::spawn(
                super::super::SessionSpec {
                    thread: ThreadId::new(),
                    cwd: std::env::temp_dir(),
                    config: ThreadConfig {
                        mode: PermissionMode::AskFirst,
                        model: None,
                        effort: None,
                    },
                    native: None,
                    root: false,
                },
                env.clone(),
                out,
                1,
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
            if quit {
                env.closing.close();
            } else {
                handle
                    .control
                    .send(super::super::SessionControl::Shutdown)
                    .unwrap();
            }
            tokio::time::timeout(Duration::from_millis(300), async {
                while let Some(crate::actor::EngineMsg::Provider { event, .. }) =
                    events.recv().await
                {
                    if matches!(
                        event,
                        super::super::ProviderEvent::Session(super::super::SessionEvent::Exited {
                            crash: None
                        })
                    ) {
                        return;
                    }
                }
                panic!("PATH cancellation emitted no exit");
            })
            .await
            .expect("Stop and quit must not wait for the login shell");
            assert_eq!(env.launches.load(std::sync::atomic::Ordering::Acquire), 0);
            env.wait_for_login_path().await;
        }
    }

    #[tokio::test]
    async fn orphan_reaper_waits_for_login_path() {
        if std::env::var_os("VEYRA_TEST_ORPHAN_REAPER").is_some() {
            std::thread::sleep(Duration::from_secs(60));
            return;
        }
        struct Owned(std::process::Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let (scratch, mut env) = delayed_environment(false);
        let mut marker = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let dead_parent = marker.id();
        marker.wait().unwrap();
        let binary = std::env::current_exe().unwrap();
        let shell = env.login_path.as_ref().unwrap().shell.as_ref().unwrap();
        std::fs::write(shell, format!(
            "#!/bin/sh\n/bin/sleep 1.5\n/bin/ln -s '{}' '{}'\nprintf '%s\\0%s' /loaded-path loaded-diff\n",
            binary.display(), scratch.0.join("provider").display())).unwrap();
        let mut command = env.command(
            &binary,
            &[
                "--exact".into(),
                "provider::env::login_path_tests::orphan_reaper_waits_for_login_path".into(),
            ],
            &scratch.0,
        );
        command.env("VEYRA_TEST_ORPHAN_REAPER", "1");
        command.env("VEYRA_PARENT_PID", dead_parent.to_string());
        let mut child = Owned(command.as_std_mut().spawn().unwrap());
        let pid = child.0.id();
        env.reaper_pids = Some([pid].into_iter().collect());
        let data = crate::data_dir::DataDir::at(scratch.0.join("data")).unwrap();
        let engine =
            crate::Engine::start_with_env(&tokio::runtime::Handle::current(), data, env.clone())
                .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(child.0.try_wait().unwrap().is_none());
        tokio::time::timeout(Duration::from_secs(5), async {
            while child.0.try_wait().unwrap().is_none() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("reaper never resolved the binary supplied after PATH loaded");
        assert_eq!(env.login_path_status(), "loaded");
        engine.shutdown_wait().await;
    }

    #[tokio::test]
    async fn spawn_waits_for_login_path() {
        let (_scratch, env) = delayed_environment(false);
        let (out, mut events) = tokio::sync::mpsc::channel(256);
        let start = Instant::now();
        let handle = super::super::session::spawn(
            super::super::SessionSpec {
                thread: ThreadId::new(),
                cwd: std::env::temp_dir(),
                config: ThreadConfig {
                    mode: PermissionMode::AskFirst,
                    model: None,
                    effort: None,
                },
                native: None,

                root: false,
            },
            env.clone(),
            out,
            1,
        );
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(env.launches.load(std::sync::atomic::Ordering::Acquire), 0);
        assert!(events.try_recv().is_err());
        env.wait_for_login_path().await;
        let loaded = &env;
        assert_eq!(
            loaded.login_path().unwrap().path,
            OsString::from("/loaded-path")
        );
        assert!(loaded.resolve(ProviderKind::Claude).is_ok());
        tokio::time::timeout(Duration::from_secs(4), async {
            while env.launches.load(std::sync::atomic::Ordering::Acquire) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(1400));
        assert_eq!(env.login_path_status(), "loaded");
        let _ = handle.control.send(super::super::SessionControl::Shutdown);
        tokio::time::timeout(Duration::from_secs(4), async {
            while let Some(crate::actor::EngineMsg::Provider { event, .. }) = events.recv().await {
                if matches!(
                    event,
                    super::super::ProviderEvent::Session(super::super::SessionEvent::Exited { .. })
                ) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(env.groups.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn probe_waits_for_login_path() {
        let (_scratch, env) = delayed_environment(false);
        let mut tasks = Vec::new();
        let mut cancels = Vec::new();
        let start = Instant::now();
        for provider in ProviderKind::ALL {
            let (cancel, cancelled) = tokio::sync::oneshot::channel();
            cancels.push(cancel);
            let env = env.clone();
            tasks.push(tokio::spawn(async move {
                let cwd = std::env::temp_dir();
                match provider {
                    ProviderKind::Claude => {
                        super::super::session::probe(&env, &cwd, cancelled).await
                    }
                    ProviderKind::Codex => {
                        super::super::session::probe_codex(&env, &cwd, cancelled).await
                    }
                    ProviderKind::Grok => {
                        super::super::session::probe_grok(&env, &cwd, cancelled).await
                    }
                }
            }));
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(tasks.iter().all(|task| !task.is_finished()));
        assert_eq!(env.launches.load(std::sync::atomic::Ordering::Acquire), 0);
        env.wait_for_login_path().await;
        let loaded = &env;
        assert_eq!(
            loaded.login_path().unwrap().path,
            OsString::from("/loaded-path")
        );
        assert!(loaded.resolve(ProviderKind::Claude).is_ok());
        tokio::time::timeout(Duration::from_secs(4), async {
            while env.launches.load(std::sync::atomic::Ordering::Acquire) < 3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(1400));
        for cancel in cancels {
            let _ = cancel.send(());
        }
        for task in tasks {
            assert!(
                tokio::time::timeout(Duration::from_secs(4), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
        }
        assert!(env.groups.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_login_shell_fills_gui_path() {
        for timeout in [false, true] {
            let (_scratch, env) = delayed_environment(true);
            if timeout {
                let shell = env.login_path.as_ref().unwrap().shell.as_ref().unwrap();
                std::fs::write(
                    shell,
                    "#!/bin/sh\nif [ \"$1\" = --warmup ]; then exit 0; fi\n\
                     /bin/sleep 1.5\n/bin/sleep 1.5\n",
                )
                .unwrap();
                assert!(
                    std::process::Command::new(shell)
                        .arg("--warmup")
                        .status()
                        .unwrap()
                        .success()
                );
            }
            let start = Instant::now();
            env.wait_for_login_path().await;
            let loaded = &env;
            assert!(start.elapsed() >= Duration::from_millis(1400));
            assert_eq!(loaded.login_path_status(), "fallback");
            let variables = loaded.environment([
                ("PATH".into(), "/wrong-path".into()),
                ("__MISE_DIFF".into(), "wrong-diff".into()),
            ]);
            assert_eq!(
                variables.get(std::ffi::OsStr::new("PATH")),
                Some(&OsString::from("/gui-path"))
            );
            assert_eq!(
                variables.get(std::ffi::OsStr::new("__MISE_DIFF")),
                Some(&OsString::from("gui-diff"))
            );
        }
    }
}
