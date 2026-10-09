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
    let output = Command::new(shell)
        .args(["-l", "-c", "printf '%s' \"$PATH\""])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("login shell could not read PATH"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
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
