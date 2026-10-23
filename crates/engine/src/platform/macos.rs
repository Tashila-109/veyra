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

pub fn display_options() -> super::DisplayOptions {
    objc2::rc::autoreleasepool(|_| {
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        super::DisplayOptions {
            reduce_transparency: workspace.accessibilityDisplayShouldReduceTransparency(),
            increase_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
            reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        }
    })
}
pub const CLAUDE_MANAGED_MCP: &str = "/Library/Application Support/ClaudeCode/managed-mcp.json";
pub const KNOWN_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    ".local/bin",
    ".grok/bin",
    ".claude/local",
];

pub fn login_shell_path() -> io::Result<(std::ffi::OsString, Option<std::ffi::OsString>)> {
    let shell = env::var_os("SHELL").unwrap_or_else(|| "/bin/zsh".into());
    bounded_shell_path(&shell, std::time::Duration::from_secs(2))
}
pub(crate) fn bounded_shell_path(
    shell: &std::ffi::OsStr,
    bound: std::time::Duration,
) -> io::Result<(std::ffi::OsString, Option<std::ffi::OsString>)> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    let mut child = Command::new(shell)
        .args(["-l", "-c", "printf '%s\\0%s' \"$PATH\" \"${__MISE_DIFF-}\""])
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
                    use std::os::unix::ffi::OsStringExt;
                    let mut fields = bytes.splitn(2, |byte| *byte == 0);
                    let path =
                        std::ffi::OsString::from_vec(fields.next().unwrap_or_default().to_vec());
                    let mise_diff = fields
                        .next()
                        .filter(|bytes| !bytes.is_empty())
                        .map(|bytes| std::ffi::OsString::from_vec(bytes.to_vec()));
                    Ok((path, mise_diff))
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
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Err(io::Error::last_os_error());
    }
    let mut pids = vec![0i32; count as usize + 64];
    let count = unsafe {
        libc::proc_listallpids(
            pids.as_mut_ptr().cast(),
            std::mem::size_of_val(pids.as_slice()) as i32,
        )
    };
    if count <= 0 {
        return Err(io::Error::last_os_error());
    }
    pids.truncate(count as usize);
    let mut processes = Vec::new();
    for pid in pids.into_iter().filter(|pid| *pid > 0) {
        let pgid = unsafe { libc::getpgid(pid) };
        if pgid < 0 {
            continue;
        }
        let Ok(bytes) = process_block(pid as u32) else {
            continue;
        };
        let Ok(block) = ProcessBlock::read(&bytes) else {
            continue;
        };
        processes.push(ProcessInfo {
            pid: pid as u32,
            pgid: pgid as u32,
            argv: block
                .argv
                .iter()
                .map(|argument| String::from_utf8_lossy(argument).into_owned())
                .collect(),
        });
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
fn process_block(pid: u32) -> io::Result<Vec<u8>> {
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
    Ok(bytes)
}
struct ProcessBlock<'a> {
    argv: Vec<&'a [u8]>,
    environment: &'a [u8],
}
impl<'a> ProcessBlock<'a> {
    fn read(bytes: &'a [u8]) -> io::Result<Self> {
        let count = bytes
            .get(..4)
            .and_then(|bytes| bytes.try_into().ok())
            .map(i32::from_ne_bytes)
            .filter(|count| *count >= 0)
            .ok_or_else(|| io::Error::other("invalid process argument block"))?;
        let end = |offset: usize| {
            bytes
                .get(offset..)
                .and_then(|rest| rest.iter().position(|byte| *byte == 0))
                .map(|index| offset + index + 1)
                .ok_or_else(|| io::Error::other("invalid process argument block"))
        };
        let mut offset = end(4)?;
        while bytes.get(offset) == Some(&0) {
            offset += 1;
        }
        let mut argv = Vec::new();
        for _ in 0..count {
            let next = end(offset)?;
            argv.push(&bytes[offset..next - 1]);
            offset = next;
        }
        Ok(Self {
            argv,
            environment: &bytes[offset..],
        })
    }
}
pub fn process_env(pid: u32, name: &str) -> io::Result<Option<String>> {
    let bytes = process_block(pid)?;
    let block = ProcessBlock::read(&bytes)?;
    let prefix = format!("{name}=");
    Ok(block
        .environment
        .split(|byte| *byte == 0)
        .find_map(|entry| {
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

pub fn is_incompatible_executable(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::ENOEXEC | libc::EBADARCH | libc::EBADMACHO)
    )
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
pub fn test_symlink(original: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(original, link)
}

pub fn private_text(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension("pending");
    let mut file = private_file(
        &temporary,
        OpenOptions::new().create(true).write(true).truncate(true),
    )?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(temporary, path)
}
pub fn quarantine_file(path: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(path, destination)?;
    let metadata = fs::symlink_metadata(destination)?;
    if metadata.is_file() {
        fs::set_permissions(destination, fs::Permissions::from_mode(0o600))?;
    } else if metadata.is_dir() {
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
pub fn optional_text(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
pub fn remove_optional_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub type Socket = tokio::net::UnixStream;
pub type SocketListener = tokio::net::UnixListener;
pub fn bind_async_socket(path: &Path) -> io::Result<SocketListener> {
    let listener = bind_socket(path)?;
    listener.set_nonblocking(true)?;
    SocketListener::from_std(listener)
}
pub async fn connect_async_socket(path: &Path) -> io::Result<Socket> {
    Socket::connect(path).await
}
pub fn peer_pid(stream: &Socket) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut size = std::mem::size_of_val(&pid) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    if pid <= 0 || size as usize != std::mem::size_of_val(&pid) {
        return Err(io::Error::other("socket peer pid unavailable"));
    }
    Ok(pid as u32)
}
fn bsd_info(pid: u32) -> io::Result<libc::proc_bsdinfo> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    if unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    } != size as i32
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { info.assume_init() })
}
pub fn parent_pid(pid: u32) -> io::Result<u32> {
    bsd_info(pid).map(|info| info.pbi_ppid)
}
const VNODE_FD_INFO: i32 = 1;

#[repr(C)]
struct DescriptorInfo {
    flags: u32,
    status: u32,
    offset: i64,
    kind: i32,
    guard: u32,
}
#[repr(C)]
struct VnodeDescriptor {
    file: DescriptorInfo,
    vnode: libc::vnode_info,
}
fn descriptor_device(pid: u32, fd: i32) -> io::Result<u32> {
    let mut info = std::mem::MaybeUninit::<VnodeDescriptor>::uninit();
    let size = std::mem::size_of::<VnodeDescriptor>();
    if unsafe {
        libc::proc_pidfdinfo(
            pid as i32,
            fd,
            VNODE_FD_INFO,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    } != size as i32
    {
        return Err(io::Error::last_os_error());
    }
    let stat = unsafe { info.assume_init() }.vnode.vi_stat;
    if stat.vst_mode & libc::S_IFMT != libc::S_IFCHR {
        return Err(io::Error::other("descriptor is not a character device"));
    }
    Ok(stat.vst_rdev)
}
pub fn terminal_device(pid: u32) -> io::Result<u32> {
    let device = bsd_info(pid)?.e_tdev;
    if device != u32::MAX {
        Ok(device)
    } else {
        descriptor_device(pid, 0)
    }
}
pub fn terminal_holder(device: u32) -> Option<u32> {
    if device == u32::MAX {
        return None;
    }
    list_processes().ok()?.into_iter().find_map(|process| {
        (bsd_info(process.pid).is_ok_and(|info| info.e_tdev == device)
            || (0..3).any(|fd| descriptor_device(process.pid, fd).ok() == Some(device)))
        .then_some(process.pid)
    })
}
pub fn exe_path(pid: u32) -> io::Result<PathBuf> {
    process_executable(pid)
}
pub fn random_token_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
    bytes
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
                "owned argument with spaces",
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
        assert!(rows.unwrap().iter().any(|row| {
            row.pid == pid
                && row.pgid == pid
                && row
                    .argv
                    .iter()
                    .any(|argument| argument == "owned argument with spaces")
        }));
        assert!(rss.unwrap() > 0);
        assert!(alive.unwrap());
        assert!(!is_alive(pid).unwrap());
    }
    #[test]
    fn terminal_holder_finds_an_owner_without_a_controlling_terminal() {
        struct OwnedPtyChild(Box<dyn portable_pty::Child + Send + Sync>);
        impl Drop for OwnedPtyChild {
            fn drop(&mut self) {
                let _ = signal_group(self.0.process_id().unwrap(), crate::platform::Signal::Kill);
                let _ = self.0.wait();
            }
        }
        let pair = portable_pty::native_pty_system()
            .openpty(Default::default())
            .unwrap();
        let mut command = portable_pty::CommandBuilder::new("/bin/sleep");
        command.arg("30");
        command.set_controlling_tty(false);
        command.env("VEYRA_PARENT_PID", std::process::id().to_string());
        let child = OwnedPtyChild(pair.slave.spawn_command(command).unwrap());
        drop(pair.slave);
        let pid = child.0.process_id().unwrap();
        let device = terminal_device(pid).unwrap();
        assert_ne!(device, u32::MAX);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while terminal_holder(device) != Some(pid) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(terminal_holder(device), Some(pid));
        drop(child);
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
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/veyra-shell-pipe-{}",
            crate::domain::ThreadId::new()
        ));
        let _scratch = crate::tests::Scratch(root.clone());
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let shell = root.join("shell");
        let marker = root.join("escaped-pid");
        let binary = env::current_exe().unwrap();
        let quoted_binary = binary.to_string_lossy().replace('\'', "'\"'\"'");
        let quoted_root = root.to_string_lossy().replace('\'', "'\"'\"'");
        fs::write(
            &shell,
            format!(
                "#!/bin/sh\nexport VEYRA_TEST_SHELL_PIPE_ROOT='{quoted_root}'\nexec '{quoted_\
            binary}' --exact platform::macos::tests::shell_pipe_child --ignored --nocapture\n"
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
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "inherited pipe held shell for {elapsed:?}"
        );
    }
    #[test]
    #[ignore = "owned inherited-pipe fixture entrypoint"]
    fn shell_pipe_child() {
        use std::os::unix::process::CommandExt;
        let root = PathBuf::from(
            env::var_os("VEYRA_TEST_SHELL_PIPE_ROOT").expect("owned fixture requires its root"),
        );
        if env::var_os("VEYRA_TEST_SHELL_PIPE_CHILD").is_some() {
            fs::write(root.join("escaped-pid"), std::process::id().to_string()).unwrap();
        } else {
            let mut child = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "platform::macos::tests::shell_pipe_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("VEYRA_TEST_SHELL_PIPE_CHILD", "1")
                .process_group(0)
                .spawn()
                .unwrap();
            child.wait().unwrap();
            return;
        }
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
    #[test]
    fn login_shell_bounded_at_2s() {
        if env::var_os("VEYRA_SHELL_TIMEOUT_CHILD").is_some() {
            assert!(login_shell_path().is_err());
            return;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/veyra-shell-{}",
            crate::domain::ThreadId::new()
        ));
        let _scratch = crate::tests::Scratch(root.clone());
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

struct RelayFd {
    fd: libc::c_int,
    flags: libc::c_int,
}
impl RelayFd {
    fn nonblocking(fd: libc::c_int) -> io::Result<Self> {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { fd, flags })
    }
}
impl Drop for RelayFd {
    fn drop(&mut self) {
        unsafe { libc::fcntl(self.fd, libc::F_SETFL, self.flags) };
    }
}
#[derive(Default)]
struct RelayBuffer {
    bytes: Vec<u8>,
    written: usize,
}
impl RelayBuffer {
    const LIMIT: usize = 8 * 1024 * 1024;
    fn pending(&self) -> bool {
        self.written < self.bytes.len()
    }
    fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.written > 0 {
            self.bytes.drain(..self.written);
            self.written = 0;
        }
        if self.bytes.len() + bytes.len() > Self::LIMIT {
            return Err(io::Error::other(
                "MCP relay exceeded its 8 MiB write buffer",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
    fn write(&mut self, fd: libc::c_int) -> io::Result<()> {
        if !self.pending() {
            return Ok(());
        }
        let bytes = &self.bytes[self.written..];
        let count = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if count > 0 {
            self.written += count as usize;
            return Ok(());
        }
        if count == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        let error = io::Error::last_os_error();
        if matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
        ) {
            Ok(())
        } else {
            Err(error)
        }
    }
}
fn relay_read(fd: libc::c_int, buffer: &mut [u8]) -> io::Result<Option<usize>> {
    let count = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    if count >= 0 {
        return Ok(Some(count as usize));
    }
    let error = io::Error::last_os_error();
    if matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    ) {
        Ok(None)
    } else {
        Err(error)
    }
}
pub fn relay_stdio(connection: UnixStream, authentication: &[u8]) -> io::Result<()> {
    let input = RelayFd::nonblocking(libc::STDIN_FILENO)?;
    let output = RelayFd::nonblocking(libc::STDOUT_FILENO)?;
    let socket = RelayFd::nonblocking(connection.as_raw_fd())?;
    let mut to_socket = RelayBuffer::default();
    to_socket.append(authentication)?;
    to_socket.append(b"\n")?;
    let mut to_stdout = RelayBuffer::default();
    let mut socket_eof = false;
    let mut buffer = [0u8; 8192];
    loop {
        if socket_eof && !to_stdout.pending() {
            return Ok(());
        }
        let mut poll = [
            libc::pollfd {
                fd: input.fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: socket.fd,
                events: if socket_eof { 0 } else { libc::POLLIN }
                    | if to_socket.pending() {
                        libc::POLLOUT
                    } else {
                        0
                    },
                revents: 0,
            },
            libc::pollfd {
                fd: output.fd,
                events: if to_stdout.pending() {
                    libc::POLLOUT
                } else {
                    0
                },
                revents: 0,
            },
        ];
        if unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as libc::nfds_t, -1) } < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if poll[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            match relay_read(input.fd, &mut buffer)? {
                Some(0) => {
                    let _ = connection.shutdown(std::net::Shutdown::Both);
                    return Ok(());
                }
                Some(count) => to_socket.append(&buffer[..count])?,
                None => {}
            }
        }
        if !socket_eof && poll[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            match relay_read(socket.fd, &mut buffer)? {
                Some(0) => socket_eof = true,
                Some(count) => to_stdout.append(&buffer[..count])?,
                None => {}
            }
        }
        if poll[1].revents & libc::POLLOUT != 0 {
            to_socket.write(socket.fd)?;
        }
        if poll[2].revents & (libc::POLLOUT | libc::POLLHUP | libc::POLLERR) != 0 {
            to_stdout.write(output.fd)?;
        }
        if poll.iter().any(|fd| fd.revents & libc::POLLNVAL != 0) {
            return Err(io::Error::other("MCP relay descriptor closed"));
        }
    }
}

pub fn raw_terminal_input() -> io::Result<()> {
    let input = io::stdin();
    let mut attributes = std::mem::MaybeUninit::<libc::termios>::uninit();
    if unsafe { libc::tcgetattr(input.as_raw_fd(), attributes.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attributes = unsafe { attributes.assume_init() };
    unsafe { libc::cfmakeraw(&mut attributes) };
    if unsafe { libc::tcsetattr(input.as_raw_fd(), libc::TCSANOW, &attributes) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
#[test]
fn raw_terminal_input_preserves_escape_and_carriage_return() {
    use std::io::{Read, Write};
    if std::env::var_os("VEYRA_TEST_RAW_INPUT").is_some() {
        raw_terminal_input().unwrap();
        std::io::stdout().write_all(b"RAW_READY\n").unwrap();
        std::io::stdout().flush().unwrap();
        let mut bytes = [0; 2];
        std::io::stdin().read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, [27, 13]);
        return;
    }
    let pair = portable_pty::native_pty_system()
        .openpty(Default::default())
        .unwrap();
    let mut command = portable_pty::CommandBuilder::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        "platform::macos::raw_terminal_input_preserves_escape_and_carriage_return",
        "--nocapture",
    ]);
    command.env("VEYRA_TEST_RAW_INPUT", "1");
    let mut child = pair.slave.spawn_command(command).unwrap();
    let pid = child.process_id().unwrap();
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut writer = pair.master.take_writer().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reading = std::thread::spawn(move || {
        let mut line = Vec::new();
        let mut byte = [0];
        while reader.read(&mut byte).unwrap_or(0) != 0 {
            line.push(byte[0]);
            if byte[0] == b'\n' {
                if line.ends_with(b"RAW_READY\n") {
                    let _ = sender.send(());
                }
                line.clear();
            }
        }
    });
    let ready = receiver.recv_timeout(std::time::Duration::from_secs(5));
    if ready.is_ok() {
        writer.write_all(&[27, 13]).unwrap();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = signal_group(pid, super::Signal::Kill);
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    drop(writer);
    drop(pair.master);
    reading.join().unwrap();
    ready.unwrap();
    assert!(status.unwrap().success());
}

pub fn user_shell() -> PathBuf {
    env::var_os("SHELL")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/zsh"))
}
pub fn duplicate_fd(fd: i32) -> io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::BorrowedFd;
    unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned()
}
pub fn foreground_group(pty: &std::os::fd::OwnedFd) -> io::Result<u32> {
    let group = unsafe { libc::tcgetpgrp(pty.as_raw_fd()) };
    if group <= 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(group as u32)
    }
}
pub fn child_pids(pid: u32) -> io::Result<Vec<u32>> {
    let mut children = vec![0u32; 64];
    loop {
        let count = unsafe {
            libc::proc_listchildpids(
                pid as i32,
                children.as_mut_ptr().cast(),
                (children.len() * std::mem::size_of::<u32>()) as i32,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let count = count as usize;
        if count < children.len() {
            children.truncate(count);
            children.retain(|pid| bsd_info(*pid).is_ok_and(|info| info.pbi_status != 5));
            return Ok(children);
        }
        children.resize(children.len() * 2, 0);
    }
}

pub struct ShellJob(std::process::Child);
impl Drop for ShellJob {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub fn shell_fixture_job() -> io::Result<ShellJob> {
    Command::new("/bin/sleep")
        .arg("60")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(ShellJob)
}

#[cfg(test)]
#[test]
fn duplicated_descriptor_survives_original_reuse() {
    use std::io::{Read, Seek, Write};
    let root = std::env::temp_dir().join(format!("veyra-fd-{}", crate::domain::ThreadId::new()));
    std::fs::create_dir(&root).unwrap();
    let _scratch = crate::tests::Scratch(root.clone());
    let mut original = File::create(root.join("original")).unwrap();
    original.write_all(b"original").unwrap();
    drop(original);
    let original = File::open(root.join("original")).unwrap();
    let number = original.as_raw_fd();
    let duplicate = duplicate_fd(number).unwrap();
    assert_ne!(duplicate.as_raw_fd(), number);
    drop(original);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let replacement = loop {
        let replacement = File::create(root.join("replacement")).unwrap();
        if replacement.as_raw_fd() == number {
            break replacement;
        }
        drop(replacement);
        assert!(
            std::time::Instant::now() < deadline,
            "the owned descriptor was not reused within two seconds"
        );
        std::thread::yield_now();
    };
    let mut held = File::from(duplicate);
    held.rewind().unwrap();
    let mut contents = String::new();
    held.read_to_string(&mut contents).unwrap();
    assert_eq!(contents, "original");
    assert!(foreground_group(&held.into()).is_err());
    assert_eq!(replacement.as_raw_fd(), number);
}

#[cfg(test)]
#[test]
#[ignore = "descriptor churn stress loop"]
fn duplicated_descriptor_reuse_under_churn() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    let stop = Arc::new(AtomicBool::new(false));
    let opened = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let stop = stop.clone();
            let opened = opened.clone();
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    let file = File::open("/dev/null").unwrap();
                    opened.fetch_add(1, Ordering::Relaxed);
                    std::thread::yield_now();
                    drop(file);
                }
            })
        })
        .collect();
    let failures = (0..20_000)
        .filter(|_| {
            std::panic::catch_unwind(duplicated_descriptor_survives_original_reuse).is_err()
        })
        .count();
    stop.store(true, Ordering::Release);
    for worker in workers {
        worker.join().unwrap();
    }
    let opened = opened.load(Ordering::Relaxed);
    println!("descriptor reuse: 20000 repetitions, {failures} failures, {opened} concurrent opens");
    assert!(opened > 0);
    assert_eq!(failures, 0);
}
