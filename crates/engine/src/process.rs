use crate::platform::{self, ProcessInfo, Signal};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

fn agent_argv(argv: &[String], include_veyra: bool) -> bool {
    let named = |arg: &str| {
        let name = Path::new(arg)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        ["claude", "codex", "grok"].contains(&name) || (include_veyra && name == "veyra")
    };
    let Some(executable) = argv.first() else {
        return false;
    };
    if named(executable) {
        return true;
    }
    let executable = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if !["node", "bun", "sh", "bash", "zsh"].contains(&executable) {
        return false;
    }
    argv.get(1).is_some_and(|script| {
        named(script)
            || (Path::new(script)
                .file_name()
                .is_some_and(|name| name == "cli.js")
                && Path::new(script)
                    .components()
                    .any(|part| part.as_os_str() == "claude-code"))
    })
}
fn candidates(
    rows: &[ProcessInfo],
    include_veyra: bool,
    binaries: &[PathBuf],
    mut executable: impl FnMut(u32) -> Option<PathBuf>,
    mut env: impl FnMut(u32) -> Option<String>,
    alive: impl Fn(u32) -> bool,
) -> BTreeSet<u32> {
    rows.iter()
        .filter(|row| {
            row.pid > 1
                && row.pid == row.pgid
                && row.pid != std::process::id()
                && (agent_argv(&row.argv, include_veyra)
                    || executable(row.pid).is_some_and(|path| binaries.contains(&path)))
        })
        .filter(|row| {
            env(row.pid)
                .and_then(|parent| parent.parse::<u32>().ok())
                .is_some_and(|parent| {
                    parent > 1
                        && parent <= i32::MAX as u32
                        && parent != std::process::id()
                        && !alive(parent)
                })
        })
        .map(|row| row.pgid)
        .collect()
}
fn orphans(rows: &[ProcessInfo], include_veyra: bool, binaries: &[PathBuf]) -> BTreeSet<u32> {
    candidates(
        rows,
        include_veyra,
        binaries,
        |pid| platform::process_executable(pid).ok(),
        |pid| {
            platform::process_env(pid, "VEYRA_PARENT_PID")
                .ok()
                .flatten()
        },
        |pid| platform::is_alive(pid).unwrap_or(true),
    )
}
pub(crate) fn start(stored: Vec<(u32, u32)>, env: &crate::provider::env::ChildEnv) {
    let owned = env.reaper_pids.clone();
    if let Err(error) = start_with(stored, env, move || {
        let rows = platform::list_processes()?;
        let rows = rows
            .into_iter()
            .filter(|row| owned.as_ref().is_none_or(|pids| pids.contains(&row.pid)))
            .collect();
        Ok(rows)
    }) {
        tracing::warn!(%error, "could not start process sweep");
    }
}
fn signal_orphan(group: u32, signal: Signal, reason: &str) {
    match platform::signal_group(group, signal) {
        Ok(()) => tracing::info!(pid = group, group, reason, "reaper signalled orphan group"),
        Err(error) => {
            tracing::warn!(pid = group, group, reason, %error, "orphan group signal failed")
        }
    }
}
fn start_with(
    stored: Vec<(u32, u32)>,
    env: &crate::provider::env::ChildEnv,
    list: impl Fn() -> std::io::Result<Vec<ProcessInfo>> + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    let binaries: Vec<_> = crate::provider::ProviderKind::ALL
        .into_iter()
        .filter_map(|provider| env.resolve(provider).ok())
        .collect();
    let groups = if stored.is_empty() {
        BTreeSet::new()
    } else {
        let rows = list().unwrap_or_else(|error| {
            tracing::warn!(%error, "stored process reaping could not list processes");
            Vec::new()
        });
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|row| stored.contains(&(row.pid, row.pgid)))
            .collect();
        orphans(&rows, false, &binaries)
    };
    for &group in &groups {
        signal_orphan(
            group,
            Signal::Terminate,
            "stored provider has a dead parent",
        );
    }
    std::thread::Builder::new()
        .name("veyra-reaper".into())
        .spawn(move || {
            let mut groups = groups;
            match list() {
                Ok(rows) => {
                    for group in orphans(&rows, true, &binaries) {
                        if groups.insert(group) {
                            signal_orphan(
                                group,
                                Signal::Terminate,
                                "marked process has a dead parent",
                            );
                        }
                    }
                }
                Err(error) => tracing::warn!(%error, "process sweep could not list processes"),
            }
            if groups.is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
            if let Ok(rows) = list() {
                for group in orphans(&rows, true, &binaries).intersection(&groups) {
                    signal_orphan(
                        *group,
                        Signal::Kill,
                        "orphan survived termination with a dead parent",
                    );
                }
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(debug_assertions)]
    #[test]
    fn reap_versioned_provider_from_symlink() {
        use crate::{domain::ThreadId, provider::env::ChildEnv};
        let Some(root) = std::env::var_os("VEYRA_TEST_VERSIONED") else {
            let root = Path::new("/tmp").join(format!("veyra-reap-{}", ThreadId::new()));
            let _scratch = crate::tests::Scratch(root.clone());
            std::fs::create_dir_all(root.join("versions")).unwrap();
            std::fs::write(
                root.join("stand-in.c"),
                "#include <unistd.h>\nint main(void) { sleep(60); return 0; }\n",
            )
            .unwrap();
            assert!(
                std::process::Command::new("/usr/bin/cc")
                    .arg(root.join("stand-in.c"))
                    .arg("-o")
                    .arg(root.join("versions/2.1.296"))
                    .status()
                    .unwrap()
                    .success()
            );
            platform::test_symlink(&root.join("versions/2.1.296"), &root.join("claude")).unwrap();
            let output = tokio::runtime::Runtime::new().unwrap().block_on(async {
                let env = ChildEnv::load();
                let mut parent = env
                    .command(Path::new("/usr/bin/true"), &[], &root)
                    .spawn()
                    .unwrap();
                let dead = parent.id().unwrap();
                parent.wait().await.unwrap();
                let mut command = env.command(&root.join("versions/2.1.296"), &[], &root);
                command.env("VEYRA_PARENT_PID", dead.to_string());
                let mut sentinel = command.spawn().unwrap();
                let pid = sentinel.id().unwrap();
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "process::tests::reap_versioned_provider_from_symlink",
                        "--nocapture",
                    ])
                    .env("VEYRA_TEST_VERSIONED", &root)
                    .env("VEYRA_TEST_UNOWNED_PID", pid.to_string())
                    .env("VEYRA_CLAUDE_BIN", root.join("claude"))
                    .output()
                    .unwrap();
                let survived = sentinel.try_wait().unwrap().is_none();
                if survived {
                    platform::signal_group(pid, Signal::Kill).unwrap();
                }
                sentinel.wait().await.unwrap();
                assert!(
                    survived,
                    "a reaper test signalled a marked orphan started only by its parent harness"
                );
                output
            });
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let env = ChildEnv::load();
            let binary = env.resolve(crate::provider::ProviderKind::Claude).unwrap();
            assert_eq!(binary.file_name().unwrap(), "2.1.296");
            let mut parent = env
                .command(Path::new("/usr/bin/true"), &[], Path::new(&root))
                .spawn()
                .unwrap();
            let dead = parent.id().unwrap();
            parent.wait().await.unwrap();
            let mut owned = Vec::new();
            for marker in [dead, dead, std::process::id()] {
                let mut command = env.command(&binary, &["60".into()], Path::new(&root));
                command.env("VEYRA_PARENT_PID", marker.to_string());
                owned.push(command.spawn().unwrap());
            }
            let pids: Vec<_> = owned.iter().map(|child| child.id().unwrap()).collect();
            tokio::time::sleep(Duration::from_millis(100)).await;
            let rows = platform::list_processes().unwrap();
            assert_eq!(rows.iter().filter(|row| pids.contains(&row.pid)).count(), 3);
            let scoped = pids.clone();
            start_with(vec![(pids[0], pids[0])], &env, move || {
                Ok(platform::list_processes()?
                    .into_iter()
                    .filter(|row| scoped.contains(&row.pid))
                    .collect())
            })
            .unwrap()
            .join()
            .unwrap();
            let unrelated: u32 = std::env::var("VEYRA_TEST_UNOWNED_PID")
                .unwrap()
                .parse()
                .unwrap();
            assert!(
                platform::is_alive(unrelated).unwrap(),
                "the parent harness's marked orphan must survive"
            );
            let exited: Vec<_> = owned
                .iter_mut()
                .map(|child| child.try_wait().unwrap().is_some())
                .collect();
            for (child, pid) in owned.iter_mut().zip(pids) {
                if child.try_wait().unwrap().is_none() {
                    platform::signal_group(pid, Signal::Kill).unwrap();
                }
                child.wait().await.unwrap();
            }
            assert_eq!(
                exited,
                [true, true, false],
                "stored and swept orphans stop; live parent's CLI stays"
            );
        });
    }
    fn row(pid: u32, argv: &[&str]) -> ProcessInfo {
        ProcessInfo {
            pid,
            pgid: pid,
            argv: argv.iter().map(|arg| (*arg).into()).collect(),
        }
    }
    #[test]
    fn reap_skips_live_parent() {
        let rows = [row(100, &["/local/claude", "-p"])];
        assert_eq!(
            candidates(
                &rows,
                false,
                &[],
                |_| None,
                |_| Some("200".into()),
                |_| true
            ),
            BTreeSet::new()
        );
        assert_eq!(
            candidates(
                &rows,
                false,
                &[],
                |_| None,
                |_| Some("200".into()),
                |_| false
            ),
            BTreeSet::from([100])
        );
    }
    #[test]
    fn reap_skips_non_agent_argv() {
        for argv in [
            vec!["tmux", "new"],
            vec!["npm", "start"],
            vec!["node", "/project/typescript/tsserver.js"],
            vec!["echo", "claude"],
        ] {
            let rows = [row(100, &argv), row(101, &["/local/codex"])];
            assert_eq!(
                candidates(
                    &rows,
                    false,
                    &[],
                    |_| None,
                    |_| Some("200".into()),
                    |_| false
                ),
                BTreeSet::from([101])
            );
        }
    }
    #[test]
    fn sweep_reads_env_only_for_agent_argv() {
        let rows = [
            row(100, &["tmux"]),
            row(101, &["claude"]),
            row(102, &["veyra"]),
            row(103, &["node", "/local/claude-code/cli.js"]),
            row(104, &["node", "tsserver.js"]),
        ];
        let mut reads = Vec::new();
        let groups = candidates(
            &rows,
            true,
            &[],
            |_| None,
            |pid| {
                reads.push(pid);
                Some("200".into())
            },
            |_| false,
        );
        assert_eq!(reads, vec![101, 102, 103]);
        assert_eq!(groups, BTreeSet::from([101, 102, 103]));
    }
    #[test]
    fn sweep_inspects_only_group_leaders() {
        let mut nonleader = row(100, &["other"]);
        nonleader.pgid = 99;
        let mut executable_reads = Vec::new();
        let mut env_reads = Vec::new();
        let groups = candidates(
            &[nonleader, row(101, &["other"])],
            true,
            &["/local/provider".into()],
            |pid| {
                executable_reads.push(pid);
                Some("/local/provider".into())
            },
            |pid| {
                env_reads.push(pid);
                Some("200".into())
            },
            |_| false,
        );
        assert_eq!(executable_reads, [101]);
        assert_eq!(env_reads, [101]);
        assert_eq!(groups, BTreeSet::from([101]));
    }
    #[test]
    fn sweep_reads_env_only_for_recognized_executable() {
        let binary = PathBuf::from("/local/versions/2.1.296");
        let recognized = row(100, &["2.1.296"]);
        let unrelated = row(101, &["other"]);
        let mut reads = Vec::new();
        let groups = candidates(
            &[recognized, unrelated],
            true,
            std::slice::from_ref(&binary),
            |pid| {
                Some(if pid == 100 {
                    binary.clone()
                } else {
                    "/local/other".into()
                })
            },
            |pid| {
                reads.push(pid);
                Some("200".into())
            },
            |_| false,
        );
        assert_eq!(reads, [100]);
        assert_eq!(groups, BTreeSet::from([100]));
    }
}
