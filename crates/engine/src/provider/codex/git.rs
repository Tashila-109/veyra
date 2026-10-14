use super::super::env::ChildEnv;
use crate::platform::{self, Signal};
use std::time::Duration;
use std::{
    fs,
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
};
use tokio::sync::{mpsc, oneshot};

struct LinkedWorktree {
    git_dir: PathBuf,
    common_dir: PathBuf,
}
fn path_line(path: &Path) -> Result<String, &'static str> {
    if !fs::symlink_metadata(path)
        .map_err(|_| "git back-link is unavailable")?
        .is_file()
    {
        return Err("git back-link is not a regular file");
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "git back-link could not be opened")?
        .take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| "git back-link could not be read")?;
    if bytes.len() > 8192 {
        return Err("git back-link exceeded 8 KiB");
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| "git back-link is not UTF-8")?;
    let mut lines = text.lines();
    let line = lines
        .next()
        .filter(|line| !line.is_empty())
        .ok_or("git back-link is empty")?;
    if lines.next().is_some() {
        return Err("git back-link has multiple lines");
    }
    Ok(line.into())
}
impl LinkedWorktree {
    fn checked(project: &Path, git_dir: &Path, common_dir: &Path) -> Result<Self, &'static str> {
        let project = project
            .canonicalize()
            .map_err(|_| "project folder is unavailable")?;
        if common_dir.file_name().is_none_or(|name| name != ".git") {
            return Err("common git metadata is not a .git folder");
        }
        if git_dir == common_dir {
            return Err("plain repository metadata stays protected");
        }
        let worktrees = common_dir.join("worktrees");
        if worktrees
            .canonicalize()
            .map_err(|_| "git worktrees folder is unavailable")?
            != worktrees
            || git_dir.parent() != Some(worktrees.as_path())
        {
            return Err("git metadata is not a direct linked-worktree child");
        }
        let common = path_line(&git_dir.join("commondir"))?;
        if git_dir
            .join(common)
            .canonicalize()
            .map_err(|_| "common git back-link is unavailable")?
            != common_dir
        {
            return Err("common git back-link changed");
        }
        let backlink = path_line(&git_dir.join("gitdir"))?;
        let git_file = git_dir
            .join(backlink)
            .canonicalize()
            .map_err(|_| "worktree git file is unavailable")?;
        if git_file.file_name().is_none_or(|name| name != ".git") {
            return Err("worktree back-link does not name a .git file");
        }
        let root = git_file.parent().ok_or("worktree folder is unavailable")?;
        if !project.ancestors().any(|ancestor| ancestor == root) {
            return Err("worktree back-link names another project");
        }
        let pointer = path_line(&git_file)?;
        let directory = pointer
            .strip_prefix("gitdir: ")
            .filter(|path| !path.is_empty())
            .ok_or("worktree git pointer is invalid")?;
        if root
            .join(directory)
            .canonicalize()
            .map_err(|_| "worktree git pointer is unavailable")?
            != git_dir
        {
            return Err("worktree git pointer does not point back");
        }
        Ok(Self {
            git_dir: git_dir.into(),
            common_dir: common_dir.into(),
        })
    }
    fn roots(self) -> Result<Vec<PathBuf>, &'static str> {
        let mut roots = vec![self.git_dir];
        for name in ["objects", "refs", "logs"] {
            let path = self.common_dir.join(name);
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(_) => return Err("common git storage is unavailable"),
                Ok(metadata) if !metadata.is_dir() => {
                    return Err("common git storage is not a regular directory");
                }
                Ok(_) => {}
            }
            if path
                .canonicalize()
                .map_err(|_| "common git storage is unavailable")?
                != path
            {
                return Err("common git storage redirects outside its folder");
            }
            roots.push(path);
        }
        Ok(roots)
    }
}
pub(super) fn worktree_roots(
    project: &Path,
    git_dir: &Path,
    common_dir: &Path,
) -> Result<Vec<PathBuf>, &'static str> {
    LinkedWorktree::checked(project, git_dir, common_dir)?.roots()
}

pub(crate) async fn discover(
    env: &ChildEnv,
    cwd: &std::path::Path,
    cleanup: &mpsc::UnboundedSender<oneshot::Receiver<()>>,
) -> Vec<std::path::PathBuf> {
    use tokio::io::AsyncReadExt;
    let result = async {
        let binary = env.resolve_git().ok_or("git is not available")?;
        let mut child = env
            .spawn(
                &binary,
                &[
                    "rev-parse".into(),
                    "--git-dir".into(),
                    "--git-common-dir".into(),
                ],
                cwd,
                None,
            )
            .map_err(|_| "git could not start")?;
        child.watch(cleanup);
        drop(child.stdin());
        let stdout = child.stdout().ok_or("git stdout is unavailable")?;
        let result = tokio::time::timeout(Duration::from_secs(1), async {
            let mut bytes = Vec::new();
            stdout
                .take(8193)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| "git output could not be read")?;
            let status = child
                .wait()
                .await
                .map_err(|_| "git could not be waited for")?;
            if !status.success() {
                return Err("git did not find repository metadata");
            }
            if bytes.len() > 8192 {
                return Err("git output exceeded 8 KiB");
            }
            let text = std::str::from_utf8(&bytes).map_err(|_| "git output is not UTF-8")?;
            if text.lines().any(str::is_empty) {
                return Err("git returned an empty metadata path");
            }
            let paths = text
                .lines()
                .map(|path| {
                    cwd.join(path)
                        .canonicalize()
                        .map_err(|_| "git metadata path is unavailable")
                })
                .collect::<Result<Vec<_>, _>>()?;
            if paths.len() != 2 || paths.iter().any(|p| !p.is_dir()) {
                return Err("git did not return two metadata folders");
            }
            worktree_roots(cwd, &paths[0], &paths[1])
        })
        .await;
        if result.is_err() {
            let _ = platform::signal_group(child.pid(), Signal::Kill);
        }
        child.stop().await.map_err(|_| "git cleanup failed")?;
        result.map_err(|_| "git metadata check exceeded one second")?
    }
    .await;
    match result {
        Ok(roots) => roots,
        Err(reason) => {
            tracing::info!(
                reason,
                "Codex Git metadata is unavailable; no writable roots added"
            );
            Vec::new()
        }
    }
}
