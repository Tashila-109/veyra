use crate::platform::{self, ProcessInfo, Signal};
use std::{collections::BTreeSet, path::Path, time::Duration};

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
    mut env: impl FnMut(u32) -> Option<String>,
    alive: impl Fn(u32) -> bool,
) -> BTreeSet<u32> {
    rows.iter()
        .filter(|row| {
            row.pid > 1
                && row.pid == row.pgid
                && row.pid != std::process::id()
                && agent_argv(&row.argv, include_veyra)
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
fn orphans(rows: &[ProcessInfo], include_veyra: bool) -> BTreeSet<u32> {
    candidates(
        rows,
        include_veyra,
        |pid| {
            platform::process_env(pid, "VEYRA_PARENT_PID")
                .ok()
                .flatten()
        },
        |pid| platform::is_alive(pid).unwrap_or(true),
    )
}
pub(crate) fn start(stored: Vec<(u32, u32)>) {
    let groups = if stored.is_empty() {
        BTreeSet::new()
    } else {
        let rows = match platform::list_processes() {
            Ok(rows) => rows,
            Err(error) => {
                tracing::warn!(%error, "stored process reaping could not list processes");
                Vec::new()
            }
        };
        let rows: Vec<_> = rows
            .into_iter()
            .filter(|row| stored.contains(&(row.pid, row.pgid)))
            .collect();
        orphans(&rows, false)
    };
    for &group in &groups {
        let _ = platform::signal_group(group, Signal::Terminate);
    }
    if let Err(error) = std::thread::Builder::new()
        .name("veyra-reaper".into())
        .spawn(move || {
            let mut groups = groups;
            match platform::list_processes() {
                Ok(rows) => {
                    for group in orphans(&rows, true) {
                        if groups.insert(group) {
                            let _ = platform::signal_group(group, Signal::Terminate);
                        }
                    }
                }
                Err(error) => tracing::warn!(%error, "process sweep could not list processes"),
            }
            if groups.is_empty() {
                return;
            }
            std::thread::sleep(Duration::from_secs(1));
            if let Ok(rows) = platform::list_processes() {
                for group in orphans(&rows, true).intersection(&groups) {
                    let _ = platform::signal_group(*group, Signal::Kill);
                }
            }
        })
    {
        tracing::warn!(%error, "could not start process sweep");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            candidates(&rows, false, |_| Some("200".into()), |_| true),
            BTreeSet::new()
        );
        assert_eq!(
            candidates(&rows, false, |_| Some("200".into()), |_| false),
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
                candidates(&rows, false, |_| Some("200".into()), |_| false),
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
            |pid| {
                reads.push(pid);
                Some("200".into())
            },
            |_| false,
        );
        assert_eq!(reads, vec![101, 102, 103]);
        assert_eq!(groups, BTreeSet::from([101, 102, 103]));
    }
}
