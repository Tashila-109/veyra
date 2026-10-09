use super::{ProviderError, ProviderKind};
#[cfg(debug_assertions)]
use crate::domain::ThreadId;
use crate::platform;
#[cfg(debug_assertions)]
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
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
    pub home: Option<PathBuf>,
    #[cfg(debug_assertions)]
    lanes: Arc<Mutex<Lanes>>,
    #[cfg(debug_assertions)]
    pub claude_bin: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub fixtures: Option<PathBuf>,
    #[cfg(debug_assertions)]
    pub record: Option<PathBuf>,
}
impl ChildEnv {
    pub fn load() -> Self {
        Self {
            path: LOGIN_PATH.clone(),
            home: std::env::var_os("HOME").map(PathBuf::from),
            #[cfg(debug_assertions)]
            lanes: Arc::default(),
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
                path: "bin::.".into(),
                home: None,
                lanes: Default::default(),
                claude_bin: None,
                fixtures: None,
                record: None,
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
}
