use crate::{
    platform::{self, Signal},
    provider::{ProviderError, TerminalControl, TerminalHandle, env::ChildEnv},
};
use alacritty_terminal::{
    event::{Event, EventListener},
    sync::FairMutex,
    term::{Config, Term},
    vte::ansi::Processor,
};
pub use alacritty_terminal::{
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point, Side},
    selection::{Selection, SelectionType},
    term::{
        TermMode,
        cell::{Cell, Flags},
    },
    vte::ansi::{Color, CursorShape, NamedColor, Rgb},
};
pub use portable_pty::PtySize;
use portable_pty::{CommandBuilder, MasterPty};
use std::{
    io::{self, Read, Write},
    os::fd::OwnedFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Default)]
pub struct Listener(Arc<Mutex<Vec<Event>>>);
impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        self.0.lock().unwrap().push(event);
    }
}

struct Size(PtySize);
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines()
    }
    fn screen_lines(&self) -> usize {
        self.0.rows as usize
    }
    fn columns(&self) -> usize {
        self.0.cols as usize
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
}
impl Default for Colors {
    fn default() -> Self {
        Self {
            foreground: Rgb {
                r: 230,
                g: 230,
                b: 230,
            },
            background: Rgb {
                r: 24,
                g: 24,
                b: 24,
            },
        }
    }
}
impl Colors {
    fn get(self, index: usize) -> Rgb {
        match index {
            index
                if index == NamedColor::Foreground as usize
                    || index == NamedColor::Cursor as usize =>
            {
                self.foreground
            }
            index if index == NamedColor::Background as usize => self.background,
            _ => xterm_color((index % 256) as u8),
        }
    }
}

pub fn xterm_color(index: u8) -> Rgb {
    let index = index as usize;
    match index {
        16..=231 => {
            let value = index - 16;
            let channel = |part| if part == 0 { 0 } else { (55 + part * 40) as u8 };
            Rgb {
                r: channel(value / 36),
                g: channel(value / 6 % 6),
                b: channel(value % 6),
            }
        }
        232..=255 => {
            let value = (8 + 10 * (index - 232)) as u8;
            Rgb {
                r: value,
                g: value,
                b: value,
            }
        }
        _ => {
            let value = index % 16;
            let level = if value >= 8 { 255 } else { 170 };
            let low = if value >= 8 { 85 } else { 0 };
            Rgb {
                r: if value & 1 != 0 { level } else { low },
                g: if value & 2 != 0 { level } else { low },
                b: if value & 4 != 0 { level } else { low },
            }
        }
    }
}

pub struct Emulator {
    pub term: Term<Listener>,
    parser: Processor,
    colors: Colors,
    listener: Listener,
    #[cfg(debug_assertions)]
    pub flood: FloodStamps,
}
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug)]
pub struct ReaderStamp {
    pub at: Instant,
    pub bytes: u64,
}
#[cfg(debug_assertions)]
#[derive(Clone, Copy, Debug, Default)]
pub struct FloodStamps {
    pub start: Option<ReaderStamp>,
    pub done: Option<ReaderStamp>,
}
impl Emulator {
    pub fn new(size: PtySize) -> Self {
        Self::with_colors(size, Colors::default())
    }
    pub fn with_colors(size: PtySize, colors: Colors) -> Self {
        let listener = Listener::default();
        Self {
            term: Term::new(
                Config {
                    kitty_keyboard: false,
                    ..Default::default()
                },
                &Size(size),
                listener.clone(),
            ),
            colors,
            parser: Processor::new(),
            listener,
            #[cfg(debug_assertions)]
            flood: FloodStamps::default(),
        }
    }
    fn feed(&mut self, bytes: &[u8], consumed: u64) -> Vec<u8> {
        let mut reply = Vec::new();
        #[cfg(debug_assertions)]
        let mut parsed = consumed - bytes.len() as u64;
        #[cfg(debug_assertions)]
        let parts = bytes
            .split_inclusive(|byte| *byte == 7 || *byte == b'\\')
            .chain(bytes.is_empty().then_some(&[][..]));
        #[cfg(not(debug_assertions))]
        let parts = std::iter::once(bytes);
        for part in parts {
            #[cfg(debug_assertions)]
            {
                parsed += part.len() as u64;
            }
            self.parser.advance(&mut self.term, part);
            for event in self.listener.0.lock().unwrap().drain(..) {
                match event {
                    Event::PtyWrite(text) => reply.extend_from_slice(text.as_bytes()),
                    Event::ColorRequest(index, format) => reply.extend_from_slice(
                        format(self.term.colors()[index].unwrap_or_else(|| self.colors.get(index)))
                            .as_bytes(),
                    ),
                    Event::Title(title) => {
                        #[cfg(debug_assertions)]
                        {
                            let stamp = ReaderStamp {
                                at: Instant::now(),
                                bytes: parsed,
                            };
                            match title.as_str() {
                                "FLOOD_START" => {
                                    self.flood = FloodStamps {
                                        start: Some(stamp),
                                        done: None,
                                    }
                                }
                                "FLOOD_DONE" => self.flood.done = Some(stamp),
                                _ => {}
                            }
                        }
                        #[cfg(not(debug_assertions))]
                        let _ = title;
                    }
                    _ => {}
                }
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = consumed;
        reply
    }
    fn flush_sync_timeout(&mut self) -> bool {
        if self
            .parser
            .sync_timeout()
            .sync_timeout()
            .is_some_and(|deadline| deadline <= Instant::now())
        {
            self.parser.stop_sync(&mut self.term);
            true
        } else {
            false
        }
    }
    pub fn visible_tail(&self) -> String {
        let mut lines: Vec<_> = self
            .text()
            .into_iter()
            .map(|line| line.trim().to_owned())
            .filter(|line| !line.is_empty())
            .rev()
            .take(10)
            .collect();
        lines.reverse();
        lines.join("\n")
    }
    pub fn text(&self) -> Vec<String> {
        (0..self.term.screen_lines())
            .map(|row| {
                let line =
                    &self.term.grid()[Line(row as i32 - self.term.grid().display_offset() as i32)];
                let text: String = (0..self.term.columns())
                    .filter_map(|col| {
                        let cell = &line[Column(col)];
                        (!cell.flags.intersects(
                            alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER
                                | alacritty_terminal::term::cell::Flags::LEADING_WIDE_CHAR_SPACER,
                        ))
                        .then_some(cell.c)
                    })
                    .collect();
                text.trim_end().to_owned()
            })
            .collect()
    }
}

pub const SHELL_TAB_LIMIT: usize = 6;

enum Input {
    Write(Vec<u8>),
    Resize(PtySize),
}
pub struct Terminal {
    pub emulator: FairMutex<Emulator>,
    input: Mutex<Option<std::sync::mpsc::Sender<Input>>>,
    dirty: AtomicBool,
    pid: Mutex<Option<u32>>,
    foreground: Mutex<Option<OwnedFd>>,
    status: Mutex<ShellStatus>,
    hangup_sent: AtomicBool,
    hangup_at: Mutex<Option<Instant>>,
    hungup_foreground: Mutex<Option<u32>>,
    closing: AtomicBool,
    wake: watch::Sender<u64>,
}
impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal").finish_non_exhaustive()
    }
}
impl Terminal {
    pub fn pid(&self) -> Option<u32> {
        *self.pid.lock().unwrap()
    }
    pub(crate) fn close(&self) {
        self.closing.store(true, Ordering::Release);
        self.hangup();
    }
    pub(crate) fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }
    pub(crate) fn hangup(&self) {
        let owned = self.pid.lock().unwrap();
        if let Some(pid) = *owned
            && !self.hangup_sent.swap(true, Ordering::AcqRel)
        {
            *self.hangup_at.lock().unwrap() = Some(Instant::now());
            *self.hungup_foreground.lock().unwrap() =
                self.foreground_group().filter(|group| *group != pid);
            let _ = platform::signal_group(pid, Signal::Hangup);
        }
    }
    pub fn status(&self) -> ShellStatus {
        self.status.lock().unwrap().clone()
    }
    fn foreground_group(&self) -> Option<u32> {
        self.foreground
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|fd| platform::foreground_group(fd).ok())
    }
    pub fn running_processes(&self) -> Vec<std::path::PathBuf> {
        let Some(pid) = self.pid() else {
            return Vec::new();
        };
        let mut pids = platform::child_pids(pid).unwrap_or_default();
        if let Some(group) = self.foreground_group().filter(|group| *group != pid) {
            pids.push(group);
        }
        pids.sort_unstable();
        pids.dedup();
        pids.into_iter()
            .map(|pid| {
                platform::process_executable(pid)
                    .unwrap_or_else(|_| std::path::PathBuf::from("command"))
            })
            .collect()
    }
    fn enqueue(&self, input: Input) -> io::Result<()> {
        self.input
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "terminal is not running"))?
            .send(input)
            .map_err(|_| io::Error::new(io::ErrorKind::NotConnected, "terminal input ended"))
    }
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        self.enqueue(Input::Write(bytes.to_vec()))
    }
    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        if size.cols < 2 || size.rows == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal needs at least two columns and one row",
            ));
        }
        self.enqueue(Input::Resize(size))
    }
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.wake.subscribe()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShellStatus {
    Starting,
    Running,
    Exited { code: u32 },
    Signalled,
    Closed,
    Failed { error: String },
}
impl ShellStatus {
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            Self::Exited { .. } | Self::Signalled | Self::Closed | Self::Failed { .. }
        )
    }
    fn from_exit(status: portable_pty::ExitStatus) -> Self {
        if status.to_string().starts_with("Terminated by ") {
            Self::Signalled
        } else {
            Self::Exited {
                code: status.exit_code(),
            }
        }
    }
}
pub(crate) fn spawn(
    cwd: std::path::PathBuf,
    size: PtySize,
    env: ChildEnv,
    colors: Colors,
) -> Result<TerminalHandle, ProviderError> {
    if !cwd.is_dir() {
        return Err(ProviderError::Protocol(
            "The thread folder no longer exists".into(),
        ));
    }
    let terminal = Arc::new(Terminal {
        emulator: FairMutex::new(Emulator::with_colors(size, colors)),
        input: Mutex::new(None),
        dirty: AtomicBool::new(false),
        pid: Mutex::new(None),
        foreground: Mutex::new(None),
        status: Mutex::new(ShellStatus::Starting),
        hangup_sent: AtomicBool::new(false),
        hangup_at: Mutex::new(None),
        hungup_foreground: Mutex::new(None),
        closing: AtomicBool::new(false),
        wake: watch::channel(0).0,
    });
    let (control, controls) = mpsc::unbounded_channel();
    let handle = TerminalHandle {
        control,
        terminal: terminal.clone(),
    };
    std::thread::Builder::new()
        .name("veyra-pty".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(cwd, size, env, terminal.clone(), controls)
            }))
            .unwrap_or_else(|_| {
                Err(ProviderError::Protocol(
                    "The shell worker stopped unexpectedly".into(),
                ))
            });
            let mut status = terminal.status.lock().unwrap();
            if let Err(error) = result {
                *status = if matches!(error, ProviderError::Cancelled) {
                    ShellStatus::Closed
                } else {
                    ShellStatus::Failed {
                        error: crate::record::redact(&error.to_string(), None),
                    }
                };
            } else if !status.is_finished() {
                *status = ShellStatus::Closed;
            }
            drop(status);
            terminal.wake.send_modify(|counter| *counter += 1);
        })?;
    Ok(handle)
}
struct ChildGroup {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    pid: u32,
    groups: crate::provider::env::Groups,
    reaped: bool,
}
impl ChildGroup {
    fn try_wait(&mut self) -> io::Result<Option<portable_pty::ExitStatus>> {
        let status = self.child.try_wait()?;
        self.reaped |= status.is_some();
        if self.reaped {
            self.groups.lock().unwrap().remove(&self.pid);
        }
        Ok(status)
    }
    fn finish(&mut self, signal: impl FnOnce(u32)) {
        if !self.reaped {
            signal(self.pid);
            self.reaped = self.child.wait().is_ok();
        }
        self.groups.lock().unwrap().remove(&self.pid);
    }
}
impl Drop for ChildGroup {
    fn drop(&mut self) {
        self.finish(|pid| {
            let _ = platform::signal_group(pid, Signal::Kill);
        });
    }
}
struct Workers {
    child: Option<ChildGroup>,
    terminal: Arc<Terminal>,
    reader: Option<std::thread::JoinHandle<io::Result<()>>>,
    writer: Option<std::thread::JoinHandle<io::Result<()>>>,
    deadline: Option<Instant>,
    device: Option<u32>,
    closing: Arc<crate::LaunchGate>,
}
impl Workers {
    fn finish(&mut self, worker: Option<std::thread::JoinHandle<io::Result<()>>>, kind: &str) {
        if let Some(worker) = worker {
            let mut deadline = *self.deadline.get_or_insert_with(|| {
                Instant::now() + Duration::from_secs(if self.closing.is_closing() { 1 } else { 3 })
            });
            while !worker.is_finished() {
                if self.closing.is_closing() {
                    deadline = deadline.min(Instant::now() + Duration::from_secs(1));
                }
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            self.deadline = Some(deadline);
            if worker.is_finished() {
                let _ = worker.join();
            } else {
                let holder_pid = self.device.and_then(platform::terminal_holder);
                tracing::warn!(
                    holder_pid,
                    worker = kind,
                    "terminal I/O deadline reached; a surviving process may hold the PTY"
                );
            }
        }
    }
}
impl Drop for Workers {
    fn drop(&mut self) {
        let mut owned = self.terminal.pid.lock().unwrap();
        drop(self.child.take());
        *owned = None;
        drop(owned);
        *self.terminal.foreground.lock().unwrap() = None;
        *self.terminal.input.lock().unwrap() = None;
        let writer = self.writer.take();
        self.finish(writer, "writer");
        let reader = self.reader.take();
        self.finish(reader, "reader");
        self.terminal.wake.send_modify(|counter| *counter += 1);
    }
}
fn take_foreground_kill(
    group: &mut Option<u32>,
    expired: bool,
    exists: impl FnOnce(u32) -> bool,
) -> Option<u32> {
    let pid = (*group)?;
    if !exists(pid) {
        *group = None;
        None
    } else if expired {
        group.take()
    } else {
        None
    }
}
fn run(
    cwd: std::path::PathBuf,
    size: PtySize,
    env: ChildEnv,
    terminal: Arc<Terminal>,
    mut controls: mpsc::UnboundedReceiver<TerminalControl>,
) -> Result<(), ProviderError> {
    let pair = portable_pty::native_pty_system()
        .openpty(size)
        .map_err(|_| {
            ProviderError::Protocol(
                "The shell could not start: macOS could not open a terminal".into(),
            )
        })?;
    let shell = env
        .variable("SHELL")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(platform::user_shell);
    #[cfg(debug_assertions)]
    let fixture = env.shell_fixture.clone();
    #[cfg(not(debug_assertions))]
    let fixture: Option<std::path::PathBuf> = None;
    let mut command = if let Some(fixture) = &fixture {
        let binary = std::env::current_exe()?
            .parent()
            .unwrap()
            .join("veyra-fake-cli");
        let mut command = CommandBuilder::new(binary);
        command.arg("shell");
        command.env("VEYRA_FAKE_FIXTURE", fixture);
        command
    } else {
        let mut command = CommandBuilder::new(shell);
        command.arg("-l");
        command
    };
    #[cfg(test)]
    command.set_controlling_tty(env.terminal_controlling_tty);
    command.cwd(&cwd);
    command.env_clear();
    for (key, value) in env.shell_environment(std::env::vars_os()) {
        command.env(key, value);
    }
    if let Some(fixture) = fixture {
        command.env("VEYRA_FAKE_FIXTURE", fixture);
    }
    let admission = env.closing.enter();
    if env.closing.is_closing() || controls.is_closed() || controls.try_recv().is_ok() {
        return Err(ProviderError::Cancelled);
    }
    let child = pair.slave.spawn_command(command).map_err(|error| {
        let reason = error
            .downcast_ref::<io::Error>()
            .map(ToString::to_string)
            .unwrap_or_else(|| "the selected shell is missing or is not executable".into());
        ProviderError::Protocol(format!("The shell could not start: {reason}"))
    })?;
    let pid = child
        .process_id()
        .ok_or_else(|| io::Error::other("PTY child has no pid"))?;
    let mut workers = Workers {
        child: Some(ChildGroup {
            child,
            pid,
            groups: env.groups.clone(),
            reaped: false,
        }),
        terminal: terminal.clone(),
        reader: None,
        writer: None,
        deadline: None,
        device: platform::terminal_device(pid).ok(),
        closing: env.closing.clone(),
    };
    env.groups.lock().unwrap().insert(pid);
    *terminal.pid.lock().unwrap() = Some(pid);
    drop(admission);
    drop(pair.slave);
    if let Err(error) = platform::lower_priority(pid) {
        tracing::warn!(%error, "could not lower terminal priority");
    }
    *terminal.foreground.lock().unwrap() = Some(platform::duplicate_fd(
        pair.master
            .as_raw_fd()
            .ok_or_else(|| io::Error::other("PTY has no descriptor"))?,
    )?);
    let reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
    let writer = pair.master.take_writer().map_err(io::Error::other)?;
    let (input, queue) = std::sync::mpsc::channel();
    *terminal.input.lock().unwrap() = Some(input);
    let writing = terminal.clone();
    workers.writer = Some(
        std::thread::Builder::new()
            .name("veyra-pty-writer".into())
            .spawn(move || write(pair.master, writer, queue, &writing))?,
    );
    let reading = terminal.clone();
    workers.reader = Some(
        std::thread::Builder::new()
            .name("veyra-pty-reader".into())
            .spawn(move || read(reader, &reading))?,
    );
    *terminal.status.lock().unwrap() = ShellStatus::Running;
    terminal.wake.send_modify(|counter| *counter += 1);
    let mut stopping: Option<Instant> = None;
    let mut foreground: Option<u32> = None;
    let mut exited = None;
    let mut leader_killed = false;
    loop {
        if stopping.is_none()
            && let Some(at) = *terminal.hangup_at.lock().unwrap()
        {
            stopping = Some(at);
            foreground = terminal
                .hungup_foreground
                .lock()
                .unwrap()
                .or_else(|| terminal.foreground_group().filter(|group| *group != pid));
        }
        if exited.is_none() {
            let mut owned = terminal.pid.lock().unwrap();
            exited = workers.child.as_mut().unwrap().try_wait()?;
            if exited.is_some() {
                *owned = None;
            }
        }
        if stopping.is_none()
            && exited.is_none()
            && (terminal.closing.load(Ordering::Acquire)
                || controls.try_recv().is_ok()
                || controls.is_closed()
                || env.closing.is_closing())
        {
            foreground = terminal.foreground_group().filter(|group| *group != pid);
            terminal.hangup();
            stopping = *terminal.hangup_at.lock().unwrap();
        }
        let grace = Duration::from_secs(if env.closing.is_closing() { 1 } else { 3 });
        if let Some(at) = stopping {
            workers.deadline = Some(at + grace);
        }
        let expired = stopping.is_some_and(|at| at.elapsed() >= grace);
        if let Some(group) = take_foreground_kill(&mut foreground, expired, |group| {
            platform::group_exists(group).unwrap_or(false)
        }) {
            let _ = platform::signal_group(group, Signal::Kill);
        }
        if expired && exited.is_none() && !leader_killed {
            leader_killed = true;
            if platform::group_exists(pid).unwrap_or(false) {
                let _ = platform::signal_group(pid, Signal::Kill);
            }
        }
        if foreground.is_none()
            && let Some(status) = exited.take()
        {
            drop(workers.child.take());
            let reader = workers.reader.take();
            workers.finish(reader, "reader");
            *terminal.status.lock().unwrap() = ShellStatus::from_exit(status);
            break;
        }
        if stopping.is_none()
            && workers
                .writer
                .as_ref()
                .is_some_and(|writer| writer.is_finished())
        {
            return Err(ProviderError::Protocol("The terminal input stopped".into()));
        }
        let reply = {
            let mut emulator = terminal.emulator.lock();
            if emulator.flush_sync_timeout() {
                terminal.dirty.store(true, Ordering::Release);
                emulator.feed(&[], 0)
            } else {
                vec![]
            }
        };
        if !reply.is_empty() {
            terminal.write(&reply)?;
        }
        if terminal.dirty.swap(false, Ordering::AcqRel) {
            terminal.wake.send_modify(|counter| *counter += 1);
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    Ok(())
}
fn write(
    master: Box<dyn MasterPty + Send>,
    mut writer: Box<dyn Write + Send>,
    queue: std::sync::mpsc::Receiver<Input>,
    terminal: &Terminal,
) -> io::Result<()> {
    for input in queue {
        match input {
            Input::Write(bytes) => {
                writer.write_all(&bytes)?;
                writer.flush()?;
            }
            Input::Resize(size) => {
                master.resize(size).map_err(io::Error::other)?;
                terminal.emulator.lock().term.resize(Size(size));
                terminal.dirty.store(true, Ordering::Release);
            }
        }
    }
    Ok(())
}
fn read(mut reader: Box<dyn Read + Send>, terminal: &Terminal) -> io::Result<()> {
    let mut bytes = [0; 64 * 1024];
    #[cfg(debug_assertions)]
    let mut consumed = 0;
    loop {
        let count = match reader.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        #[cfg(debug_assertions)]
        let reply = {
            consumed += count as u64;
            terminal.emulator.lock().feed(&bytes[..count], consumed)
        };
        #[cfg(not(debug_assertions))]
        let reply = terminal.emulator.lock().feed(&bytes[..count], 0);
        if !reply.is_empty() {
            terminal.write(&reply)?;
        }
        terminal.dirty.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_shell_spawn_has_a_plain_start_reason() {
        let root = std::path::Path::new("/tmp").join(format!(
            "veyra-shell-spawn-{}",
            crate::domain::ThreadId::new()
        ));
        std::fs::create_dir(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let env =
            ChildEnv::isolated().with_variable("SHELL", root.join("missing-shell").as_os_str());
        let handle = spawn(root, PtySize::default(), env, Colors::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.status().is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            handle.status(),
            ShellStatus::Failed {
                error:
                    "The shell could not start: the selected shell is missing or is not executable"
                        .into()
            }
        );
        assert!(handle.pid().is_none());
    }

    #[test]
    fn failure_after_shell_started_keeps_runtime_words() {
        let root = std::path::Path::new("/tmp").join(format!(
            "veyra-shell-input-{}",
            crate::domain::ThreadId::new()
        ));
        std::fs::create_dir(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let shell = root.join("shell");
        std::fs::write(&shell, "#!/bin/sh\nexec /bin/sleep 60\n").unwrap();
        assert!(
            std::process::Command::new("/bin/chmod")
                .arg("700")
                .arg(&shell)
                .status()
                .unwrap()
                .success()
        );
        let env = ChildEnv::isolated().with_variable("SHELL", shell.as_os_str());
        let handle = spawn(root, PtySize::default(), env, Colors::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.status() != ShellStatus::Running {
            assert!(
                Instant::now() < deadline,
                "shell did not start: {:?}",
                handle.status()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let pid = handle.pid().unwrap();
        handle.terminal.input.lock().unwrap().take();
        while !handle.status().is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            handle.status(),
            ShellStatus::Failed {
                error: "The terminal input stopped".into()
            }
        );
        assert!(!platform::is_alive(pid).unwrap());
    }

    #[test]
    fn child_group_signals_only_before_reaping() {
        for reaped in [false, true] {
            let mut command = tokio::process::Command::new("/bin/sleep");
            command.arg("60");
            platform::own_group(&mut command);
            let child = command.as_std_mut().spawn().unwrap();
            let pid = child.id();
            let groups = Arc::new(Mutex::new([pid].into_iter().collect()));
            let mut guard = ChildGroup {
                child: Box::new(child),
                pid,
                groups: groups.clone(),
                reaped: false,
            };
            if reaped {
                platform::signal_group(pid, Signal::Kill).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while guard.try_wait().unwrap().is_none() {
                    assert!(Instant::now() < deadline);
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            let mut signals = 0;
            guard.finish(|group| {
                signals += 1;
                assert_eq!(group, pid);
                assert!(platform::is_alive(pid).unwrap());
                platform::signal_group(group, Signal::Kill).unwrap();
            });
            assert_eq!(signals, usize::from(!reaped));
            drop(guard);
            assert!(groups.lock().unwrap().is_empty());
            assert!(!platform::is_alive(pid).unwrap());
        }
    }

    #[test]
    fn foreground_grace_skips_group_gone_at_deadline() {
        let mut group = Some(44);
        assert_eq!(take_foreground_kill(&mut group, true, |_| false), None);
        assert_eq!(group, None);
    }
    #[test]
    fn foreground_grace_forgets_gone_groups_and_takes_survivors_once() {
        let mut group = Some(42);
        assert_eq!(take_foreground_kill(&mut group, false, |_| true), None);
        assert_eq!(group, Some(42));
        assert_eq!(take_foreground_kill(&mut group, false, |_| false), None);
        assert_eq!(group, None);
        assert_eq!(
            take_foreground_kill(&mut group, true, |_| panic!("revisited a gone group")),
            None
        );
        let mut group = Some(43);
        assert_eq!(
            take_foreground_kill(&mut group, true, |id| id == 43),
            Some(43)
        );
        assert_eq!(group, None);
        assert_eq!(
            take_foreground_kill(&mut group, true, |_| panic!("repeated a grace kill")),
            None
        );
    }

    #[test]
    fn shell_exit_outcomes_are_distinct() {
        assert_eq!(
            ShellStatus::from_exit(portable_pty::ExitStatus::with_exit_code(7)),
            ShellStatus::Exited { code: 7 }
        );
        assert_eq!(
            ShellStatus::from_exit(portable_pty::ExitStatus::with_signal("Hangup")),
            ShellStatus::Signalled
        );
    }
    #[cfg(debug_assertions)]
    #[test]
    fn flood_stamps_match_consumed_title_boundaries() {
        let mut emulator = Emulator::new(PtySize::default());
        let bytes = b"x\x1b]0;FLOOD_START\x07one\r\n\x1b]0;FLOOD_DONE\x07";
        assert_eq!(bytes.len(), 37);
        emulator.feed(bytes, 37);
        let start = emulator.flood.start.unwrap();
        let done = emulator.flood.done.unwrap();
        assert_eq!(start.bytes, 17);
        assert_eq!(done.bytes, 37);
        assert!(done.at >= start.at);
        assert_eq!(&emulator.text()[..2], &["xone", ""]);
    }
    #[test]
    fn emulator_answers_queries_with_supplied_colors() {
        let mut emulator = Emulator::with_colors(
            PtySize::default(),
            Colors {
                foreground: Rgb { r: 1, g: 2, b: 3 },
                background: Rgb { r: 4, g: 5, b: 6 },
            },
        );
        let query = b"\x1b]10;?\x07\x1b]11;?\x07\x1b]4;196;?\x07\x1b[?2026$p\x1b[6n\x1b[c";
        let reply = emulator.feed(query, query.len() as u64);
        assert_eq!(
            reply,
            b"\x1b]10;rgb:0101/0202/0303\x07\x1b]11;rgb:0404/0505/0606\x07\x1b]4;196;rgb:ffff/000\
            0/0000\x07\x1b[?2026;2$y\x1b[1;1R\x1b[?6c"
        );
        assert!(
            !emulator
                .term
                .mode()
                .contains(alacritty_terminal::term::TermMode::DISAMBIGUATE_ESC_CODES)
        );
        assert_eq!(emulator.feed(b"\x1b[>0q\x1b]7501;?\x07", 15), b"");
    }
    #[test]
    fn emulator_is_send() {
        fn send<T: Send>() {}
        send::<Emulator>();
        send::<Terminal>();
        let emulator = std::thread::spawn(|| {
            let mut emulator = Emulator::new(PtySize::default());
            assert_eq!(
                emulator.feed(b"\x1b[2;4HOK\x1b[6n\x1b[c", 16),
                b"\x1b[2;6R\x1b[?6c"
            );
            emulator
        })
        .join()
        .unwrap();
        assert_eq!(emulator.text()[1], "   OK");
    }
}
