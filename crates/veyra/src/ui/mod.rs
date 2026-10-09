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
    menu::PopupMenu,
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
    provider::{Probe, ProviderKind, Readiness},
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct SnapshotRequest(u64);

enum Fence {
    Loading {
        request: SnapshotRequest,
        buffered: Vec<UiEvent>,
    },
    Live(u64),
}
#[derive(Default)]
struct AppState {
    projects: BTreeMap<ProjectId, Project>,
    threads: BTreeMap<ThreadId, Thread>,
    turns: BTreeMap<TurnId, Turn>,
    items: BTreeMap<ThreadId, BTreeMap<ItemId, Item>>,
    #[cfg(debug_assertions)]
    delta_counts: BTreeMap<ItemId, u64>,
    fences: BTreeMap<ThreadId, Fence>,
    settings: Settings,
    statuses: BTreeMap<ThreadId, Status>,
    snapshot_sequence: u64,
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
                    if let Some(items) = self.items.remove(thread) {
                        #[cfg(debug_assertions)]
                        for id in items.keys() {
                            self.delta_counts.remove(id);
                        }
                        #[cfg(not(debug_assertions))]
                        let _ = items;
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
                ChangeEvent::ThreadStatusChanged { thread, status } => {
                    self.statuses.insert(*thread, status.clone());
                }
                ChangeEvent::NativeSessionBound {
                    thread,
                    native,
                    cli_version,
                } => {
                    if let Some(row) = self.threads.get_mut(thread) {
                        row.native = Some(native.clone());
                        if let Some(version) = cli_version {
                            row.cli_version = Some(version.clone());
                        }
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
                Some(Fence::Loading { buffered, .. }) => {
                    buffered.push(event);
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
                thread,
                item,
                text,
                #[cfg(debug_assertions)]
                delta_count,
                ..
            } => {
                #[cfg(debug_assertions)]
                self.delta_counts.insert(item, delta_count);
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
    fn begin_snapshot(&mut self, thread: ThreadId) -> SnapshotRequest {
        self.snapshot_sequence += 1;
        let request = SnapshotRequest(self.snapshot_sequence);
        let buffered = match self.fences.remove(&thread) {
            Some(Fence::Loading { buffered, .. }) => buffered,
            _ => vec![],
        };
        self.fences
            .insert(thread, Fence::Loading { request, buffered });
        request
    }
    fn current_snapshot(&self, thread: ThreadId, request: SnapshotRequest) -> bool {
        matches!(self.fences.get(&thread), Some(Fence::Loading { request: current, .. }) if *current == request)
    }
    fn fail_snapshot(&mut self, thread: ThreadId, request: SnapshotRequest) -> bool {
        if !self.current_snapshot(thread, request) {
            return false;
        }
        if let Some(Fence::Loading { buffered, .. }) = self.fences.remove(&thread) {
            for event in buffered {
                self.apply(event);
            }
        }
        true
    }
    fn finish_snapshot(
        &mut self,
        thread: ThreadId,
        request: SnapshotRequest,
        snapshot: ThreadSnapshot,
    ) -> bool {
        if !self.current_snapshot(thread, request) {
            return false;
        }
        let buffered = match self.fences.remove(&thread) {
            Some(Fence::Loading { buffered, .. }) => buffered,
            _ => return false,
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
        #[cfg(debug_assertions)]
        {
            self.delta_counts
                .retain(|id, _| self.items.values().any(|items| items.contains_key(id)));
            self.delta_counts.extend(snapshot.delta_counts);
        }
        self.fences.insert(thread, Fence::Live(snapshot.counter));
        for event in buffered {
            if let Some((_, counter)) = event_thread(&event)
                && counter > snapshot.counter
            {
                self.apply_transcript(event);
                self.fences.insert(thread, Fence::Live(counter));
            }
        }
        true
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
        self.statuses.get(&id).cloned().unwrap_or(Status::Idle)
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
    Chat(ThreadId),
    Settings { back: Option<ThreadId> },
}
impl MainView {
    fn chat(self) -> Option<ThreadId> {
        match self {
            Self::Chat(thread) => Some(thread),
            _ => None,
        }
    }
    fn selected(self) -> Option<ThreadId> {
        match self {
            Self::Chat(thread) => Some(thread),
            Self::Settings { back } => back,
            Self::NewThread => None,
        }
    }
}
fn cycle<'a, T>(values: &'a [T], current: Option<&T>) -> Option<&'a T>
where
    T: PartialEq,
{
    let index = current.and_then(|current| values.iter().position(|value| value == current));
    values.get(index.map(|index| (index + 1) % values.len()).unwrap_or(0))
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
    probe_cwd: PathBuf,
    probes: veyra_engine::provider::Probes,
    probe_generation: u64,
    project: Option<ProjectId>,
    provider: ProviderKind,
    config: ThreadConfig,
    providers: Vec<Probe>,
    input: Entity<TextareaState>,
    scroller: Entity<MessageScrollerState>,
    expanded: BTreeSet<ItemId>,
    branch: String,
    branch_task: tokio::task::JoinSet<()>,
    provider_picker: Option<Entity<PopupMenu>>,
    #[cfg(debug_assertions)]
    provider_picker_labels: Vec<String>,
    _input_subscription: Subscription,
}
fn provider_rows(probes: &[Probe]) -> [(ProviderKind, String); 3] {
    ProviderKind::ALL.map(|provider| {
        let readiness = match probes
            .iter()
            .find(|probe| probe.provider == provider)
            .map(|probe| &probe.readiness)
        {
            Some(Readiness::Ready) => "Ready",
            Some(Readiness::NotInstalled) => "Not installed",
            Some(Readiness::SignedOut) => "Signed out",
            _ => "Checking…",
        };
        (provider, format!("{} · {readiness}", provider.label()))
    })
}
impl Shell {
    fn new(
        engine: Engine,
        runtime: tokio::runtime::Handle,
        mut events: tokio::sync::mpsc::UnboundedReceiver<UiEvent>,
        probe_cwd: PathBuf,
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
                            && shell.view.chat().is_none()
                        {
                            shell.config.mode = shell.state.settings.default_mode;
                        }
                        let count = shell
                            .view
                            .chat()
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
                        if shell.view.chat().is_some_and(|thread| {
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
            probe_cwd,
            probes: Default::default(),
            probe_generation: 0,
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
            branch_task: Default::default(),
            provider_picker: None,
            #[cfg(debug_assertions)]
            provider_picker_labels: vec![],
            _input_subscription: subscription,
        };
        shell.recheck(false, window, cx);
        shell
    }
    fn set_provider_picker(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.provider_picker = open.then(|| {
            let rows = provider_rows(&self.providers);
            #[cfg(debug_assertions)]
            {
                self.provider_picker_labels = rows.iter().map(|(_, label)| label.clone()).collect();
            }
            let focus = self.focus.clone();
            let menu = PopupMenu::build(window, cx, move |mut menu, _, _| {
                menu = menu.action_context(focus);
                for (provider, label) in rows {
                    menu = menu.menu(
                        label,
                        Box::new(PickProvider {
                            provider: Some(provider),
                        }),
                    );
                }
                menu
            });
            cx.subscribe_in(&menu, window, |shell, _, _: &DismissEvent, window, cx| {
                shell.provider_picker = None;
                shell.input.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
            })
            .detach();
            menu
        });
        cx.notify();
    }
    pub(crate) fn notice(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
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
    fn show(&mut self, view: MainView) {
        self.branch_task = Default::default();
        self.branch.clear();
        self.view = view;
    }
    fn select_thread(&mut self, thread: ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        self.show(MainView::Chat(thread));
        self.notice = None;
        let request = self.state.begin_snapshot(thread);
        let reply = self.engine.snapshot(thread);
        self.scroller.update(cx, |state, cx| state.reset(0, cx));
        self.input.update(cx, |input, cx| input.focus(window, cx));
        let cwd = self.state.threads.get(&thread).map(|row| row.cwd.clone());
        let (sender, branch) = tokio::sync::oneshot::channel();
        self.branch_task.spawn_on(
            async move {
                let branch = if let Some(cwd) = cwd {
                    tokio::process::Command::new("git")
                        .args(["branch", "--show-current"])
                        .current_dir(cwd)
                        .kill_on_drop(true)
                        .output()
                        .await
                        .ok()
                        .filter(|output| output.status.success())
                        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let _ = sender.send(branch);
            },
            &self.runtime,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(branch) = branch.await {
                let _ = this.update(cx, |shell, cx| {
                    if shell.view.chat() == Some(thread)
                        && shell.state.snapshot_sequence == request.0
                    {
                        shell.branch = branch;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let snapshot = reply.await;
            let _ = this.update(cx, |shell, cx| {
                let applied = match snapshot {
                    Ok(snapshot) => shell.state.finish_snapshot(thread, request, snapshot),
                    Err(_) => {
                        let current = shell.state.fail_snapshot(thread, request);
                        if current {
                            shell.notice = Some("The transcript could not be loaded".into());
                        }
                        current
                    }
                };
                if applied && shell.view.chat() == Some(thread) {
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
        if self.provider_picker.is_some() {
            self.set_provider_picker(true, window, cx);
        }
        self.probe_generation += 1;
        let generation = self.probe_generation;
        let retry_thread = retry.then(|| self.view.chat()).flatten();
        let env = veyra_engine::provider::env::ChildEnv::load();
        for provider in ProviderKind::ALL {
            let reply =
                self.probes
                    .check(&self.runtime, provider, env.clone(), self.probe_cwd.clone());
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(probe) = reply.await {
                    let _ = this.update_in(cx, |shell, window, cx| {
                        if shell.probe_generation != generation {
                            return;
                        }
                        shell.providers.push(probe);
                        if shell.provider_picker.is_some() {
                            shell.set_provider_picker(true, window, cx);
                        }
                        if let Some(thread) = retry_thread
                            && shell.state.threads[&thread].provider == provider
                            && shell.ready(provider)
                        {
                            shell.retry_turn(thread, window, cx);
                        }
                        cx.notify();
                    });
                }
            })
            .detach();
        }
        cx.notify();
    }
    fn retry_turn(&mut self, thread: ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .state
            .transcript(thread)
            .iter()
            .rev()
            .find_map(|item| match &item.body {
                ItemBody::UserText(text) => Some(text.clone()),
                _ => None,
            });
        if let Some(text) = text {
            if self.state.threads[&thread].hold.is_some() {
                self.send_command(
                    Command::Continue {
                        thread,
                        drop_queued: false,
                    },
                    window,
                    cx,
                );
            }
            self.send_command(Command::Send { thread, text }, window, cx);
        }
    }
    fn probe(&self, provider: ProviderKind) -> Option<&Probe> {
        self.providers
            .iter()
            .find(|probe| probe.provider == provider)
    }
    fn ready(&self, provider: ProviderKind) -> bool {
        self.probe(provider)
            .is_some_and(|probe| probe.readiness == Readiness::Ready)
    }
    fn composer_config(&self) -> &ThreadConfig {
        self.view
            .chat()
            .and_then(|id| self.state.threads.get(&id))
            .map(|row| &row.config)
            .unwrap_or(&self.config)
    }
    fn chosen_provider(&self) -> ProviderKind {
        self.view
            .chat()
            .and_then(|id| self.state.threads.get(&id))
            .map(|row| row.provider)
            .unwrap_or(self.provider)
    }
    fn disabled_reason(&self) -> Option<String> {
        if matches!(self.view, MainView::Settings { .. }) {
            return Some("Close Settings to send a message".into());
        }
        if self.project.is_none() && self.view == MainView::NewThread {
            return Some("Add a project to start a thread".into());
        }
        if !self.ready(self.chosen_provider()) {
            let provider = self.chosen_provider();
            return Some(match self.probe(provider).map(|probe| &probe.readiness) {
                Some(Readiness::NotInstalled) => format!(
                    "{} is not installed. {}",
                    provider.label(),
                    provider.install_hint()
                ),
                Some(Readiness::SignedOut) => format!(
                    "Sign in with {} and check again.",
                    provider.sign_in_command()
                ),
                _ => format!("Checking {}…", provider.label()),
            });
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
        let command = if let MainView::Chat(thread) = self.view {
            Command::Send { thread, text }
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
        self.show(MainView::NewThread);
        self.provider_picker = None;
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
        let mode = action
            .mode
            .unwrap_or_else(|| settings::next_mode(self.composer_config().mode));
        if matches!(self.view, MainView::Settings { .. }) {
            self.send_command(Command::SetDefaultMode { mode }, window, cx);
        } else {
            self.update_config(window, cx, |config| config.mode = mode);
        }
    }
    fn update_config(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut ThreadConfig),
    ) {
        let mut config = self.composer_config().clone();
        update(&mut config);
        if let MainView::Chat(thread) = self.view {
            self.send_command(Command::Configure { thread, config }, window, cx);
        } else {
            self.config = config;
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
        if matches!(self.view, MainView::Settings { .. })
            || !self.input.focus_handle(cx).is_focused(window)
        {
            anyhow::bail!("the focused element has no text input handler");
        }
        self.input.update(cx, |input, cx| {
            input.replace_text_in_range(None, text, window, cx)
        });
        Ok(())
    }
    #[cfg(debug_assertions)]
    fn dump_rows(&self) -> Vec<serde_json::Value> {
        let mut rows = Vec::new();
        for project in self.state.projects.values() {
            rows.push(serde_json::json!({
                "kind":"project", "id":project.id, "name":project.name, "path":project.path
            }));
            for thread in self
                .state
                .threads
                .values()
                .filter(|thread| thread.project == project.id && !thread.archived)
            {
                rows.push(serde_json::json!({
                    "kind":"thread", "id":thread.id, "title":thread.title,
                    "status":status_name(&self.state.status(thread.id)),
                    "archived":thread.archived, "mode":thread.config.mode
                }));
            }
        }
        rows
    }
    #[cfg(debug_assertions)]
    fn dump_selected(&self) -> Option<serde_json::Value> {
        self.view
            .selected()
            .and_then(|id| self.state.threads.get(&id))
            .map(|thread| {
                let status = self.state.status(thread.id);
                serde_json::json!({
                    "id":thread.id, "title":thread.title,
                    "native_id":thread.native.as_ref().map(|native| &native.0),
                    "cli_version":thread.cli_version, "config":thread.config,
                    "status":status_name(&status), "header_status":header::status_text(&status),
                    "failure":self.state.failure(thread.id)
                })
            })
    }
    #[cfg(debug_assertions)]
    fn dump_providers(&self) -> Vec<serde_json::Value> {
        ProviderKind::ALL
            .into_iter()
            .map(|provider| {
                let probe = self.probe(provider);
                serde_json::json!({
                    "provider":provider,
                    "readiness":probe.map(|probe| &probe.readiness).unwrap_or(&Readiness::Checking),
                    "path":probe.and_then(|probe| probe.path.as_ref()),
                    "version":probe.and_then(|probe| probe.version.as_ref()),
                    "models":probe.map(|probe| &probe.models),
                    "account":probe.map(Probe::account_presence).unwrap_or("unknown")
                })
            })
            .collect()
    }
    #[cfg(debug_assertions)]
    fn dump_transcript(&self) -> Vec<serde_json::Value> {
        self.view
            .selected()
            .map(|thread| self.state.transcript(thread))
            .unwrap_or_default()
            .iter()
            .map(|item| {
                let mut row = transcript::dump_item(item);
                row["expanded"] = serde_json::json!(self.expanded.contains(&item.id));
                row["delta_count"] = self
                    .state
                    .delta_counts
                    .get(&item.id)
                    .copied()
                    .unwrap_or_default()
                    .into();
                row
            })
            .collect()
    }
    #[cfg(debug_assertions)]
    fn dump_composer(&self) -> serde_json::Value {
        let reason = self.disabled_reason();
        serde_json::json!({
            "provider":self.chosen_provider(), "mode":self.composer_config().mode,
            "model":self.composer_config().model, "effort":self.composer_config().effort,
            "send_enabled":reason.is_none(), "disabled_reason":reason,
            "provider_picker_open":self.provider_picker.is_some(),
            "provider_picker_labels":self.provider_picker.as_ref().map(|_| &self.provider_picker_labels)
        })
    }
    #[cfg(debug_assertions)]
    pub fn dump(&self, data_dir: &std::path::Path, cx: &App) -> serde_json::Value {
        serde_json::json!({
            "pid":std::process::id(), "build_rev":env!("VEYRA_BUILD_REV"), "data_dir":data_dir,
            "appearance":cx.global::<Theme>().appearance.name(), "view":self.view_name(),
            "sidebar":{"width":f32::from(self.sidebar_width), "rows":self.dump_rows()},
            "selected_thread":self.dump_selected(), "transcript":self.dump_transcript(),
            "composer_text":self.input.read(cx).value().to_string(), "composer":self.dump_composer(),
            "providers":self.dump_providers(), "settings":{"default_mode":self.state.settings.default_mode},
            "perf":{"applied_batches":self.state.applied_batches, "max_batch_gap_ms":self.state.max_batch_gap_ms},
            "notice":self.notice.as_ref().map(|notice| notice.as_ref())
        })
    }
    #[cfg(debug_assertions)]
    fn view_name(&self) -> &'static str {
        match self.view {
            MainView::Settings { .. } => "settings",
            MainView::NewThread => "new_thread",
            MainView::Chat(thread) if self.state.failure(thread).is_some() => "failed_thread",
            MainView::Chat(_) => "chat",
        }
    }
}
impl Focusable for Shell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>().clone();
        let root = div()
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
                shell.show(MainView::Settings {
                    back: shell.view.selected(),
                });
                shell.notice = None;
                window.focus(&shell.focus, cx);
                cx.notify();
            }))
            .on_action(cx.listener(|shell, action: &SetDefaultMode, window, cx| {
                let mode = action
                    .mode
                    .unwrap_or_else(|| settings::next_mode(shell.state.settings.default_mode));
                shell.send_command(Command::SetDefaultMode { mode }, window, cx);
            }))
            .on_action(cx.listener(|shell, _: &OpenProviderPicker, window, cx| {
                if shell.view == MainView::NewThread {
                    shell.set_provider_picker(shell.provider_picker.is_none(), window, cx);
                }
            }))
            .on_action(cx.listener(|shell, _: &RecheckProviders, window, cx| {
                shell.recheck(false, window, cx)
            }))
            .on_action(cx.listener(|shell, _: &Retry, window, cx| shell.recheck(true, window, cx)))
            .on_action(cx.listener(|shell, _: &OpenTerminal, _, cx| {
                if let Some(thread) = shell.view.selected()
                    && let Err(error) =
                        veyra_engine::platform::open_terminal_app(&shell.state.threads[&thread].cwd)
                {
                    shell.notice(error.to_string(), cx);
                }
            }))
            .on_action(cx.listener(|shell, action: &PickPermission, window, cx| {
                shell.pick_permission(action, window, cx)
            }))
            .on_action(cx.listener(|shell, action: &PickProvider, window, cx| {
                if shell.view != MainView::NewThread {
                    shell.notice("Create a new thread to choose a provider", cx);
                    return;
                }
                shell.provider = action.provider.unwrap_or_else(|| {
                    *cycle(&ProviderKind::ALL, Some(&shell.provider)).expect("providers")
                });
                shell.provider_picker = None;
                shell.input.update(cx, |input, cx| input.focus(window, cx));
                shell.config.model = None;
                shell.config.effort = None;
                cx.notify();
            }))
            .on_action(cx.listener(|shell, action: &PickModel, window, cx| {
                let models: Vec<_> = shell
                    .probe(shell.chosen_provider())
                    .into_iter()
                    .flat_map(|probe| probe.models.iter().map(|model| model.value.clone()))
                    .collect();
                let model = action
                    .model
                    .clone()
                    .or_else(|| cycle(&models, shell.composer_config().model.as_ref()).cloned());
                shell.update_config(window, cx, |config| config.model = model);
            }))
            .on_action(cx.listener(|shell, action: &PickEffort, window, cx| {
                let levels = shell
                    .probe(shell.chosen_provider())
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
                        cycle(levels, shell.composer_config().effort.as_ref()).cloned()
                    })
                });
                shell.update_config(window, cx, |config| config.effort = effort);
            }))
            .on_action(cx.listener(|shell, action: &SelectThread, window, cx| {
                if let Some(id) = action
                    .thread
                    .or_else(|| {
                        shell
                            .state
                            .threads
                            .values()
                            .find(|thread| !thread.archived)
                            .map(|thread| thread.id)
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
                    .or_else(|| shell.state.projects.keys().next().copied())
                    .filter(|id| shell.state.projects.contains_key(id))
                {
                    shell.project = Some(id);
                    shell.new_thread(window, cx);
                }
            }))
            .on_action(cx.listener(|shell, action: &ToggleItem, _, cx| {
                if let Some(id) = action.item.or_else(|| {
                    shell.view.selected().and_then(|thread| {
                        shell
                            .state
                            .transcript(thread)
                            .iter()
                            .rev()
                            .find(|item| {
                                matches!(item.body, ItemBody::Reasoning { .. } | ItemBody::Tool(_))
                            })
                            .map(|item| item.id)
                    })
                }) && !shell.expanded.remove(&id)
                {
                    shell.expanded.insert(id);
                }
                shell.scroller.update(cx, |state, cx| state.remeasure(cx));
                cx.notify();
            }))
            .on_action(cx.listener(|shell, _: &DismissNotice, window, cx| {
                shell.notice = None;
                shell.provider_picker = None;
                if matches!(shell.view, MainView::Settings { .. }) {
                    if let Some(thread) = shell.view.selected() {
                        shell.select_thread(thread, window, cx);
                    } else {
                        shell.new_thread(window, cx);
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|shell, _: &Quit, _, cx| {
                shell.show(MainView::NewThread);
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
                        MainView::Settings { .. } => {
                            settings::render(self.state.settings, self.notice.clone(), cx)
                                .into_any_element()
                        }
                        MainView::Chat(_) => transcript::render(self, cx).into_any_element(),
                    }),
            );
        #[cfg(debug_assertions)]
        let root = root.on_action(cx.listener(|_, _: &ActivateWindow, window, cx| {
            cx.activate(true);
            window.activate_window();
        }));
        root
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
                let shell =
                    cx.new(|cx| Shell::new(engine, runtime, events, data_dir.clone(), window, cx));
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
    use super::{AppState, provider_rows};
    use std::collections::BTreeMap;
    use veyra_engine::{
        domain::*,
        events::{ChangeEvent, ThreadSnapshot, UiEvent},
        provider::{Probe, ProviderKind, Readiness},
    };

    #[test]
    fn window_projection_shows_pending_background_work() {
        use veyra_engine::{
            events::ChangeSet,
            live::{Live, LiveThread},
            provider::SessionHandle,
        };
        let thread = ThreadId::new();
        let turn = Turn {
            id: TurnId::new(),
            thread,
            ordinal: 1,
            origin: TurnOrigin::Provider,
            state: TurnState::Ended {
                outcome: TurnOutcome::Completed,
                started: Some(1),
                at: 2,
            },
        };
        let row = Thread {
            id: thread,
            project: ProjectId::new(),
            title: "background".into(),
            lineage: Lineage::Root,
            provider: ProviderKind::Claude,
            config: ThreadConfig {
                model: None,
                effort: None,
                mode: PermissionMode::FullAccess,
            },
            cwd: "/tmp".into(),
            native: None,
            native_cursor: None,
            cli_version: None,
            hold: None,
            view: View::Chat,
            import_pending: false,
            archived: false,
        };
        let mut state = AppState::default();
        state.apply(UiEvent::Change {
            counter: Some(1),
            event: ChangeEvent::ThreadCreated(row.clone()),
        });
        state.apply(UiEvent::Change {
            counter: Some(2),
            event: ChangeEvent::TurnEnded(turn.clone()),
        });
        let mut projection = Projection::default();
        projection.threads.insert(thread, row);
        projection.insert_turn(turn);
        let (control, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut changes = ChangeSet::default();
        changes.live.insert(
            thread,
            LiveThread {
                live: Live::Structured {
                    session: SessionHandle { control },
                    pid: 1,
                    pgid: 1,
                },
                background_work: true,
                ..Default::default()
            },
        );
        let (events, _) = changes.install(&mut projection, &mut BTreeMap::new());
        for event in events {
            state.apply(UiEvent::Change {
                counter: Some(3),
                event,
            });
        }
        assert_eq!(state.status(thread), Status::Working { since: 0 });
    }
    #[test]
    fn snapshot_failure_releases_only_its_own_fence() {
        let thread = ThreadId::new();
        let item = Item {
            id: ItemId::new(),
            thread,
            turn: TurnId::new(),
            seq: 1,
            body: ItemBody::AssistantText {
                text: "retained".into(),
                done: true,
            },
        };
        let mut state = AppState::default();
        let old = state.begin_snapshot(thread);
        state.apply(UiEvent::Change {
            counter: Some(2),
            event: ChangeEvent::ItemCompleted(item.clone()),
        });
        let current = state.begin_snapshot(thread);
        assert!(!state.fail_snapshot(thread, old));
        assert!(state.fail_snapshot(thread, current));
        assert_eq!(state.transcript(thread), vec![item]);
        assert!(matches!(state.fences[&thread], super::Fence::Live(2)));
    }
    #[test]
    fn snapshot_reselection_keeps_the_current_reply() {
        let thread = ThreadId::new();
        let item = Item {
            id: ItemId::new(),
            thread,
            turn: TurnId::new(),
            seq: 1,
            body: ItemBody::AssistantText {
                text: "complete".into(),
                done: true,
            },
        };
        let mut state = AppState::default();
        let first = state.begin_snapshot(thread);
        state.apply(UiEvent::Change {
            counter: Some(2),
            event: ChangeEvent::ItemCompleted(item.clone()),
        });
        let second = state.begin_snapshot(thread);
        state.finish_snapshot(
            thread,
            first,
            ThreadSnapshot {
                items: vec![],
                buffered_text: BTreeMap::new(),
                #[cfg(debug_assertions)]
                delta_counts: BTreeMap::new(),
                counter: 1,
            },
        );
        state.finish_snapshot(
            thread,
            second,
            ThreadSnapshot {
                items: vec![item.clone()],
                buffered_text: BTreeMap::new(),
                #[cfg(debug_assertions)]
                delta_counts: BTreeMap::new(),
                counter: 2,
            },
        );
        assert_eq!(state.transcript(thread), vec![item]);
    }
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
        let request = state.begin_snapshot(thread);
        state.apply(UiEvent::Change {
            counter: Some(3),
            event: ChangeEvent::ThreadStatusChanged {
                thread,
                status: Status::Working { since: 0 },
            },
        });
        assert_eq!(state.status(thread), Status::Working { since: 0 });
        state.finish_snapshot(
            thread,
            request,
            ThreadSnapshot {
                items: vec![],
                buffered_text: BTreeMap::new(),
                #[cfg(debug_assertions)]
                delta_counts: BTreeMap::new(),
                counter: 10,
            },
        );
        let native = NativeSessionId("fresh-native".into());
        state.apply(UiEvent::Change {
            counter: Some(4),
            event: ChangeEvent::NativeSessionBound {
                thread,
                native: native.clone(),
                cli_version: Some("2.1.295".into()),
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
            event: ChangeEvent::ThreadStatusChanged {
                thread,
                status: Status::Done { took: 5 },
            },
        });
        assert_eq!(state.threads[&thread].native, Some(native));
        assert_eq!(
            state.threads[&thread].cli_version.as_deref(),
            Some("2.1.295")
        );
        state.apply(UiEvent::Change {
            counter: Some(8),
            event: ChangeEvent::ThreadStatusChanged {
                thread,
                status: Status::Done { took: 5 },
            },
        });
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
        let request = state.begin_snapshot(thread);
        state.apply(UiEvent::TextAppended {
            #[cfg(debug_assertions)]
            delta_count: 1,
            thread,
            item: id,
            counter: 2,
            text: "b".into(),
        });
        state.apply(UiEvent::TextAppended {
            #[cfg(debug_assertions)]
            delta_count: 1,
            thread,
            item: id,
            counter: 4,
            text: "d".into(),
        });
        state.finish_snapshot(
            thread,
            request,
            ThreadSnapshot {
                items: vec![item],
                buffered_text: BTreeMap::from([(id, "bc".into())]),
                #[cfg(debug_assertions)]
                delta_counts: BTreeMap::new(),
                counter: 3,
            },
        );
        state.apply(UiEvent::TextAppended {
            #[cfg(debug_assertions)]
            delta_count: 1,
            thread,
            item: id,
            counter: 3,
            text: "duplicate".into(),
        });
        state.apply(UiEvent::TextAppended {
            #[cfg(debug_assertions)]
            delta_count: 1,
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
    #[test]
    fn provider_rows_label_each_readiness() {
        assert_eq!(provider_rows(&[])[0].1, "Claude Code · Checking…");
        let mut probe = Probe {
            provider: ProviderKind::Claude,
            path: None,
            version: None,
            readiness: Readiness::NotInstalled,
            models: vec![],
            account: None,
        };
        assert_eq!(
            provider_rows(&[probe.clone()])[0].1,
            "Claude Code · Not installed"
        );
        probe.readiness = Readiness::Ready;
        assert_eq!(provider_rows(&[probe])[0].1, "Claude Code · Ready");
    }
}
