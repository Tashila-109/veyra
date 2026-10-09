use super::{ProviderError, ProviderKind};
#[cfg(debug_assertions)]
use crate::domain::ThreadId;
use crate::platform;
#[cfg(debug_assertions)]
use std::{collections::BTreeMap, sync::Mutex};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
    sync::LazyLock,
};
use tokio::process::{Child, Command};

static LOGIN_PATH: LazyLock<OsString> = LazyLock::new(|| {
    platform::login_shell_path().unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
});

#[derive(Clone, Debug)]
pub struct ChildEnv {
    pub path: OsString,
    #[cfg(debug_assertions)]
    pub claude_bin: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub fixtures: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub record: Option<PathBuf>,
}
impl Default for ChildEnv {
    fn default() -> Self {
        Self::load()
    }
}
impl ChildEnv {
    pub fn load() -> Self {
        Self {
            path: LOGIN_PATH.clone(),
            #[cfg(debug_assertions)]
            claude_bin: std::env::var_os("VEYRA_CLAUDE_BIN").map(PathBuf::from),
            #[cfg(debug_assertions)]
            fixtures: std::env::var_os("VEYRA_FAKE_FIXTURES").map(PathBuf::from),
            #[cfg(debug_assertions)]
            record: std::env::var_os("VEYRA_RECORD").map(PathBuf::from),
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
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(dir)
            } else {
                PathBuf::from(dir)
            }
        });
        std::env::split_paths(&gui)
            .chain(std::env::split_paths(&self.path))
            .chain(known)
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
        Ok(cmd.spawn()?)
    }
}
fn executable(path: &Path) -> Option<PathBuf> {
    platform::is_executable(path)
        .then(|| std::fs::canonicalize(path).ok())
        .flatten()
}

#[cfg(debug_assertions)]
#[derive(Default)]
struct Lanes {
    threads: BTreeMap<ThreadId, (usize, u32)>,
}
#[cfg(debug_assertions)]
static LANES: LazyLock<Mutex<Lanes>> = LazyLock::new(|| Mutex::new(Lanes::default()));
#[cfg(debug_assertions)]
pub(crate) fn register_thread(thread: ThreadId) {
    let mut lanes = LANES.lock().unwrap();
    let next = lanes.threads.len() + 1;
    lanes.threads.entry(thread).or_insert((next, 0));
}
#[cfg(debug_assertions)]
pub(crate) fn next_lane(thread: ThreadId) -> (usize, u32) {
    let mut lanes = LANES.lock().unwrap();
    let next = lanes.threads.len() + 1;
    let entry = lanes.threads.entry(thread).or_insert((next, 0));
    entry.1 += 1;
    *entry
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::{next_lane, register_thread};
    use crate::domain::ThreadId;
    #[test]
    fn recording_lanes_follow_creation_not_spawn_order() {
        let first = ThreadId::new();
        let second = ThreadId::new();
        register_thread(first);
        register_thread(second);
        let second_first = next_lane(second);
        let first_first = next_lane(first);
        assert!(first_first.0 < second_first.0);
        assert_eq!(first_first.1, 1);
        assert_eq!(second_first.1, 1);
        assert_eq!(next_lane(second), (second_first.0, 2));
    }
}
