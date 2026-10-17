use crate::{
    actor::EngineMsg,
    platform::{self, Signal},
    provider::{
        ProviderError, ProviderKind, SessionSpec, TerminalControl, TerminalEvent, TerminalHandle,
        env::ChildEnv,
    },
};
pub use alacritty_terminal::vte::ansi::Rgb;
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::Dimensions,
    index::{Column, Line},
    sync::FairMutex,
    term::{Config, Term},
    vte::ansi::Processor,
};
pub use portable_pty::PtySize;
use portable_pty::{CommandBuilder, MasterPty};
use std::{
    io::{self, Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
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
            256 | 258 | 267 => self.foreground,
            257 | 268 => self.background,
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
}

pub struct Emulator {
    pub term: Term<Listener>,
    parser: Processor,
    colors: Colors,
    listener: Listener,
    pub title: Option<String>,
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
            title: None,
            #[cfg(debug_assertions)]
            flood: FloodStamps::default(),
        }
    }
    fn feed(&mut self, bytes: &[u8], consumed: u64) -> Vec<u8> {
        let mut reply = Vec::new();
        #[cfg(debug_assertions)]
        let mut parsed = consumed - bytes.len() as u64;
        for part in bytes
            .split_inclusive(|byte| *byte == 7 || *byte == b'\\')
            .chain(bytes.is_empty().then_some(&[][..]))
        {
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
                        self.title = Some(title);
                    }
                    Event::ResetTitle => self.title = None,
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
    pub fn text(&self) -> Vec<String> {
        (0..self.term.screen_lines())
            .map(|row| {
                let line = &self.term.grid()[Line(row as i32)];
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

struct PtyIo {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}
pub struct Terminal {
    pub emulator: FairMutex<Emulator>,
    io: Mutex<Option<PtyIo>>,
    dirty: AtomicBool,
    pid: AtomicU32,
    hangup_sent: AtomicBool,
    wake: watch::Sender<u64>,
}
impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal").finish_non_exhaustive()
    }
}
impl Terminal {
    pub(crate) fn pid(&self) -> u32 {
        self.pid.load(Ordering::Acquire)
    }
    pub(crate) fn hangup(&self) {
        if !self.hangup_sent.swap(true, Ordering::AcqRel) {
            let _ = platform::signal_group(self.pid(), Signal::Hangup);
        }
    }
    pub fn write(&self, bytes: &[u8]) -> io::Result<()> {
        let writer = self
            .io
            .lock()
            .unwrap()
            .as_ref()
            .map(|io| io.writer.clone())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotConnected, "terminal is not running")
            })?;
        let mut writer = writer.lock().unwrap();
        writer.write_all(bytes)?;
        writer.flush()
    }
    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        if size.cols < 2 || size.rows == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "terminal needs at least two columns and one row",
            ));
        }
        let io = self.io.lock().unwrap();
        io.as_ref()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, "terminal is not running"))?
            .master
            .resize(size)
            .map_err(io::Error::other)?;
        self.emulator.lock().term.resize(Size(size));
        self.dirty.store(true, Ordering::Release);
        Ok(())
    }
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.wake.subscribe()
    }
}

pub(crate) fn spawn(
    provider: ProviderKind,
    spec: SessionSpec,
    env: ChildEnv,
    out: mpsc::Sender<EngineMsg>,
    epoch: u32,
    colors: Colors,
) -> Result<TerminalHandle, ProviderError> {
    if spec.native.is_none() {
        return Err(ProviderError::Protocol(
            "Start the conversation in chat before opening its terminal".into(),
        ));
    }
    let size = PtySize::default();
    let terminal = Arc::new(Terminal {
        emulator: FairMutex::new(Emulator::with_colors(size, colors)),
        io: Mutex::new(None),
        dirty: AtomicBool::new(false),
        pid: AtomicU32::new(0),
        hangup_sent: AtomicBool::new(false),
        wake: watch::channel(0).0,
    });
    let (control, mut controls) = mpsc::unbounded_channel();
    let handle = TerminalHandle {
        control,
        terminal: Some(terminal.clone()),
    };
    let runtime = tokio::runtime::Handle::current();
    std::thread::Builder::new().name("veyra-pty".into()).spawn(move || {
        let thread = spec.thread;
        let result = (|| {
            let cursor = runtime.block_on(capture_cursor(provider, spec.clone(), env.clone(), epoch, &mut controls))?;
            if controls.try_recv().is_ok() || controls.is_closed() { return Err(ProviderError::Cancelled); }
            let (reply, prepared) = std::sync::mpsc::sync_channel(1);
            out.blocking_send(EngineMsg::Terminal { thread, epoch, event: TerminalEvent::Prepared { cursor, reply } }).map_err(|_| ProviderError::Cancelled)?;
            loop {
                match prepared.recv_timeout(Duration::from_millis(25)) {
                    Ok(true) => break,
                    Ok(false) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(ProviderError::Cancelled),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) if env.closing.is_closing() || out.is_closed() => return Err(ProviderError::Cancelled),
                    Err(_) => {}
                }
            }
            run(provider, spec, env, &out, epoch, terminal, controls)
        })();
        if let Err(error) = result {
            tracing::warn!(error = %crate::record::redact(&error.to_string(), None), "terminal ended with an error");
        }
        let _ = out.blocking_send(EngineMsg::Terminal { thread, epoch, event: TerminalEvent::Exited });
    })?;
    Ok(handle)
}

async fn capture_cursor(
    provider: ProviderKind,
    mut spec: SessionSpec,
    env: ChildEnv,
    epoch: u32,
    controls: &mut mpsc::UnboundedReceiver<TerminalControl>,
) -> Result<Option<String>, ProviderError> {
    if provider == ProviderKind::Claude {
        return tokio::task::spawn_blocking(move || {
            super::provider::claude::session_file_import(
                &env.claude_config_dir()?,
                spec.native.as_ref().unwrap(),
                &Default::default(),
            )
            .map(|history| history.cursor)
        })
        .await
        .map_err(|error| ProviderError::Protocol(error.to_string()))?;
    }
    spec.import = true;
    spec.root = false;
    spec.history.get_or_insert_default().cursor = None;
    let (out, mut events) = mpsc::channel(256);
    let handle = super::provider::session::spawn_provider(provider, spec, env.clone(), out, epoch)?;
    let mut history = None;
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut failure = None;
    loop {
        tokio::select! {
            message = events.recv() => match message {
                Some(EngineMsg::Provider { event: super::provider::ProviderEvent::Session(event), .. }) => match event {
                    super::provider::SessionEvent::History { history: value, .. } => history = Some(value),
                    super::provider::SessionEvent::Ready { .. } => { let _ = handle.control.send(super::provider::SessionControl::Shutdown); }
                    super::provider::SessionEvent::Exited { crash } => {
                        if let Some(error) = failure.or(crash) { return Err(ProviderError::Protocol(error)); }
                        return history.map(|history| history.cursor).ok_or_else(|| ProviderError::Protocol("Provider exited before history capture".into()));
                    }
                    _ => {}
                },
                None => return Err(ProviderError::Protocol("History capture ended without exit".into())),
                _ => {}
            },
            _ = tokio::time::sleep(Duration::from_millis(25)) => {
                if failure.is_none() && (Instant::now() >= deadline || env.closing.is_closing() || controls.try_recv().is_ok() || controls.is_closed()) {
                    failure = Some("History capture cancelled or timed out".into());
                    let _ = handle.control.send(super::provider::SessionControl::Shutdown);
                }
            }
        }
    }
}

struct ChildGroup {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    pid: u32,
    groups: crate::provider::env::Groups,
}
impl Drop for ChildGroup {
    fn drop(&mut self) {
        let _ = platform::signal_group(self.pid, Signal::Kill);
        let _ = self.child.wait();
        self.groups.lock().unwrap().remove(&self.pid);
    }
}
fn run(
    provider: ProviderKind,
    spec: SessionSpec,
    env: ChildEnv,
    out: &mpsc::Sender<EngineMsg>,
    epoch: u32,
    terminal: Arc<Terminal>,
    mut controls: mpsc::UnboundedReceiver<TerminalControl>,
) -> Result<(), ProviderError> {
    let binary = env.resolve(provider)?;
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize::default())
        .map_err(io::Error::other)?;
    let mut command = CommandBuilder::new(binary);
    command.args(provider.tui_command(spec.native.as_ref().unwrap(), &spec.config));
    command.cwd(&spec.cwd);
    command.env_clear();
    for (key, value) in env.tui_environment(std::env::vars_os()) {
        command.env(key, value);
    }
    #[cfg(debug_assertions)]
    if let Some(root) = &env.fixtures {
        let (n, spawn) = env.next_lane(spec.thread);
        let fixture = root.join(format!("t{n}-s{spawn}.jsonl"));
        if !fixture.is_file() {
            return Err(ProviderError::FixtureMissing(fixture));
        }
        command.env("VEYRA_FAKE_FIXTURE", std::fs::canonicalize(fixture)?);
    }
    let admission = env.closing.enter();
    if env.closing.is_closing() {
        return Err(ProviderError::Cancelled);
    }
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(io::Error::other)?;
    let pid = child
        .process_id()
        .ok_or_else(|| io::Error::other("PTY child has no pid"))?;
    env.groups.lock().unwrap().insert(pid);
    terminal.pid.store(pid, Ordering::Release);
    let mut child = ChildGroup {
        child,
        pid,
        groups: env.groups.clone(),
    };
    drop(admission);
    drop(pair.slave);
    if let Err(error) = platform::lower_priority(pid) {
        tracing::warn!(%error, "could not lower terminal priority");
    }
    let reader = pair.master.try_clone_reader().map_err(io::Error::other)?;
    let writer = Arc::new(Mutex::new(
        pair.master.take_writer().map_err(io::Error::other)?,
    ));
    let reading = terminal.clone();
    let replies = writer.clone();
    let reader = std::thread::Builder::new()
        .name("veyra-pty-reader".into())
        .spawn(move || {
            let result = read(reader, &reading, &replies);
            if let Err(error) = result {
                tracing::warn!(%error, "terminal reader stopped");
            }
        })?;
    *terminal.io.lock().unwrap() = Some(PtyIo {
        master: pair.master,
        writer,
    });
    let _ = out.blocking_send(EngineMsg::Terminal {
        thread: spec.thread,
        epoch,
        event: TerminalEvent::Ready { pid },
    });
    let result = (|| -> io::Result<()> {
        let mut stopping = None;
        loop {
            if child.child.try_wait()?.is_some() {
                break;
            }
            if stopping.is_none()
                && (controls.try_recv().is_ok()
                    || controls.is_closed()
                    || env.closing.is_closing()
                    || out.is_closed())
            {
                terminal.hangup();
                stopping = Some(Instant::now());
            }
            let stop_grace = Duration::from_secs(3);
            if stopping.is_some_and(|at: Instant| at.elapsed() >= stop_grace) {
                let _ = platform::signal_group(pid, Signal::Kill);
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
    })();
    drop(child);
    terminal.pid.store(0, Ordering::Release);
    *terminal.io.lock().unwrap() = None;
    let joined = reader
        .join()
        .map_err(|_| io::Error::other("terminal reader panicked"));
    terminal.wake.send_modify(|counter| *counter += 1);
    result?;
    joined?;
    Ok(())
}
fn read(
    mut reader: Box<dyn Read + Send>,
    terminal: &Terminal,
    writer: &Mutex<Box<dyn Write + Send>>,
) -> io::Result<()> {
    let mut bytes = [0; 64 * 1024];
    let mut consumed = 0;
    loop {
        let count = match reader.read(&mut bytes) {
            Ok(0) => return Ok(()),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        consumed += count as u64;
        let reply = terminal.emulator.lock().feed(&bytes[..count], consumed);
        if !reply.is_empty() {
            let mut writer = writer.lock().unwrap();
            writer.write_all(&reply)?;
            writer.flush()?;
        }
        terminal.dirty.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(emulator.title.as_deref(), Some("FLOOD_DONE"));
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
        assert_eq!(reply, b"\x1b]10;rgb:0101/0202/0303\x07\x1b]11;rgb:0404/0505/0606\x07\x1b]4;196;rgb:ffff/0000/0000\x07\x1b[?2026;2$y\x1b[1;1R\x1b[?6c");
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
                emulator.feed(b"\x1b[2;4HM7\x1b[6n\x1b[c", 16),
                b"\x1b[2;6R\x1b[?6c"
            );
            emulator
        })
        .join()
        .unwrap();
        assert_eq!(emulator.text()[1], "   M7");
    }
}
