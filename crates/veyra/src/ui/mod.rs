pub mod actions;
mod composer;
mod header;
mod new_thread;
mod settings;
mod sidebar;
pub mod theme;
mod transcript;

use actions::*;
use gpui_kit::component::{
    StyledExt,
    input::{InputEvent, TextareaState},
    message_scroller::MessageScrollerState,
};
use gpui_kit::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Instant,
};
use theme::{Appearance, Theme};
use veyra_engine::{
    Engine,
    commands::{Ack, Command},
    domain::*,
    events::{ChangeEvent, ThreadSnapshot, UiEvent},
    live::LiveState,
    provider::{Probe, ProviderKind, Readiness},
};

enum Fence {
    Loading(Vec<UiEvent>),
    Live(u64),
}
#[derive(Default)]
struct AppState {
    projects: BTreeMap<ProjectId, Project>,
    threads: BTreeMap<ThreadId, Thread>,
    turns: BTreeMap<TurnId, Turn>,
    items: BTreeMap<ThreadId, BTreeMap<ItemId, Item>>,
    live: BTreeMap<ThreadId, LiveState>,
    fences: BTreeMap<ThreadId, Fence>,
    settings: Settings,
    applied_batches: u64,
    max_batch_gap_ms: f64,
    last_text_batch: Option<Instant>,
}
fn event_thread(event: &UiEvent) -> Option<(ThreadId, u64)> {
    match event {
        UiEvent::Change {
            counter: Some(counter),
            event,
        } => event.thread().map(|thread| (thread, *counter)),
        UiEvent::TextAppended {
            thread, counter, ..
        } => Some((*thread, *counter)),
        _ => None,
    }
}
impl AppState {
    fn apply_all(&mut self, events: Vec<UiEvent>) {
        self.applied_batches += 1;
        if events.iter().any(|event| {
            matches!(
                event,
                UiEvent::Change {
                    event: ChangeEvent::TurnStarted(_),
                    ..
                }
            )
        }) {
            self.last_text_batch = None;
        }
        if events
            .iter()
            .any(|event| matches!(event, UiEvent::TextAppended { .. }))
        {
            let now = Instant::now();
            if let Some(last) = self.last_text_batch {
                self.max_batch_gap_ms = self
                    .max_batch_gap_ms
                    .max(now.duration_since(last).as_secs_f64() * 1000.);
            }
            self.last_text_batch = Some(now);
        }
        for event in events {
            self.apply(event);
        }
    }
    fn apply(&mut self, event: UiEvent) {
        if let UiEvent::Change { event, .. } = &event {
            match event {
                ChangeEvent::ProjectAdded(project) => {
                    self.projects.insert(project.id, project.clone());
                }
                ChangeEvent::ThreadCreated(thread) => {
                    self.threads.insert(thread.id, thread.clone());
                }
                ChangeEvent::ThreadArchived { thread } => {
                    if let Some(row) = self.threads.get_mut(thread) {
                        row.archived = true;
                    }
                }
                ChangeEvent::ThreadConfigured { thread, config } => {
                    if let Some(row) = self.threads.get_mut(thread) {
                        row.config = config.clone();
                    }
                }
                ChangeEvent::ThreadHoldChanged { thread, hold } => {
                    if let Some(row) = self.threads.get_mut(thread) {
                        row.hold = *hold;
                    }
                }
                ChangeEvent::ThreadLiveChanged { thread, state } => {
                    self.live.insert(*thread, *state);
                }
                ChangeEvent::NativeSessionBound {
                    thread,
                    native,
                    cli_version,
                } => {
                    if let Some(row) = self.threads.get_mut(thread) {
                        row.native = Some(native.clone());
                        row.cli_version = Some(cli_version.clone());
                    }
                }
                ChangeEvent::TurnQueued(turn)
                | ChangeEvent::TurnStarted(turn)
                | ChangeEvent::TurnEnded(turn) => {
                    self.turns.insert(turn.id, turn.clone());
                }
                ChangeEvent::SettingsChanged(settings) => self.settings = *settings,
                ChangeEvent::ItemAdded(_)
                | ChangeEvent::ItemCompleted(_)
                | ChangeEvent::RequestOpened(_)
                | ChangeEvent::RequestClosed(_)
                | ChangeEvent::TaskStarted(_)
                | ChangeEvent::TaskSettled(_)
                | ChangeEvent::TaskDelivered(_) => {}
            }
        }
        if !matches!(
            &event,
            UiEvent::TextAppended { .. }
                | UiEvent::Change {
                    event: ChangeEvent::ItemAdded(_) | ChangeEvent::ItemCompleted(_),
                    ..
                }
        ) {
            return;
        }
        if let Some((thread, counter)) = event_thread(&event) {
            match self.fences.get_mut(&thread) {
                Some(Fence::Loading(buffer)) => {
                    buffer.push(event);
                    return;
                }
                Some(Fence::Live(applied)) if counter <= *applied => return,
                Some(Fence::Live(applied)) => *applied = counter,
                None => {
                    self.fences.insert(thread, Fence::Live(counter));
                }
            }
        }
        self.apply_transcript(event);
    }
    fn apply_transcript(&mut self, event: UiEvent) {
        match event {
            UiEvent::Change {
                event: ChangeEvent::ItemAdded(item) | ChangeEvent::ItemCompleted(item),
                ..
            } => {
                self.items
                    .entry(item.thread)
                    .or_default()
                    .insert(item.id, item);
            }
            UiEvent::TextAppended {
                thread, item, text, ..
            } => {
                if let Some(item) = self
                    .items
                    .get_mut(&thread)
                    .and_then(|items| items.get_mut(&item))
                    && let ItemBody::AssistantText { text: body, .. }
                    | ItemBody::Reasoning { text: body, .. } = &mut item.body
                {
                    body.push_str(&text);
                }
            }
            _ => {}
        }
    }
    fn begin_snapshot(&mut self, thread: ThreadId) {
        self.fences.insert(thread, Fence::Loading(vec![]));
    }
    fn finish_snapshot(&mut self, thread: ThreadId, snapshot: ThreadSnapshot) {
        let buffered = match self.fences.remove(&thread) {
            Some(Fence::Loading(buffer)) => buffer,
            _ => return,
        };
        let mut items = BTreeMap::new();
        for mut item in snapshot.items {
            if let Some(text) = snapshot.buffered_text.get(&item.id)
                && let ItemBody::AssistantText { text: body, .. }
                | ItemBody::Reasoning { text: body, .. } = &mut item.body
            {
                body.push_str(text);
            }
            items.insert(item.id, item);
        }
        self.items.insert(thread, items);
        self.fences.insert(thread, Fence::Live(snapshot.counter));
        for event in buffered {
            if let Some((_, counter)) = event_thread(&event)
                && counter > snapshot.counter
            {
                self.apply_transcript(event);
                self.fences.insert(thread, Fence::Live(counter));
            }
        }
    }
    fn transcript(&self, thread: ThreadId) -> Vec<Item> {
        let mut items: Vec<_> = self
            .items
            .get(&thread)
            .into_iter()
            .flat_map(|items| items.values())
            .cloned()
            .collect();
        items.sort_by_key(|item| {
            (
                self.turns
                    .get(&item.turn)
                    .map(|turn| turn.ordinal)
                    .unwrap_or_default(),
                item.seq,
            )
        });
        items
    }
    fn status(&self, id: ThreadId) -> Status {
        let row = &self.threads[&id];
        let turns = self.turns.values().filter(|turn| turn.thread == id);
        if row.hold.is_some() && turns.clone().any(|turn| turn.state == TurnState::Queued) {
            return Status::Held;
        }
        if let Some(turn) = turns
            .clone()
            .find(|turn| matches!(turn.state, TurnState::Running { .. }))
        {
            return Status::Working {
                since: turn.state.started().unwrap_or_default(),
            };
        }
        if matches!(
            self.live.get(&id),
            Some(LiveState::Starting | LiveState::Running)
        ) {
            return Status::Working { since: 0 };
        }
        match turns
            .max_by_key(|turn| turn.ordinal)
            .map(|turn| &turn.state)
        {
            Some(TurnState::Ended {
                outcome: TurnOutcome::Completed,
                started,
                at,
            }) => Status::Done {
                took: started
                    .map(|start| at.saturating_sub(start))
                    .unwrap_or_default(),
            },
            Some(TurnState::Ended {
                outcome: TurnOutcome::Failed { .. },
                ..
            }) => Status::Failed,
            Some(TurnState::Ended {
                outcome: TurnOutcome::Interrupted { .. },
                ..
            }) => Status::Interrupted,
            _ => Status::Idle,
        }
    }
    fn failure(&self, thread: ThreadId) -> Option<&str> {
        self.turns
            .values()
            .filter(|turn| turn.thread == thread)
            .max_by_key(|turn| turn.ordinal)
            .and_then(|turn| match &turn.state {
                TurnState::Ended {
                    outcome: TurnOutcome::Failed { message },
                    ..
                } => Some(message.as_str()),
                _ => None,
            })
    }
}
fn status_name(status: &Status) -> &'static str {
    match status {
        Status::NeedsYou => "NeedsYou",
        Status::Failed => "Failed",
        Status::Held => "Held",
        Status::Interrupted => "Interrupted",
        Status::Working { .. } => "Working",
        Status::InTerminal => "InTerminal",
        Status::Done { .. } => "Done",
        Status::Idle => "Idle",
    }
}
#[derive(Clone, Copy, PartialEq)]
enum MainView {
    NewThread,
    Chat,
    Settings,
}
pub struct Shell {
    focus: FocusHandle,
    sidebar_width: Pixels,
    resizing: bool,
    notice: Option<SharedString>,
    engine: Engine,
    runtime: tokio::runtime::Handle,
    state: AppState,
    view: MainView,
    selected: Option<ThreadId>,
    project: Option<ProjectId>,
    provider: ProviderKind,
    config: ThreadConfig,
    providers: Vec<Probe>,
    input: Entity<TextareaState>,
    scroller: Entity<MessageScrollerState>,
    expanded: BTreeSet<ItemId>,
    branch: String,
    _input_subscription: Subscription,
}
impl Shell {
    fn new(
        engine: Engine,
        runtime: tokio::runtime::Handle,
        mut events: tokio::sync::mpsc::UnboundedReceiver<UiEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Describe the task")
                .auto_grow(2, 10)
                .submit_on_enter(true)
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        let subscription =
            cx.subscribe_in(&input, window, |_, _, event: &InputEvent, window, cx| {
                if matches!(
                    event,
                    InputEvent::PressEnter {
                        secondary: false,
                        shift: false
                    }
                ) {
                    window.dispatch_action(Box::new(Send), cx);
                }
            });
        cx.spawn(async move |this, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                if this
                    .update(cx, |shell, cx| {
                        let old_default = shell.state.settings.default_mode;
                        for event in &batch {
                            if let UiEvent::StorageBlocked { error } = event {
                                shell.notice = error.clone().map(Into::into);
                            }
                        }
                        shell.state.apply_all(batch);
                        if shell.project.is_none() {
                            shell.project = shell.state.projects.keys().next().copied();
                        }
                        if old_default != shell.state.settings.default_mode
                            && shell.view != MainView::Chat
                        {
                            shell.config.mode = shell.state.settings.default_mode;
                        }
                        let count = shell
                            .selected
                            .map(|thread| shell.state.transcript(thread).len())
                            .unwrap_or_default();
                        shell.scroller.update(cx, |state, cx| {
                            if count > state.item_count() {
                                state.append(count - state.item_count(), cx);
                            } else if count < state.item_count() {
                                state.reset(count, cx);
                            }
                            state.remeasure(cx);
                        });
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let cadence = cx.global::<Theme>().motion.spinner.duration;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(cadence).await;
                if this
                    .update(cx, |shell, cx| {
                        if shell.selected.is_some_and(|thread| {
                            matches!(shell.state.status(thread), Status::Working { .. })
                        }) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let mut shell = Self {
            focus,
            sidebar_width: cx.global::<Theme>().layout.sidebar_default,
            resizing: false,
            notice: None,
            engine,
            runtime,
            state: AppState::default(),
            view: MainView::NewThread,
            selected: None,
            project: None,
            provider: ProviderKind::Claude,
            config: ThreadConfig {
                model: None,
                effort: None,
                mode: PermissionMode::FullAccess,
            },
            providers: vec![],
            input,
            scroller: cx.new(|cx| MessageScrollerState::new(0, cx)),
            expanded: BTreeSet::new(),
            branch: String::new(),
            _input_subscription: subscription,
        };
        shell.recheck(false, window, cx);
        shell
    }
    fn notice(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        tracing::info!(message = message.as_ref(), "window notice");
        self.notice = Some(message);
        cx.notify();
    }
    fn send_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        let reply = self.engine.send(command);
        cx.spawn_in(window, async move |this, cx| {
            let result = reply.await;
            let _ = this.update_in(cx, |shell, window, cx| match result {
                Ok(Ok(Ack::Project(id))) => {
                    shell.project = Some(id);
                    shell.notice = None;
                    cx.notify();
                }
                Ok(Ok(Ack::Thread(id))) => {
                    shell.select_thread(id, window, cx);
                    shell
                        .input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                }
                Ok(Ok(Ack::Turn(_))) => {
                    shell
                        .input
                        .update(cx, |input, cx| input.set_value("", window, cx));
                    shell.notice = None;
                    cx.notify();
                }
                Ok(Ok(Ack::Applied)) => {
                    shell.notice = None;
                    cx.notify();
                }
                Ok(Err(error)) => shell.notice(error.to_string(), cx),
                Err(_) => shell.notice("The engine closed before answering", cx),
            });
        })
        .detach();
    }
    fn select_thread(&mut self, thread: ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        self.view = MainView::Chat;
        self.selected = Some(thread);
        self.notice = None;
        self.state.begin_snapshot(thread);
        let reply = self.engine.snapshot(thread);
        self.scroller.update(cx, |state, cx| state.reset(0, cx));
        self.input.update(cx, |input, cx| input.focus(window, cx));
        let cwd = self.state.threads.get(&thread).map(|row| row.cwd.clone());
        let branch = cx.background_executor().spawn(async move {
            cwd.and_then(|cwd| {
                std::process::Command::new("git")
                    .args(["branch", "--show-current"])
                    .current_dir(cwd)
                    .output()
                    .ok()
            })
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let snapshot = reply.await;
            let branch = branch.await;
            let _ = this.update(cx, |shell, cx| {
                if let Ok(snapshot) = snapshot {
                    shell.state.finish_snapshot(thread, snapshot);
                } else {
                    shell.notice = Some("The transcript could not be loaded".into());
                }
                if shell.selected == Some(thread) {
                    shell.branch = branch;
                    let count = shell.state.transcript(thread).len();
                    shell
                        .scroller
                        .update(cx, |state, cx| state.reset(count, cx));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn recheck(&mut self, retry: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.providers.clear();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn(async move {
            let env = veyra_engine::provider::env::ChildEnv::load();
            let mut probes = Vec::new();
            for provider in [
                ProviderKind::Claude,
                ProviderKind::Codex,
                ProviderKind::Grok,
            ] {
                probes.push(provider.probe(&env).await);
            }
            let _ = tx.send(probes);
        });
        let retry_thread = retry.then_some(self.selected).flatten();
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(probes) = rx.await {
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.providers = probes;
                    if let Some(thread) = retry_thread
                        && shell.ready(shell.state.threads[&thread].provider)
                    {
                        let text =
                            shell.state.transcript(thread).iter().rev().find_map(
                                |item| match &item.body {
                                    ItemBody::UserText(text) => Some(text.clone()),
                                    _ => None,
                                },
                            );
                        if let Some(text) = text {
                            if shell.state.threads[&thread].hold.is_some() {
                                shell.send_command(
                                    Command::Continue {
                                        thread,
                                        drop_queued: false,
                                    },
                                    window,
                                    cx,
                                );
                            }
                            shell.send_command(Command::Send { thread, text }, window, cx);
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
        cx.notify();
    }
    fn ready(&self, provider: ProviderKind) -> bool {
        self.providers
            .iter()
            .any(|probe| probe.provider == provider && probe.readiness == Readiness::Ready)
    }
    fn composer_config(&self) -> &ThreadConfig {
        self.selected
            .filter(|_| self.view == MainView::Chat)
            .and_then(|id| self.state.threads.get(&id))
            .map(|row| &row.config)
            .unwrap_or(&self.config)
    }
    fn chosen_provider(&self) -> ProviderKind {
        self.selected
            .filter(|_| self.view == MainView::Chat)
            .and_then(|id| self.state.threads.get(&id))
            .map(|row| row.provider)
            .unwrap_or(self.provider)
    }
    fn disabled_reason(&self) -> Option<String> {
        if self.view == MainView::Settings {
            return Some("Close Settings to send a message".into());
        }
        if self.project.is_none() && self.view == MainView::NewThread {
            return Some("Add a project to start a thread".into());
        }
        if !self.ready(self.chosen_provider()) {
            let provider = self.chosen_provider();
            return Some(
                match self
                    .providers
                    .iter()
                    .find(|probe| probe.provider == provider)
                    .map(|probe| &probe.readiness)
                {
                    Some(Readiness::NotInstalled) => format!(
                        "{} is not installed. Run npm install -g @anthropic-ai/claude-code and check again.",
                        provider.label()
                    ),
                    Some(Readiness::SignedOut) => format!(
                        "Sign in with {} and check again.",
                        provider.sign_in_command()
                    ),
                    _ => format!("Checking {}…", provider.label()),
                },
            );
        }
        None
    }
    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reason) = self.disabled_reason() {
            self.notice(reason, cx);
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            self.notice("Write a message before sending", cx);
            return;
        }
        let command = if self.view == MainView::Chat {
            Command::Send {
                thread: self.selected.expect("selected chat"),
                text,
            }
        } else {
            Command::CreateThread {
                project: self.project.expect("chosen project"),
                provider: self.provider,
                config: self.config.clone(),
                text,
            }
        };
        self.send_command(command, window, cx);
    }
    fn new_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.view = MainView::NewThread;
        self.selected = None;
        self.config.mode = self.state.settings.default_mode;
        self.notice = None;
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }
    fn add_project(&mut self, action: &AddProject, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = &action.path {
            self.send_command(Command::AddProject { path: path.into() }, window, cx);
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Add project".into()),
        });
        cx.spawn_in(window, async move |this, cx| match paths.await {
            Ok(Ok(Some(paths))) => {
                let _ = this.update_in(cx, |shell, window, cx| {
                    for path in paths {
                        shell.send_command(Command::AddProject { path }, window, cx);
                    }
                });
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |shell, cx| shell.notice(error.to_string(), cx));
            }
            _ => {}
        })
        .detach();
    }
    fn pick_permission(
        &mut self,
        action: &PickPermission,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.composer_config().mode;
        let mode = match settings::parse_mode(action.mode.as_deref(), current) {
            Ok(mode) => mode,
            Err(error) => {
                self.notice(error, cx);
                return;
            }
        };
        if self.view == MainView::Settings {
            self.send_command(Command::SetDefaultMode { mode }, window, cx);
        } else if self.view == MainView::Chat {
            let mut config = self.composer_config().clone();
            config.mode = mode;
            self.send_command(
                Command::Configure {
                    thread: self.selected.unwrap(),
                    config,
                },
                window,
                cx,
            );
        } else {
            self.config.mode = mode;
            cx.notify();
        }
    }
    #[cfg(debug_assertions)]
    pub fn insert_text(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        if self.view == MainView::Settings || !self.input.focus_handle(cx).is_focused(window) {
            anyhow::bail!("the focused element has no text input handler");
        }
        self.input.update(cx, |input, cx| {
            input.replace_text_in_range(None, text, window, cx)
        });
        Ok(())
    }
    #[cfg(debug_assertions)]
    pub fn dump(&self, data_dir: &std::path::Path, cx: &App) -> serde_json::Value {
        let mut rows = Vec::new();
        for project in self.state.projects.values() {
            rows.push(serde_json::json!({"kind":"project","id":project.id,"name":project.name,"path":project.path}));
            for thread in self
                .state
                .threads
                .values()
                .filter(|thread| thread.project == project.id && !thread.archived)
            {
                rows.push(serde_json::json!({"kind":"thread","id":thread.id,"title":thread.title,"status":status_name(&self.state.status(thread.id)),"archived":thread.archived,"mode":thread.config.mode}));
            }
        }
        let selected = self.selected.and_then(|id| self.state.threads.get(&id)).map(|thread| serde_json::json!({"id":thread.id,"title":thread.title,"native_id":thread.native.as_ref().map(|native| &native.0),"cli_version":thread.cli_version,"config":thread.config,"status":status_name(&self.state.status(thread.id)),"failure":self.state.failure(thread.id)}));
        let providers: Vec<_> = [ProviderKind::Claude,ProviderKind::Codex,ProviderKind::Grok].into_iter().map(|provider| {
            let probe = self.providers.iter().find(|probe| probe.provider == provider);
            serde_json::json!({"provider":provider,"readiness":probe.map(|probe| &probe.readiness).unwrap_or(&Readiness::Checking),"path":probe.and_then(|probe| probe.path.as_ref()),"version":probe.and_then(|probe| probe.version.as_ref()),"models":probe.map(|probe| &probe.models)})
        }).collect();
        let transcript: Vec<_> = self
            .selected
            .map(|thread| self.state.transcript(thread))
            .unwrap_or_default()
            .iter()
            .map(|item| {
                let mut row = transcript::dump_item(item);
                row["expanded"] = serde_json::json!(self.expanded.contains(&item.id));
                row
            })
            .collect();
        let reason = self.disabled_reason();
        serde_json::json!({
            "pid":std::process::id(),"build_rev":env!("VEYRA_BUILD_REV"),"data_dir":data_dir,
            "appearance":cx.global::<Theme>().appearance.name(),"view":self.view_name(),
            "sidebar":{"width":f32::from(self.sidebar_width),"rows":rows},"selected_thread":selected,
            "transcript":transcript,"composer_text":self.input.read(cx).value().to_string(),
            "composer":{"provider":self.chosen_provider(),"mode":self.composer_config().mode,"model":self.composer_config().model,"effort":self.composer_config().effort,"send_enabled":reason.is_none(),"disabled_reason":reason},
            "providers":providers,"settings":{"default_mode":self.state.settings.default_mode},
            "perf":{"applied_batches":self.state.applied_batches,"max_batch_gap_ms":self.state.max_batch_gap_ms},
            "notice":self.notice.as_ref().map(|notice| notice.as_ref())
        })
    }
    #[cfg(debug_assertions)]
    fn view_name(&self) -> &'static str {
        match self.view {
            MainView::Settings => "settings",
            MainView::NewThread => "new_thread",
            MainView::Chat
                if self
                    .selected
                    .is_some_and(|thread| self.state.failure(thread).is_some()) =>
            {
                "failed_thread"
            }
            MainView::Chat => "chat",
        }
    }
}
impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        div()
            .h_flex()
            .size_full()
            .bg(theme.colors.bg)
            .text_color(theme.colors.text)
            .font_family(theme.text.ui_font.clone())
            .text_size(theme.text.ui.size)
            .line_height(theme.text.ui.line_height)
            .track_focus(&self.focus)
            .key_context("Veyra")
            .on_action(cx.listener(|shell, action: &AddProject, window, cx| {
                shell.add_project(action, window, cx)
            }))
            .on_action(cx.listener(|shell, _: &NewThread, window, cx| shell.new_thread(window, cx)))
            .on_action(cx.listener(|shell, _: &Send, window, cx| shell.send(window, cx)))
            .on_action(cx.listener(|shell, _: &OpenSettings, window, cx| {
                shell.view = MainView::Settings;
                shell.notice = None;
                window.focus(&shell.focus, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|shell, action: &SetDefaultMode, window, cx| {
                match settings::parse_mode(
                    action.mode.as_deref(),
                    shell.state.settings.default_mode,
                ) {
                    Ok(mode) => shell.send_command(Command::SetDefaultMode { mode }, window, cx),
                    Err(error) => shell.notice(error, cx),
                }
            }))
            .on_action(cx.listener(|shell, _: &RecheckProviders, window, cx| {
                shell.recheck(false, window, cx)
            }))
            .on_action(cx.listener(|shell, _: &Retry, window, cx| shell.recheck(true, window, cx)))
            .on_action(cx.listener(|shell, _: &OpenTerminal, _, cx| {
                if let Some(thread) = shell.selected
                    && let Err(error) =
                        veyra_engine::platform::open_terminal_app(&shell.state.threads[&thread].cwd)
                {
                    shell.notice(error.to_string(), cx);
                }
            }))
            .on_action(cx.listener(|shell, action: &PickPermission, window, cx| {
                shell.pick_permission(action, window, cx)
            }))
            .on_action(cx.listener(|shell, action: &PickProvider, _, cx| {
                if shell.view != MainView::NewThread {
                    shell.notice("Create a new thread to choose a provider", cx);
                    return;
                }
                let next = match shell.provider {
                    ProviderKind::Claude => ProviderKind::Codex,
                    ProviderKind::Codex => ProviderKind::Grok,
                    ProviderKind::Grok => ProviderKind::Claude,
                };
                match action
                    .provider
                    .as_deref()
                    .map(|value| serde_json::from_value::<ProviderKind>(serde_json::json!(value)))
                {
                    Some(Ok(provider)) => shell.provider = provider,
                    Some(Err(_)) => shell.notice("Unknown provider", cx),
                    None => shell.provider = next,
                }
                shell.config.model = None;
                shell.config.effort = None;
                cx.notify();
            }))
            .on_action(cx.listener(|shell, action: &PickModel, window, cx| {
                let models = shell
                    .providers
                    .iter()
                    .find(|probe| probe.provider == shell.chosen_provider())
                    .map(|probe| &probe.models);
                let model = action.model.clone().or_else(|| {
                    models.and_then(|models| {
                        let current = shell.composer_config().model.as_ref().and_then(|value| {
                            models.iter().position(|model| &model.value == value)
                        });
                        models
                            .get(
                                current
                                    .map(|index| (index + 1) % models.len().max(1))
                                    .unwrap_or(0),
                            )
                            .map(|model| model.value.clone())
                    })
                });
                if shell.view == MainView::Chat {
                    let mut config = shell.composer_config().clone();
                    config.model = model;
                    shell.send_command(
                        Command::Configure {
                            thread: shell.selected.unwrap(),
                            config,
                        },
                        window,
                        cx,
                    );
                } else {
                    shell.config.model = model;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|shell, action: &PickEffort, window, cx| {
                let levels = shell
                    .providers
                    .iter()
                    .find(|probe| probe.provider == shell.chosen_provider())
                    .and_then(|probe| {
                        probe
                            .models
                            .iter()
                            .find(|model| {
                                Some(&model.value) == shell.composer_config().model.as_ref()
                            })
                            .or_else(|| probe.models.first())
                    })
                    .map(|model| &model.effort_levels);
                let effort = action.effort.clone().or_else(|| {
                    levels.and_then(|levels| {
                        let current = shell
                            .composer_config()
                            .effort
                            .as_ref()
                            .and_then(|value| levels.iter().position(|level| level == value));
                        levels
                            .get(
                                current
                                    .map(|index| (index + 1) % levels.len().max(1))
                                    .unwrap_or(0),
                            )
                            .cloned()
                    })
                });
                if shell.view == MainView::Chat {
                    let mut config = shell.composer_config().clone();
                    config.effort = effort;
                    shell.send_command(
                        Command::Configure {
                            thread: shell.selected.unwrap(),
                            config,
                        },
                        window,
                        cx,
                    );
                } else {
                    shell.config.effort = effort;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|shell, action: &SelectThread, window, cx| {
                if let Some(id) = action
                    .thread
                    .as_ref()
                    .and_then(|value| {
                        serde_json::from_value::<ThreadId>(serde_json::json!(value)).ok()
                    })
                    .or_else(|| {
                        action
                            .thread
                            .is_none()
                            .then(|| {
                                shell
                                    .state
                                    .threads
                                    .values()
                                    .find(|thread| !thread.archived)
                                    .map(|thread| thread.id)
                            })
                            .flatten()
                    })
                    .filter(|id| {
                        shell
                            .state
                            .threads
                            .get(id)
                            .is_some_and(|thread| !thread.archived)
                    })
                {
                    shell.select_thread(id, window, cx);
                }
            }))
            .on_action(cx.listener(|shell, action: &SelectProject, window, cx| {
                if let Some(id) = action
                    .project
                    .as_ref()
                    .and_then(|value| {
                        serde_json::from_value::<ProjectId>(serde_json::json!(value)).ok()
                    })
                    .or_else(|| {
                        action
                            .project
                            .is_none()
                            .then(|| shell.state.projects.keys().next().copied())
                            .flatten()
                    })
                    .filter(|id| shell.state.projects.contains_key(id))
                {
                    shell.project = Some(id);
                    shell.new_thread(window, cx);
                }
            }))
            .on_action(cx.listener(|shell, action: &ToggleItem, _, cx| {
                if let Some(id) = action
                    .item
                    .as_ref()
                    .and_then(|value| {
                        serde_json::from_value::<ItemId>(serde_json::json!(value)).ok()
                    })
                    .or_else(|| {
                        action
                            .item
                            .is_none()
                            .then(|| {
                                shell.selected.and_then(|thread| {
                                    shell
                                        .state
                                        .transcript(thread)
                                        .iter()
                                        .rev()
                                        .find(|item| {
                                            matches!(
                                                item.body,
                                                ItemBody::Reasoning { .. } | ItemBody::Tool(_)
                                            )
                                        })
                                        .map(|item| item.id)
                                })
                            })
                            .flatten()
                    })
                    && !shell.expanded.remove(&id)
                {
                    shell.expanded.insert(id);
                }
                shell.scroller.update(cx, |state, cx| state.remeasure(cx));
                cx.notify();
            }))
            .on_action(cx.listener(|shell, _: &DismissNotice, window, cx| {
                shell.notice = None;
                if shell.view == MainView::Settings {
                    if let Some(thread) = shell.selected {
                        shell.select_thread(thread, window, cx);
                    } else {
                        shell.new_thread(window, cx);
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|shell, _: &Quit, _, cx| {
                shell.engine.shutdown();
                cx.quit();
            }))
            .on_action(cx.listener(|shell, action: &SetSidebarWidth, _, cx| {
                let layout = &cx.global::<Theme>().layout;
                shell.sidebar_width =
                    px(action.width).clamp(layout.sidebar_min, layout.sidebar_max);
                cx.notify();
            }))
            .on_action(cx.listener(|shell, _: &NarrowSidebar, window, cx| {
                window.dispatch_action(
                    Box::new(SetSidebarWidth {
                        width: f32::from(
                            shell.sidebar_width - cx.global::<Theme>().layout.sidebar_step,
                        ),
                    }),
                    cx,
                );
            }))
            .on_action(cx.listener(|shell, _: &WidenSidebar, window, cx| {
                window.dispatch_action(
                    Box::new(SetSidebarWidth {
                        width: f32::from(
                            shell.sidebar_width + cx.global::<Theme>().layout.sidebar_step,
                        ),
                    }),
                    cx,
                );
            }))
            .on_action(cx.listener(|_, _: &ResetSidebar, window, cx| {
                window.dispatch_action(
                    Box::new(SetSidebarWidth {
                        width: f32::from(cx.global::<Theme>().layout.sidebar_default),
                    }),
                    cx,
                );
            }))
            .on_mouse_move(cx.listener(|shell, event: &MouseMoveEvent, window, cx| {
                if shell.resizing && event.pressed_button == Some(MouseButton::Left) {
                    window.dispatch_action(
                        Box::new(SetSidebarWidth {
                            width: f32::from(event.position.x),
                        }),
                        cx,
                    );
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|shell, _, _, _| shell.resizing = false),
            )
            .child(sidebar::render(self, cx))
            .child(
                div()
                    .id("sidebar-resize")
                    .w(theme.layout.border)
                    .h_full()
                    .flex_none()
                    .bg(theme.colors.border)
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|shell, _, _, cx| {
                            shell.resizing = true;
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(header::render(self, cx))
                    .child(match self.view {
                        MainView::NewThread => new_thread::render(self, cx).into_any_element(),
                        MainView::Settings => {
                            settings::render(self.state.settings, self.notice.clone(), cx)
                                .into_any_element()
                        }
                        MainView::Chat => transcript::render(self, cx).into_any_element(),
                    }),
            )
    }
}
pub fn run(
    engine: Engine,
    runtime: tokio::runtime::Handle,
    data_dir: PathBuf,
    #[cfg(debug_assertions)] requests: tokio::sync::mpsc::Receiver<crate::control::Request>,
) {
    let events = engine.subscribe();
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::register_fonts(cx).expect("bundled Geist fonts");
            actions::init(cx);
            let layout = Theme::new(Appearance::Dark).layout;
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    layout.window_default,
                    cx,
                ))),
                window_min_size: Some(layout.window_min),
                titlebar: Some(TitlebarOptions {
                    title: Some("Veyra".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(layout.traffic_lights),
                }),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, move |window, cx| {
                Theme::apply(Appearance::from_window(window.appearance()), window, cx);
                window
                    .observe_window_appearance(|window, cx| {
                        Theme::apply(Appearance::from_window(window.appearance()), window, cx)
                    })
                    .detach();
                let shell = cx.new(|cx| Shell::new(engine, runtime, events, window, cx));
                #[cfg(debug_assertions)]
                crate::control::attach(&shell, requests, data_dir.clone(), window, cx);
                window.on_next_frame(move |_, _| {
                    let line = format!(
                        "veyra ready pid={} data_dir={}",
                        std::process::id(),
                        data_dir.display()
                    );
                    eprintln!("{line}");
                    tracing::info!("{line}");
                });
                shell
            })
            .expect("open Veyra window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::AppState;
    use std::collections::BTreeMap;
    use veyra_engine::{
        domain::*,
        events::{ChangeEvent, ThreadSnapshot, UiEvent},
        live::LiveState,
        provider::ProviderKind,
    };
    #[test]
    fn snapshot_fence_keeps_metadata_not_carried_by_snapshot() {
        let thread = ThreadId::new();
        let mut state = AppState::default();
        state.apply(UiEvent::Change {
            counter: Some(1),
            event: ChangeEvent::ThreadCreated(Thread {
                id: thread,
                project: ProjectId::new(),
                title: "chat".into(),
                lineage: Lineage::Root,
                provider: ProviderKind::Claude,
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::FullAccess,
                },
                cwd: "/tmp/veyra-snapshot-test".into(),
                native: None,
                native_cursor: None,
                cli_version: None,
                hold: None,
                view: View::Chat,
                import_pending: false,
                archived: false,
            }),
        });
        state.begin_snapshot(thread);
        state.apply(UiEvent::Change {
            counter: Some(3),
            event: ChangeEvent::ThreadLiveChanged {
                thread,
                state: LiveState::Starting,
            },
        });
        assert_eq!(state.live[&thread], LiveState::Starting);
        state.finish_snapshot(
            thread,
            ThreadSnapshot {
                items: vec![],
                buffered_text: BTreeMap::new(),
                counter: 10,
            },
        );
        let native = NativeSessionId("fresh-native".into());
        state.apply(UiEvent::Change {
            counter: Some(4),
            event: ChangeEvent::NativeSessionBound {
                thread,
                native: native.clone(),
                cli_version: "2.1.295".into(),
            },
        });
        state.apply(UiEvent::Change {
            counter: Some(6),
            event: ChangeEvent::TurnEnded(Turn {
                id: TurnId::new(),
                thread,
                ordinal: 1,
                origin: TurnOrigin::Provider,
                state: TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    started: Some(2),
                    at: 7,
                },
            }),
        });
        state.apply(UiEvent::Change {
            counter: Some(7),
            event: ChangeEvent::ThreadLiveChanged {
                thread,
                state: LiveState::Parked,
            },
        });
        assert_eq!(state.threads[&thread].native, Some(native));
        assert_eq!(
            state.threads[&thread].cli_version.as_deref(),
            Some("2.1.295")
        );
        assert_eq!(state.status(thread), Status::Done { took: 5 });
    }
    #[test]
    fn snapshot_fence_applies_only_newer_events() {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let id = ItemId::new();
        let item = Item {
            id,
            thread,
            turn,
            seq: 1,
            body: ItemBody::AssistantText {
                text: "a".into(),
                done: false,
            },
        };
        let mut state = AppState::default();
        state.begin_snapshot(thread);
        state.apply(UiEvent::TextAppended {
            thread,
            item: id,
            counter: 2,
            text: "b".into(),
        });
        state.apply(UiEvent::TextAppended {
            thread,
            item: id,
            counter: 4,
            text: "d".into(),
        });
        state.finish_snapshot(
            thread,
            ThreadSnapshot {
                items: vec![item],
                buffered_text: BTreeMap::from([(id, "bc".into())]),
                counter: 3,
            },
        );
        state.apply(UiEvent::TextAppended {
            thread,
            item: id,
            counter: 3,
            text: "duplicate".into(),
        });
        state.apply(UiEvent::TextAppended {
            thread,
            item: id,
            counter: 5,
            text: "e".into(),
        });
        assert_eq!(
            state.transcript(thread)[0].body,
            ItemBody::AssistantText {
                text: "abcde".into(),
                done: false
            }
        );
    }
}
