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
    let start = std::time::Instant::now();
    let result = (|| {
        use std::io::Read;
        let flags = unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(stdout.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        let mut status = None;
        let mut eof = false;
        loop {
            if !eof {
                match stdout.read(&mut buffer) {
                    Ok(0) => eof = true,
                    Ok(count) => bytes.extend_from_slice(&buffer[..count]),
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            if status.is_none() {
                status = child.try_wait()?;
            }
            if eof && let Some(status) = status {
                return if status.success() {
                    Ok(String::from_utf8_lossy(&bytes).trim().into())
                } else {
                    Err(io::Error::other("login shell could not read PATH"))
                };
            }
            if start.elapsed() >= bound {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "login shell PATH timed out",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    })();
    let _ = signal_group(pid, super::Signal::Kill);
    let _ = child.wait();
    result
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

pub struct ProcessInfo {
    pub pid: u32,
    pub pgid: u32,
    pub argv: Vec<String>,
}
pub fn is_alive(pid: u32) -> io::Result<bool> {
    if pid <= 1 {
        return Ok(true);
    }
    if unsafe { libc::kill(pid as i32, 0) } == 0 {
        return Ok(true);
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(io::Error::last_os_error()),
    }
}
pub fn list_processes() -> io::Result<Vec<ProcessInfo>> {
    let output = Command::new("ps")
        .args(["-axww", "-o", "pid=,pgid=,command="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other("process list failed"));
    }
    let mut processes = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut words = line.split_whitespace();
        let pid = words.next().and_then(|word| word.parse().ok());
        let pgid = words.next().and_then(|word| word.parse().ok());
        if let (Some(pid), Some(pgid)) = (pid, pgid) {
            processes.push(ProcessInfo {
                pid,
                pgid,
                argv: words.map(str::to_owned).collect(),
            });
        }
    }
    Ok(processes)
}
pub fn process_executable(pid: u32) -> io::Result<PathBuf> {
    let mut bytes = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    if unsafe { libc::proc_pidpath(pid as i32, bytes.as_mut_ptr().cast(), bytes.len() as u32) } <= 0
    {
        return Err(io::Error::last_os_error());
    }
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(bytes[..end].to_vec())).canonicalize()
}
pub fn process_env(pid: u32, name: &str) -> io::Result<Option<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut size = 0;
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut bytes = vec![0u8; size];
    if unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    bytes.truncate(size);
    let count = bytes
        .get(..4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(i32::from_ne_bytes)
        .filter(|count| *count >= 0)
        .ok_or_else(|| io::Error::other("invalid process argument block"))?;
    let mut offset = 4;
    let end = |offset: usize| {
        bytes
            .get(offset..)
            .and_then(|rest| rest.iter().position(|b| *b == 0))
            .map(|index| offset + index + 1)
            .ok_or_else(|| io::Error::other("invalid process argument block"))
    };
    offset = end(offset)?;
    while bytes.get(offset) == Some(&0) {
        offset += 1;
    }
    for _ in 0..count {
        offset = end(offset)?;
    }
    let prefix = format!("{name}=");
    Ok(bytes[offset..].split(|byte| *byte == 0).find_map(|entry| {
        entry
            .strip_prefix(prefix.as_bytes())
            .map(|value| String::from_utf8_lossy(value).into_owned())
    }))
}
pub fn lower_priority(group: u32) -> io::Result<()> {
    if unsafe { libc::setpriority(libc::PRIO_PGRP, group, 10) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
pub fn resident_bytes(pid: u32) -> io::Result<u64> {
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as i32;
    if unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    } != size
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init() }.pti_resident_size)
}
pub struct AwakeGuard(std::process::Child);
impl Drop for AwakeGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub fn hold_awake(pid: u32) -> io::Result<AwakeGuard> {
    use std::process::Stdio;
    Command::new("/usr/bin/caffeinate")
        .args(["-i", "-w", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(AwakeGuard)
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
    fn platform_inspection_reads_owned_process() {
        if env::var_os("VEYRA_TEST_INSPECTION_CHILD").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(10));
            return;
        }
        use std::os::unix::process::CommandExt;
        let parent = std::process::id().to_string();
        let mut child = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "platform::macos::tests::platform_inspection_reads_owned_process",
            ])
            .env("VEYRA_TEST_INSPECTION_CHILD", "1")
            .env("VEYRA_PARENT_PID", &parent)
            .env_remove("VEYRA_PARENT")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id();
        let marker = process_env(pid, "VEYRA_PARENT_PID");
        let shorter = process_env(pid, "VEYRA_PARENT");
        let rows = list_processes();
        let rss = resident_bytes(pid);
        let alive = is_alive(pid);
        signal_group(pid, crate::platform::Signal::Kill).ok();
        child.wait().unwrap();
        assert_eq!(marker.unwrap(), Some(parent));
        assert_eq!(shorter.unwrap(), None);
        assert!(
            rows.unwrap()
                .iter()
                .any(|row| row.pid == pid && row.pgid == pid)
        );
        assert!(rss.unwrap() > 0);
        assert!(alive.unwrap());
        assert!(!is_alive(pid).unwrap());
    }
    #[test]
    fn lower_priority_sets_group_nice() {
        use std::os::unix::process::CommandExt;
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let group = child.id();
        let result = lower_priority(group);
        let nice = unsafe { libc::getpriority(libc::PRIO_PGRP, group) };
        let _ = signal_group(group, crate::platform::Signal::Kill);
        child.wait().unwrap();
        result.unwrap();
        assert_eq!(nice, 10);
    }
    #[test]
    fn login_shell_bounds_inherited_pipe() {
        let root = std::env::temp_dir().join(format!(
            "veyra-shell-pipe-{}",
            crate::domain::ThreadId::new()
        ));
        fs::create_dir(&root).unwrap();
        let shell = root.join("shell");
        let marker = root.join("escaped-pid");
        fs::write(
            &shell,
            format!(
                r#"#!/usr/bin/env python3
import os, time
if os.fork() == 0:
    os.setsid()
    with open('{}', 'w') as marker: marker.write(str(os.getpid()))
    time.sleep(4)
    os._exit(0)
time.sleep(60)
"#,
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shell, fs::Permissions::from_mode(0o700)).unwrap();
        let start = std::time::Instant::now();
        let result = bounded_shell_path(shell.as_os_str(), std::time::Duration::from_secs(2));
        let elapsed = start.elapsed();
        let escaped: i32 = fs::read_to_string(marker).unwrap().parse().unwrap();
        unsafe {
            libc::kill(escaped, libc::SIGKILL);
        }
        fs::remove_dir_all(root).unwrap();
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "inherited pipe held shell for {elapsed:?}"
        );
    }
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
