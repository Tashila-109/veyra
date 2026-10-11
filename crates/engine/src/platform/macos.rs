use std::{
    env,
    fs::{self, DirBuilder, File, OpenOptions},
    io,
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::Command,
};

pub const SOCKET_PATH_MAX: usize = 103;
pub const KNOWN_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    ".local/bin",
    ".claude/local",
];

pub fn login_shell_path() -> io::Result<std::ffi::OsString> {
    let shell = env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    bounded_shell_path(&shell, std::time::Duration::from_secs(2))
}
fn bounded_shell_path(
    shell: &std::ffi::OsStr,
    bound: std::time::Duration,
) -> io::Result<std::ffi::OsString> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut child = Command::new(shell)
        .args(["-l", "-c", "printf '%s' \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    let pid = child.id();
    let mut stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(error) => break Err(error),
            Ok(None) => {}
        }
        if start.elapsed() >= bound {
            break Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "login shell PATH timed out",
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let _ = signal_group(pid, super::Signal::Kill);
    let _ = child.wait();
    let output = reader
        .join()
        .map_err(|_| io::Error::other("login shell reader failed"))??;
    if !status?.success() {
        return Err(io::Error::other("login shell could not read PATH"));
    }
    Ok(String::from_utf8_lossy(&output).trim().into())
}
pub fn exit_signal(status: &std::process::ExitStatus) -> Option<&'static str> {
    use std::os::unix::process::ExitStatusExt;
    match status.signal()? {
        libc::SIGHUP => Some("SIGHUP"),
        libc::SIGINT => Some("SIGINT"),
        libc::SIGTERM => Some("SIGTERM"),
        libc::SIGKILL => Some("SIGKILL"),
        _ => Some("other signal"),
    }
}
pub fn ignore_terminate() {
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }
}

pub fn own_group(cmd: &mut tokio::process::Command) {
    use std::os::unix::process::CommandExt;
    cmd.as_std_mut().process_group(0);
}

pub fn signal_group(group: u32, signal: super::Signal) -> io::Result<()> {
    let signal = match signal {
        super::Signal::Hangup => libc::SIGHUP,
        super::Signal::Terminate => libc::SIGTERM,
        super::Signal::Kill => libc::SIGKILL,
    };
    if unsafe { libc::killpg(group as i32, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn group_exists(group: u32) -> io::Result<bool> {
    if unsafe { libc::killpg(group as i32, 0) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error),
    }
}

pub fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

pub fn open_terminal_app(dir: &Path) -> io::Result<()> {
    let status = Command::new("open")
        .args(["-a", "Terminal"])
        .arg(dir)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other("Terminal could not open the project"))
    }
}

pub fn default_data_dir() -> io::Result<PathBuf> {
    let home = env::var_os("HOME")
        .ok_or_else(|| io::Error::other("HOME is not set; set VEYRA_DATA_DIR"))?;
    Ok(PathBuf::from(home).join("Library/Application Support/Veyra"))
}

pub fn lock_exclusive(file: &File) -> io::Result<()> {
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

pub fn private_dir(path: &Path) -> io::Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

pub fn private_file(path: &Path, options: &OpenOptions) -> io::Result<File> {
    let file = options.clone().mode(0o600).open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

pub fn is_private(path: &Path) -> io::Result<bool> {
    let metadata = fs::metadata(path)?;
    let expected = if metadata.is_dir() { 0o700 } else { 0o600 };
    Ok(metadata.permissions().mode() & 0o777 == expected)
}

pub fn bind_socket(path: &Path) -> io::Result<UnixListener> {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = UnixListener::bind(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

pub fn connect_socket(path: &Path) -> io::Result<UnixStream> {
    UnixStream::connect(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn login_shell_bounded_at_2s() {
        if env::var_os("VEYRA_SHELL_TIMEOUT_CHILD").is_some() {
            assert!(login_shell_path().is_err());
            return;
        }
        let root = env::temp_dir().join(format!("veyra-shell-{}", crate::domain::ThreadId::new()));
        fs::create_dir(&root).unwrap();
        let shell = root.join("shell");
        fs::write(
            &shell,
            "#!/bin/sh\nprintf '%s' \"$$\" > pid\n/bin/sleep 10 & wait\n",
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        let start = std::time::Instant::now();
        let result = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::macos::tests::login_shell_bounded_at_2s",
            ])
            .env("SHELL", &shell)
            .env("VEYRA_SHELL_TIMEOUT_CHILD", "1")
            .current_dir(&root)
            .output()
            .unwrap();
        let elapsed = start.elapsed();
        let pid: u32 = fs::read_to_string(root.join("pid"))
            .unwrap()
            .parse()
            .unwrap();
        let alive = group_exists(pid).unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        assert!(
            elapsed < std::time::Duration::from_millis(2500),
            "{elapsed:?}"
        );
        assert!(!alive);
    }
}
