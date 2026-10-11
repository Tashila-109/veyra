pub mod actions;
mod composer;
mod header;
mod new_thread;
mod notify;
mod settings;
mod sidebar;
pub mod theme;
mod transcript;

use actions::*;
use gpui_kit::component::{
    StyledExt,
    input::{InputEvent, TextareaState},
    menu::PopupMenu,
};
use gpui_kit::{prelude::*, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::Instant,
};
use theme::{Appearance, Theme};
use transcript::MessageScrollerState;
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
    requests: Vec<Request>,
    statuses: BTreeMap<ThreadId, Status>,
    snapshot_sequence: u64,
    applied_batches: u64,
    #[cfg(debug_assertions)]
    max_batch_events: usize,
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
        #[cfg(debug_assertions)]
        {
            self.max_batch_events = self.max_batch_events.max(events.len());
        }
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
                ChangeEvent::RequestOpened(request) | ChangeEvent::RequestClosed(request) => {
                    if let Some(row) = self.requests.iter_mut().find(|row| row.id == request.id) {
                        *row = request.clone();
                    } else {
                        self.requests.push(request.clone());
                    }
                }
                ChangeEvent::ItemAdded(_)
                | ChangeEvent::ItemCompleted(_)
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
    fn pending_requests(&self, thread: ThreadId) -> Vec<&Request> {
        self.requests
            .iter()
            .filter(|request| request.thread == thread && request.state == RequestState::Pending)
            .collect()
    }
    fn family_status(&self, root: ThreadId) -> Status {
        self.threads
            .values()
            .filter(|thread| !thread.archived && thread.root() == root)
            .map(|thread| self.status(thread.id))
            .min_by_key(notify::priority)
            .unwrap_or(Status::Idle)
    }
    fn status(&self, id: ThreadId) -> Status {
        self.statuses.get(&id).cloned().unwrap_or(Status::Idle)
    }
    fn retry_prompt(&self, thread: ThreadId) -> Option<String> {
        let turn = self
            .turns
            .values()
            .filter(|turn| turn.thread == thread && turn.state != TurnState::Queued)
            .max_by_key(|turn| turn.ordinal)?;
        if !matches!(
            turn.state,
            TurnState::Ended {
                outcome: TurnOutcome::Failed { .. },
                ..
            }
        ) {
            return None;
        }
        let TurnOrigin::User { item } = turn.origin else {
            return None;
        };
        match &self.items.get(&thread)?.get(&item)?.body {
            ItemBody::UserText(text) => Some(text.clone()),
            _ => None,
        }
    }
    fn failure(&self, thread: ThreadId) -> Option<&str> {
        self.turns
            .values()
            .filter(|turn| turn.thread == thread && turn.state != TurnState::Queued)
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
    #[cfg(debug_assertions)]
    repaints: u64,
    focus: FocusHandle,
    dock_focus: FocusHandle,
    dock_focused: bool,
    quit_focus: FocusHandle,
    quit_sheet: bool,
    quitting: bool,
    window_active: bool,
    notifications_posted: u64,
    question_draft: Option<(RequestId, Vec<Answer>)>,
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
        cx.observe_window_activation(window, |shell, window, cx| {
            shell.window_active = window.is_window_active();
            cx.notify();
        })
        .detach();
        cx.on_app_quit(|shell, _| {
            shell.engine.shutdown();
            shell.quitting = true;
            shell.probes = Default::default();
            shell.forget_branch();
            let engine = shell.engine.clone();
            async move { engine.on_app_quit().await }
        })
        .detach();
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
                        let families: BTreeMap<_, _> = shell
                            .state
                            .threads
                            .values()
                            .map(|thread| (thread.root(), shell.state.family_status(thread.root())))
                            .collect();
                        shell.state.apply_all(batch);
                        shell.post_banners(&families, cx);
                        shell.start_branch_lookup(cx);
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
                        if shell
                            .state
                            .statuses
                            .values()
                            .any(|status| matches!(status, Status::Working { .. }))
                        {
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
            #[cfg(debug_assertions)]
            repaints: 0,
            focus,
            dock_focus: cx.focus_handle(),
            dock_focused: false,
            quit_focus: cx.focus_handle(),
            quit_sheet: false,
            quitting: false,
            window_active: window.is_window_active(),
            notifications_posted: 0,
            question_draft: None,
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
        cx.on_focus(&shell.dock_focus, window, |shell, _, cx| {
            shell.dock_focused = true;
            cx.notify();
        })
        .detach();
        cx.on_focus_out(&shell.dock_focus, window, |shell, _, _, cx| {
            shell.dock_focused = false;
            cx.notify();
        })
        .detach();
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
    fn forget_branch(&mut self) {
        self.branch_task = Default::default();
        self.branch.clear();
    }
    fn show(&mut self, view: MainView) {
        if matches!(view, MainView::Settings { .. }) {
            self.branch.clear();
        } else {
            self.forget_branch();
        }
        self.view = view;
    }
    fn start_branch_lookup(&mut self, cx: &mut Context<Self>) {
        if !self.branch_task.is_empty() {
            return;
        }
        let Some(thread) = self.view.chat() else {
            return;
        };
        let Some(cwd) = self.state.threads.get(&thread).map(|row| row.cwd.clone()) else {
            return;
        };
        let selection = self.state.snapshot_sequence;
        let (sender, branch) = tokio::sync::oneshot::channel();
        self.branch_task.spawn_on(
            async move {
                let branch = tokio::process::Command::new("git")
                    .args(["branch", "--show-current"])
                    .current_dir(cwd)
                    .kill_on_drop(true)
                    .output()
                    .await
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                    .unwrap_or_default();
                let _ = sender.send(branch);
            },
            &self.runtime,
        );
        cx.spawn(async move |this, cx| {
            if let Ok(branch) = branch.await {
                let _ = this.update(cx, |shell, cx| {
                    if shell.view.chat() == Some(thread)
                        && shell.state.snapshot_sequence == selection
                    {
                        shell.branch = branch;
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
    fn select_thread(&mut self, thread: ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        self.show(MainView::Chat(thread));
        self.question_draft = None;
        self.notice = None;
        let request = self.state.begin_snapshot(thread);
        let reply = self.engine.snapshot(thread);
        self.scroller.update(cx, |state, cx| state.reset(0, cx));
        self.input.update(cx, |input, cx| input.focus(window, cx));
        self.start_branch_lookup(cx);
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
        if self.quitting {
            return;
        }
        self.providers.clear();
        if self.provider_picker.is_some() {
            self.set_provider_picker(true, window, cx);
        }
        self.probe_generation += 1;
        let generation = self.probe_generation;
        let retry_thread = retry.then(|| self.view.chat()).flatten();
        let env = self.engine.child_env();
        for provider in ProviderKind::ALL {
            let reply =
                self.probes
                    .check(&self.runtime, provider, env.clone(), self.probe_cwd.clone());
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(probe) = reply.await {
                    let _ = this.update_in(cx, |shell, window, cx| {
                        if shell.quitting || shell.probe_generation != generation {
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
        let text = self.state.retry_prompt(thread);
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
        if text.trim().is_empty() && self.view.chat().is_some_and(|thread| self.running(thread)) {
            window.dispatch_action(Box::new(Stop), cx);
            return;
        }
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
    fn running(&self, thread: ThreadId) -> bool {
        self.state
            .turns
            .values()
            .any(|turn| turn.thread == thread && matches!(turn.state, TurnState::Running { .. }))
    }
    fn busy_threads(&self) -> Vec<String> {
        self.engine
            .window_threads()
            .iter()
            .filter(|(_, state)| state.busy)
            .filter_map(|(id, _)| {
                self.state
                    .threads
                    .get(id)
                    .map(|thread| thread.title.clone())
            })
            .collect()
    }
    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(thread) = self.view.chat() {
            self.send_command(Command::Interrupt { thread }, window, cx);
        }
    }
    fn continue_queue(&mut self, drop_queued: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(thread) = self.view.chat() {
            self.send_command(
                Command::Continue {
                    thread,
                    drop_queued,
                },
                window,
                cx,
            );
        }
    }
    fn respond(&mut self, decision: Decision, window: &mut Window, cx: &mut Context<Self>) {
        let request = self.view.chat().and_then(|thread| {
            self.state
                .pending_requests(thread)
                .first()
                .map(|row| row.id)
        });
        if let Some(request) = request {
            self.send_command(Command::Respond { request, decision }, window, cx);
            if self.view.chat().is_some_and(|thread| {
                self.state
                    .pending_requests(thread)
                    .iter()
                    .all(|row| row.id == request)
            }) {
                self.input.update(cx, |input, cx| input.focus(window, cx));
            }
        }
    }
    fn answer_question(&mut self, option: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.view.chat() else {
            return;
        };
        let request = self
            .state
            .pending_requests(thread)
            .first()
            .copied()
            .cloned();
        let Some(Request {
            id,
            kind: RequestKind::Question { questions },
            ..
        }) = request
        else {
            return;
        };
        let index = self
            .question_draft
            .as_ref()
            .filter(|(request, _)| *request == id)
            .map_or(0, |(_, answers)| answers.len());
        let Some(text) = questions
            .get(index)
            .and_then(|question| question.options.get(option))
            .cloned()
        else {
            return;
        };
        self.question_draft.get_or_insert_with(|| (id, vec![]));
        if self
            .question_draft
            .as_ref()
            .is_some_and(|(request, _)| *request != id)
        {
            self.question_draft = Some((id, vec![]));
        }
        let (_, answers) = self.question_draft.as_mut().unwrap();
        answers.push(Answer { text });
        if answers.len() == questions.len() {
            let (_, answers) = self.question_draft.take().unwrap();
            self.respond(Decision::Answers(answers), window, cx);
        }
        cx.notify();
    }
    fn render_quit_sheet(&self, cx: &App) -> impl IntoElement {
        use gpui_kit::component::button::ButtonVariants;
        let t = cx.global::<Theme>();
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.quit_focus)
            .key_context("VeyraQuit")
            .child(
                div()
                    .v_flex()
                    .w(t.layout.transcript_min)
                    .p(t.space.0[7])
                    .gap(t.space.0[5])
                    .rounded(t.radius.large)
                    .bg(t.colors.card)
                    .border(t.layout.border)
                    .border_color(t.colors.border_strong)
                    .child(div().text_size(t.text.h2.size).child("Quit Veyra?"))
                    .child("Quitting stops these threads:")
                    .children(
                        self.busy_threads()
                            .into_iter()
                            .map(|title| div().child(title)),
                    )
                    .child(
                        div()
                            .h_flex()
                            .justify_end()
                            .gap(t.space.0[3])
                            .child(
                                composer::labelled_button("cancel-quit", "Cancel", cx).on_click(
                                    |_, window, cx| {
                                        window.dispatch_action(Box::new(CancelQuit), cx)
                                    },
                                ),
                            )
                            .child(
                                composer::labelled_button("confirm-quit", "Quit", cx)
                                    .primary()
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(Box::new(ConfirmQuit), cx)
                                    }),
                            ),
                    ),
            )
    }
    fn discard_queued_visible(&self) -> bool {
        self.view.chat().is_some_and(|thread| {
            self.state
                .threads
                .get(&thread)
                .is_some_and(|row| row.hold.is_some())
                && self
                    .state
                    .turns
                    .values()
                    .any(|turn| turn.thread == thread && turn.state == TurnState::Queued)
        })
    }
    fn confirm_quit(&mut self, cx: &mut Context<Self>) {
        if self.quitting {
            return;
        }
        self.engine.shutdown();
        self.quitting = true;
        self.probes = Default::default();
        self.forget_branch();
        let engine = self.engine.clone();
        cx.spawn(async move |_, cx| {
            engine.shutdown_wait().await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
    fn post_banners(&mut self, previous: &BTreeMap<ThreadId, Status>, cx: &mut Context<Self>) {
        let roots: BTreeSet<_> = self
            .state
            .threads
            .values()
            .filter(|thread| !thread.archived)
            .map(Thread::root)
            .collect();
        for root in roots {
            let status = self.state.family_status(root);
            let old = previous.get(&root).unwrap_or(&Status::Idle);
            if let Some(banner) = self.state.threads.get(&root).and_then(|thread| {
                notify::banner_for(self.window_active, old, &status, root, &thread.title)
            }) {
                self.notifications_posted += 1;
                cx.show_system_notification(banner);
            }
        }
    }
    fn notification_response(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(root) = serde_json::from_value::<ThreadId>(serde_json::Value::String(tag.into()))
            && let Some(thread) = self.banner_target(root)
        {
            cx.activate(true);
            window.activate_window();
            self.select_thread(thread, window, cx);
        }
    }
    fn banner_target(&self, root: ThreadId) -> Option<ThreadId> {
        sidebar::ordered_threads(&self.state)
            .into_iter()
            .find(|id| {
                self.state.threads[id].root() == root && self.state.status(*id) == Status::NeedsYou
            })
            .or_else(|| {
                self.state
                    .threads
                    .get(&root)
                    .filter(|thread| !thread.archived)
                    .map(|thread| thread.id)
            })
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
                    "failure":self.state.failure(thread.id), "hold":thread.hold,
                    "wait_reason":self.engine.window_threads().get(&thread.id)
                        .and_then(|state| state.wait_reason).map(|reason| reason.to_string())
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
        let items = self
            .view
            .selected()
            .map(|thread| self.state.transcript(thread))
            .unwrap_or_default();
        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut row = transcript::dump_item(item);
                row["expanded"] = serde_json::json!(self.expanded.contains(&item.id));
                row["outcome"] = transcript::row_outcome(&items, index, &self.state.turns).into();
                row["wait_reason"] = self
                    .state
                    .turns
                    .get(&item.turn)
                    .filter(|turn| turn.state == TurnState::Queued)
                    .and_then(|_| {
                        self.engine
                            .window_threads()
                            .get(&item.thread)
                            .and_then(|state| state.wait_reason)
                    })
                    .map(|reason| reason.to_string())
                    .into();
                if let ItemBody::Tool(tool) = &item.body {
                    row["failed_last_line"] = transcript::failed_tool_last_line(tool).into();
                }
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
    fn dump_composer(&self, cx: &App) -> serde_json::Value {
        let reason = self.disabled_reason();
        serde_json::json!({
            "provider":self.chosen_provider(), "mode":self.composer_config().mode,
            "model":self.composer_config().model, "effort":self.composer_config().effort,
            "send_enabled":reason.is_none(), "disabled_reason":reason,
            "provider_picker_open":self.provider_picker.is_some(),
            "provider_picker_labels":self.provider_picker.as_ref().map(|_| &self.provider_picker_labels),
            "stop_visible":self.view.chat().is_some_and(|thread| self.running(thread))
                && self.input.read(cx).value().trim().is_empty(),
            "discard_queued_visible":self.discard_queued_visible(),
            "dock":self.view.chat().and_then(|thread| self.state.pending_requests(thread).first().map(|request| {
                let text = match &request.kind {
                    RequestKind::Approval { action, .. } => Some(composer::approval_text(
                        action, self.chosen_provider().label())),
                    _ => None,
                };
                serde_json::json!({"request":request, "position":1,
                    "action_text":text.as_ref().map(|(label, _, _)| label),
                    "consequence":text.as_ref().map(|(_, consequence, _)| consequence),
                    "preview":text.as_ref().map(|(_, _, preview)| preview),
                    "count":self.state.pending_requests(thread).len(), "focused":self.dock_focused,
                    "question_index":self.question_draft.as_ref().filter(|(id, _)| *id == request.id)
                        .map_or(0, |(_, answers)| answers.len())})
            }))
        })
    }
    #[cfg(debug_assertions)]
    pub fn dump(&self, data_dir: &std::path::Path, cx: &App) -> serde_json::Value {
        let perf = self.engine.perf();
        let mut transcript = self.scroller.read(cx).dump();
        transcript["items"] = serde_json::json!(self.dump_transcript());
        serde_json::json!({
            "pid":std::process::id(), "build_rev":env!("VEYRA_BUILD_REV"), "data_dir":data_dir,
            "appearance":cx.global::<Theme>().appearance.name(), "view":self.view_name(),
            "header_branch":self.branch,
            "sidebar":{"width":f32::from(self.sidebar_width), "rows":self.dump_rows()},
            "selected_thread":self.dump_selected(), "transcript":transcript,
            "composer_text":self.input.read(cx).value().to_string(), "composer":self.dump_composer(cx),
            "providers":self.dump_providers(), "settings":{"default_mode":self.state.settings.default_mode},
            "awake":self.engine.awake(), "processes":self.engine.processes(),
            "perf":{"applied_batches":self.state.applied_batches, "max_batch_gap_ms":self.state.max_batch_gap_ms,
                "commits":perf.commits, "actor_max_ms":perf.actor_max_ms, "actor_busy_ms":perf.actor_busy_ms,
                "repaints":self.repaints, "max_batch_events":self.state.max_batch_events},
            "window_active":self.window_active, "notifications":{"posted":self.notifications_posted},
            "quit_sheet":{"open":self.quit_sheet, "threads":self.busy_threads()},
            "requests":self.state.requests,
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
        #[cfg(debug_assertions)]
        {
            self.repaints += 1;
        }
        let theme = cx.global::<Theme>().clone();
        let root = div()
            .h_flex()
            .size_full()
            .relative()
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
            .on_action(cx.listener(|shell, _: &Stop, window, cx| shell.stop(window, cx)))
            .on_action(cx.listener(|shell, _: &Continue, window, cx| {
                shell.continue_queue(false, window, cx)
            }))
            .on_action(cx.listener(|shell, _: &DiscardQueued, window, cx| {
                shell.continue_queue(true, window, cx)
            }))
            .on_action(cx.listener(|shell, action: &StopThread, window, cx| {
                if let Some(thread) = action.thread.or_else(|| shell.view.chat()) {
                    shell.send_command(Command::StopThread { thread }, window, cx);
                }
            }))
            .on_action(cx.listener(|shell, _: &NextNeedsYou, window, cx| {
                if let Some(thread) = sidebar::next_needs_you(
                    &sidebar::ordered_threads(&shell.state),
                    shell.view.selected(),
                    &shell.state.statuses,
                ) {
                    shell.select_thread(thread, window, cx);
                }
            }))
            .on_action(cx.listener(|shell, _: &AllowOnce, window, cx| {
                shell.respond(Decision::AllowOnce, window, cx)
            }))
            .on_action(cx.listener(|shell, _: &AllowInThread, window, cx| {
                shell.respond(Decision::AllowInThread, window, cx)
            }))
            .on_action(
                cx.listener(|shell, _: &Deny, window, cx| {
                    shell.respond(Decision::Deny, window, cx)
                }),
            )
            .on_action(cx.listener(|shell, action: &AnswerQuestion, window, cx| {
                shell.answer_question(action.option.unwrap_or(0), window, cx);
            }))
            .on_action(cx.listener(|shell, _: &FocusDock, window, cx| {
                if shell
                    .view
                    .chat()
                    .is_some_and(|thread| !shell.state.pending_requests(thread).is_empty())
                {
                    window.focus(&shell.dock_focus, cx);
                }
            }))
            .on_action(cx.listener(|shell, _: &FocusComposer, window, cx| {
                shell.input.update(cx, |input, cx| input.focus(window, cx));
            }))
            .on_action(cx.listener(|shell, _: &ScrollTranscriptUp, window, cx| {
                window.focus(&shell.focus, cx);
                shell.scroller.update(cx, |state, cx| {
                    state.scroll_to_item(0, cx);
                });
            }))
            .on_action(cx.listener(|shell, _: &ScrollTranscriptDown, window, cx| {
                shell
                    .scroller
                    .update(cx, |state, cx| state.scroll_down(window, cx));
            }))
            .on_action(cx.listener(|shell, _: &ScrollTranscriptEnd, _, cx| {
                shell
                    .scroller
                    .update(cx, |state, cx| state.scroll_to_end(cx));
            }))
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
                let item = action.item.or_else(|| {
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
                });
                if let Some(id) = item
                    && !shell.expanded.remove(&id)
                {
                    shell.expanded.insert(id);
                }
                shell.scroller.update(cx, |state, cx| {
                    if let Some(index) = shell.view.chat().and_then(|thread| {
                        shell
                            .state
                            .transcript(thread)
                            .iter()
                            .position(|row| Some(row.id) == item)
                    }) {
                        state.expand(index, cx);
                    } else {
                        state.remeasure(cx);
                    }
                });
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
            .on_action(cx.listener(|shell, _: &Quit, window, cx| {
                if shell.busy_threads().is_empty() {
                    shell.confirm_quit(cx);
                } else {
                    shell.quit_sheet = true;
                    window.focus(&shell.quit_focus, cx);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|shell, _: &ConfirmQuit, _, cx| shell.confirm_quit(cx)))
            .on_action(cx.listener(|shell, _: &CancelQuit, window, cx| {
                if shell.quitting {
                    return;
                }
                shell.quit_sheet = false;
                shell.input.update(cx, |input, cx| input.focus(window, cx));
                cx.notify();
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
        let root = root.when(self.quit_sheet, |root| {
            root.child(self.render_quit_sheet(cx))
        });
        #[cfg(debug_assertions)]
        let root = root.on_action(cx.listener(|_, action: &DebugStallUi, window, cx| {
            let milliseconds = action.milliseconds;
            cx.defer_in(window, move |_, _, _| {
                std::thread::sleep(std::time::Duration::from_millis(milliseconds));
            });
        }));
        #[cfg(debug_assertions)]
        let root = root.on_action(cx.listener(
            |shell, action: &DebugScrollTranscript, window, cx| {
                if action.pixels.is_finite() {
                    shell.scroller.update(cx, |state, cx| {
                        state.scroll_by(action.pixels.into(), window, cx)
                    });
                }
            },
        ));
        #[cfg(debug_assertions)]
        let root = root.on_action(cx.listener(|_, _: &ActivateWindow, window, cx| {
            cx.activate(true);
            window.activate_window();
            window.refresh();
        }));
        #[cfg(debug_assertions)]
        let root = root.on_action(cx.listener(
            |shell, action: &DebugNotificationResponse, window, cx| {
                if let Some(tag) = &action.tag {
                    shell.notification_response(tag, window, cx);
                }
            },
        ));
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
            cx.set_app_identity("dev.veyra.app", "Veyra");
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
                let notifications = shell.downgrade();
                let handle = window.window_handle();
                cx.on_system_notification_response(move |response, cx| {
                    let _ = handle.update(cx, |_, window, cx| {
                        let _ = notifications.update(cx, |shell, cx| {
                            shell.notification_response(response.tag.as_ref(), window, cx);
                        });
                    });
                });
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
    use gpui::{
        AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
        Styled, Window, div,
    };
    use std::collections::BTreeMap;
    use veyra_engine::{
        domain::*,
        events::{ChangeEvent, ThreadSnapshot, UiEvent},
        provider::{Probe, ProviderKind, Readiness},
    };

    fn draw(cx: &mut gpui::VisualTestContext) {
        for _ in 0..2 {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        cx.run_until_parked();
    }

    fn with_docked_shell(
        cx: &mut gpui::TestAppContext,
        check: impl FnOnce(
            Entity<super::Shell>,
            &mut gpui::VisualTestContext,
            std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
            tokio::sync::mpsc::UnboundedSender<UiEvent>,
        ),
    ) {
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        struct ActionCapture {
            shell: Entity<super::Shell>,
            dispatched: std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>,
        }
        impl Render for ActionCapture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let dispatched = self.dispatched.clone();
                let allowed = self.dispatched.clone();
                let denied = self.dispatched.clone();
                let granted = self.dispatched.clone();
                div()
                    .size_full()
                    .capture_action(move |_: &super::Stop, _, _| {
                        dispatched.borrow_mut().push("Stop");
                    })
                    .capture_action(move |_: &super::AllowOnce, _, _| {
                        allowed.borrow_mut().push("AllowOnce");
                    })
                    .capture_action(move |_: &super::Deny, _, _| {
                        denied.borrow_mut().push("Deny");
                    })
                    .capture_action(move |_: &super::AllowInThread, _, _| {
                        granted.borrow_mut().push("AllowInThread");
                    })
                    .child(self.shell.clone())
            }
        }
        let scratch = Scratch(
            std::path::PathBuf::from("/tmp").join(format!("veyra-dock-{}", ThreadId::new())),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let engine = veyra_engine::Engine::start_with_processes(
            runtime.handle(),
            veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
            vec![],
        )
        .unwrap();
        runtime.block_on(engine.shutdown_wait());
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        cx.update(|cx| {
            gpui_kit::init(cx);
            super::actions::init(cx);
        });
        let dispatched = std::rc::Rc::new(std::cell::RefCell::new(vec![]));
        let (view, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(super::Appearance::Dark, window, cx);
            let shell = cx.new(|cx| {
                let mut shell = super::Shell::new(
                    engine,
                    runtime.handle().clone(),
                    events,
                    scratch.0.clone(),
                    window,
                    cx,
                );
                shell.probes = Default::default();
                shell.providers = vec![Probe {
                    provider: ProviderKind::Claude,
                    path: None,
                    version: None,
                    readiness: Readiness::Ready,
                    models: vec![],
                    account: None,
                }];
                let thread = ThreadId::new();
                let turn = TurnId::new();
                shell.view = super::MainView::Chat(thread);
                shell.state.turns.insert(
                    turn,
                    Turn {
                        id: turn,
                        thread,
                        ordinal: 1,
                        origin: TurnOrigin::Provider,
                        state: TurnState::Running { since: 0 },
                    },
                );
                shell.state.requests = (0..2)
                    .map(|index| Request {
                        id: RequestId::new(),
                        thread,
                        turn,
                        native: NativeRequestRef(format!("question-{index}")),
                        state: RequestState::Pending,
                        kind: RequestKind::Question {
                            questions: vec![Question {
                                text: "Choose a route".into(),
                                options: vec!["Short".into(), "Long".into()],
                            }],
                        },
                    })
                    .collect();
                shell.input.update(cx, |input, cx| input.focus(window, cx));
                shell
            });
            ActionCapture {
                shell,
                dispatched: dispatched.clone(),
            }
        });
        let shell = view.read_with(cx, |view, _| view.shell.clone());
        cx.run_until_parked();
        draw(cx);
        shell.update_in(cx, |shell, window, cx| {
            shell.input.update(cx, |input, cx| input.focus(window, cx));
        });
        draw(cx);
        cx.update(|window, _| window.activate_window());
        draw(cx);
        check(shell, cx, dispatched, sender);
    }

    #[gpui::test]
    fn empty_send_dispatches_stop_action(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, _| {
            shell.update_in(cx, |shell, window, cx| shell.send(window, cx));
            cx.run_until_parked();
            draw(cx);
            assert_eq!(
                dispatched.borrow().as_slice(),
                ["Stop"],
                "empty Send must reach the window's Stop action"
            );
            cx.dispatch_action(super::Stop);
            cx.run_until_parked();
            draw(cx);
            assert_eq!(dispatched.borrow().as_slice(), ["Stop", "Stop"]);
        });
    }

    #[gpui::test]
    fn long_approval_keeps_decision_buttons_visible(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let command = format!(
                "cat <<'END'\n{}END",
                "long approval command line\n".repeat(200)
            );
            shell.update(cx, |shell, cx| {
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        tool: "command tool".into(),
                        target: ApprovalTarget::Command {
                            command: command.clone(),
                            preview: ApprovalPreview::new("argument preview\n".repeat(200)),
                        },
                    },
                    grant_key: Some(GrantKey {
                        tool: "command tool".into(),
                        command: command.clone(),
                    }),
                };
                cx.notify();
            });
            draw(cx);
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            let text = cx.debug_bounds("approval-details").unwrap();
            let deny = cx.debug_bounds("approval-deny-button").unwrap();
            let allow = cx.debug_bounds("approval-allow-button").unwrap();
            let viewport = cx.update(|window, _| window.viewport_size());
            assert!(
                text.size.height <= theme.layout.composer_max_height,
                "command exceeds bounded approval area: {:?}",
                text.size
            );
            for button in [deny, allow] {
                assert!(
                    button.origin.y >= gpui::Pixels::ZERO && button.bottom() <= viewport.height,
                    "decision button is off-screen: {button:?}"
                );
            }
            let dock = shell.read_with(cx, |shell, cx| shell.dump_composer(cx)["dock"].clone());
            assert_eq!(dock["action_text"], command, "command must remain whole");
            let before = cx.debug_bounds("approval-action-text").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: text.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(
                    gpui::Pixels::ZERO,
                    -theme.layout.composer_max_height,
                )),
                touch_phase: gpui::TouchPhase::Moved,
                ..Default::default()
            });
            draw(cx);
            let after = cx.debug_bounds("approval-action-text").unwrap();
            assert!(
                after.origin.y < before.origin.y,
                "approval command did not scroll"
            );
            assert_eq!(cx.debug_bounds("approval-deny-button").unwrap(), deny);
            assert_eq!(cx.debug_bounds("approval-allow-button").unwrap(), allow);
        });
    }

    #[gpui::test]
    fn file_approval_dock_names_target_and_consequence(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            for (operation, location, consequence) in [
                (
                    FileOperation::Write,
                    TargetLocation::OutsideProject,
                    "Claude Code writes a file outside the project folder.",
                ),
                (
                    FileOperation::Edit,
                    TargetLocation::InsideProject,
                    "Claude Code edits a file inside the project folder.",
                ),
                (
                    FileOperation::Write,
                    TargetLocation::NotVerified,
                    "Claude Code writes a file at a location that is not verified.",
                ),
            ] {
                shell.update(cx, |shell, _| {
                    shell.state.requests.truncate(1);
                    shell.state.requests[0].kind = RequestKind::Approval {
                        action: ProposedAction {
                            tool: "file tool".into(),
                            target: ApprovalTarget::File {
                                operation,
                                location,
                                path: "/tmp/x".into(),
                                preview: ApprovalPreview::new("contents to approve".into()),
                            },
                        },
                        grant_key: None,
                    };
                });
                let dock = shell.read_with(cx, |shell, cx| shell.dump_composer(cx)["dock"].clone());
                assert_eq!(dock["action_text"], "file tool /tmp/x");
                assert_eq!(dock["consequence"], consequence);
                assert_eq!(dock["preview"], "contents to approve");
            }
            let command = "{\"file_path\":\"/tmp/x\"}";
            shell.update(cx, |shell, _| {
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: command.into(),
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
                    grant_key: None,
                };
            });
            let dock = shell.read_with(cx, |shell, cx| shell.dump_composer(cx)["dock"].clone());
            assert_eq!(dock["action_text"], command);
            assert_eq!(
                dock["consequence"],
                "Claude Code will run this command in the project folder."
            );
        });
    }

    #[gpui::test]
    fn last_approval_returns_focus_to_composer(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: "echo ok".into(),
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
                    grant_key: None,
                };
                cx.notify();
            });
            draw(cx);
            cx.simulate_keystrokes("secondary-shift-y");
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| shell.dock_focused));
            cx.simulate_keystrokes("secondary-enter");
            draw(cx);
            assert!(!shell.read_with(cx, |shell, _| shell.dock_focused));
        });
    }

    #[gpui::test]
    fn question_focus_prompt_matches_approval(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            assert!(cx.debug_bounds("veyra::FocusDock").is_some());
            cx.simulate_keystrokes("secondary-shift-y");
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| shell.dock_focused));
            assert!(cx.debug_bounds("veyra::FocusDock").is_none());
        });
    }

    #[gpui::test]
    fn allow_in_thread_chip_follows_keyboard_focus(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, sender| {
            shell.update(cx, |shell, cx| {
                for request in &mut shell.state.requests {
                    request.kind = RequestKind::Approval {
                        action: ProposedAction {
                            tool: "Bash".into(),
                            target: ApprovalTarget::Command {
                                command: "echo ok".into(),
                                preview: ApprovalPreview::new(String::new()),
                            },
                        },
                        grant_key: Some(GrantKey {
                            tool: "Bash".into(),
                            command: "echo ok".into(),
                        }),
                    };
                }
                cx.notify();
            });
            draw(cx);
            assert!(cx.debug_bounds("veyra::AllowInThread").is_none());
            cx.simulate_keystrokes("secondary-alt-enter");
            draw(cx);
            assert!(dispatched.borrow().is_empty());
            cx.simulate_keystrokes("secondary-shift-y");
            draw(cx);
            assert!(cx.debug_bounds("veyra::AllowInThread").is_some());
            cx.simulate_keystrokes("secondary-alt-enter");
            draw(cx);
            assert_eq!(dispatched.borrow().as_slice(), ["AllowInThread"]);
            assert!(shell.read_with(cx, |shell, _| shell.dock_focused));
            let mut answered = shell.read_with(cx, |shell, _| shell.state.requests[0].clone());
            answered.state = RequestState::Answered(Decision::AllowInThread);
            sender
                .send(UiEvent::Change {
                    counter: Some(1),
                    event: ChangeEvent::RequestClosed(answered),
                })
                .unwrap();
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell
                    .state
                    .pending_requests(shell.view.chat().unwrap())
                    .len()),
                1
            );
            cx.simulate_keystrokes("secondary-alt-enter");
            draw(cx);
            assert_eq!(
                dispatched.borrow().as_slice(),
                ["AllowInThread", "AllowInThread"]
            );
            assert!(!shell.read_with(cx, |shell, _| shell.dock_focused));
            assert!(cx.debug_bounds("veyra::AllowInThread").is_none());
        });
    }

    #[gpui::test]
    fn approval_chips_follow_keyboard_focus(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, _| {
            shell.update(cx, |shell, cx| {
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: "touch veyra-check.txt".into(),
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
                    grant_key: None,
                };
                cx.notify();
            });
            cx.run_until_parked();
            draw(cx);
            assert!(
                cx.debug_bounds("veyra::AllowOnce").is_none(),
                "composer Return does not allow"
            );
            assert!(
                cx.debug_bounds("veyra::Deny").is_none(),
                "composer period does not deny"
            );
            assert!(cx.debug_bounds("veyra::FocusDock").is_some());
            cx.simulate_keystrokes("secondary-enter");
            cx.run_until_parked();
            draw(cx);
            assert_eq!(dispatched.borrow().as_slice(), ["Stop"]);
            cx.simulate_keystrokes("secondary-.");
            draw(cx);
            assert_eq!(dispatched.borrow().as_slice(), ["Stop"]);
            cx.simulate_keystrokes("secondary-shift-y");
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| shell.dock_focused));
            assert!(cx.debug_bounds("veyra::AllowOnce").is_some());
            assert!(cx.debug_bounds("veyra::Deny").is_some());
            assert!(cx.debug_bounds("veyra::FocusDock").is_none());
            cx.simulate_keystrokes("secondary-enter");
            cx.run_until_parked();
            draw(cx);
            assert_eq!(dispatched.borrow().as_slice(), ["Stop", "AllowOnce"]);
            cx.simulate_keystrokes("secondary-shift-y");
            draw(cx);
            cx.simulate_keystrokes("secondary-.");
            draw(cx);
            assert_eq!(
                dispatched.borrow().as_slice(),
                ["Stop", "AllowOnce", "Deny"]
            );
            cx.simulate_keystrokes("escape");
            cx.run_until_parked();
            draw(cx);
            assert!(!shell.read_with(cx, |shell, _| shell.dock_focused));
            assert!(cx.debug_bounds("veyra::AllowOnce").is_none());
        });
    }

    #[gpui::test]
    fn answering_one_of_several_requests_keeps_dock_focus(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, sender| {
            cx.simulate_keystrokes("secondary-shift-y");
            cx.run_until_parked();
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| shell.dock_focused));
            cx.simulate_keystrokes("1");
            cx.run_until_parked();
            draw(cx);
            assert!(
                shell.read_with(cx, |shell, _| shell.dock_focused),
                "another request still needs the dock"
            );
            let mut answered = shell.read_with(cx, |shell, _| shell.state.requests[0].clone());
            answered.state = RequestState::Answered(Decision::Answers(vec![Answer {
                text: "Short".into(),
            }]));
            sender
                .send(UiEvent::Change {
                    counter: Some(1),
                    event: ChangeEvent::RequestClosed(answered),
                })
                .unwrap();
            cx.run_until_parked();
            draw(cx);
            cx.simulate_keystrokes("1");
            cx.run_until_parked();
            draw(cx);
            assert!(
                !shell.read_with(cx, |shell, _| shell.dock_focused),
                "last answer returns to composer"
            );
            assert!(shell.read_with(cx, |shell, _| shell.question_draft.is_none()));
        });
    }

    #[gpui::test]
    fn native_app_quit_closes_engine_without_quit_action(cx: &mut gpui::TestAppContext) {
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let scratch = Scratch(
            std::path::PathBuf::from("/tmp").join(format!("veyra-quit-{}", ThreadId::new())),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let engine = veyra_engine::Engine::start_with_processes(
            runtime.handle(),
            veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
            vec![],
        )
        .unwrap();
        let (_, events) = tokio::sync::mpsc::unbounded_channel();
        cx.update(gpui_kit::init);
        cx.skip_drawing();
        let _window = cx.add_window_view(|window, cx| {
            super::Theme::apply(super::Appearance::Dark, window, cx);
            let mut shell = super::Shell::new(
                engine.clone(),
                runtime.handle().clone(),
                events,
                scratch.0.clone(),
                window,
                cx,
            );
            shell.probes = Default::default();
            shell
        });
        cx.update(|cx| cx.shutdown());
        let result = runtime.block_on(engine.send(
            veyra_engine::commands::Command::SetDefaultMode {
                mode: PermissionMode::AskFirst,
            },
        ));
        runtime.block_on(engine.shutdown_wait());
        drop(engine);
        assert!(matches!(
            result.unwrap(),
            Err(veyra_engine::commands::CommandError::Closed)
        ));
    }

    #[gpui::test]
    fn chat_branch_loads_when_the_thread_row_follows_selection(cx: &mut gpui::TestAppContext) {
        use std::{path::PathBuf, process::Command, time::Duration};

        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).expect("remove owned scratch");
            }
        }

        let thread = ThreadId::new();
        let path = PathBuf::from("/tmp").join(format!("veyra-branch-{thread}"));
        std::fs::create_dir(&path).unwrap();
        let scratch = Scratch(path);
        assert!(
            Command::new("git")
                .args(["init", "-q", "-b", "fixture-branch"])
                .current_dir(&scratch.0)
                .status()
                .unwrap()
                .success()
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let engine = veyra_engine::Engine::start_with_processes(
            runtime.handle(),
            veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
            vec![],
        )
        .unwrap();
        runtime.block_on(engine.shutdown_wait());
        let (sender, events) = tokio::sync::mpsc::unbounded_channel();
        cx.update(gpui_kit::init);
        cx.skip_drawing();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(super::Appearance::Dark, window, cx);
            let mut shell = super::Shell::new(
                engine,
                runtime.handle().clone(),
                events,
                scratch.0.clone(),
                window,
                cx,
            );
            shell.probes = Default::default();
            shell
        });
        shell.update_in(cx, |shell, window, cx| {
            shell.select_thread(thread, window, cx)
        });
        cx.run_until_parked();
        sender
            .send(UiEvent::Change {
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
                    cwd: scratch.0.clone(),
                    native: None,
                    native_cursor: None,
                    cli_version: None,
                    hold: None,
                    view: View::Chat,
                    import_pending: false,
                    archived: false,
                }),
            })
            .unwrap();
        let end = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < end {
            runtime.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
            cx.run_until_parked();
            if shell.read_with(cx, |shell, _| shell.branch == "fixture-branch") {
                break;
            }
        }
        shell.update(cx, |shell, _| shell.engine.shutdown());
        assert_eq!(
            shell.read_with(cx, |shell, cx| shell.dump(&scratch.0, cx))["header_branch"],
            "fixture-branch"
        );
    }

    #[test]
    fn retry_resends_failed_turn_prompt_with_newer_queued_message() {
        let thread = ThreadId::new();
        let mut state = AppState::default();
        for (ordinal, text, turn_state) in [
            (
                1,
                "failed prompt",
                TurnState::Ended {
                    outcome: TurnOutcome::Failed {
                        message: "failure".into(),
                    },
                    started: Some(1),
                    at: 2,
                },
            ),
            (2, "queued prompt", TurnState::Queued),
        ] {
            let turn = TurnId::new();
            let item = ItemId::new();
            state.turns.insert(
                turn,
                Turn {
                    id: turn,
                    thread,
                    ordinal,
                    origin: TurnOrigin::User { item },
                    state: turn_state,
                },
            );
            state.items.entry(thread).or_default().insert(
                item,
                Item {
                    id: item,
                    thread,
                    turn,
                    seq: 0,
                    body: ItemBody::UserText(text.into()),
                },
            );
        }
        assert_eq!(state.retry_prompt(thread), Some("failed prompt".into()));
    }

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
                    draining: None,
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

    #[gpui::test]
    fn escape_after_confirm_quit_keeps_sheet(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update_in(cx, |shell, window, cx| {
                shell.quit_sheet = true;
                window.focus(&shell.quit_focus, cx);
                cx.notify();
            });
            draw(cx);
            cx.simulate_keystrokes("escape");
            draw(cx);
            assert!(!shell.read_with(cx, |shell, _| shell.quit_sheet));
            shell.update_in(cx, |shell, window, cx| {
                shell.quit_sheet = true;
                window.focus(&shell.quit_focus, cx);
                cx.notify();
            });
            draw(cx);
            cx.dispatch_action(super::ConfirmQuit);
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| shell.quitting));
            cx.simulate_keystrokes("escape");
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| shell.quit_sheet));
        });
    }

    #[gpui::test]
    fn recheck_after_confirm_quit_launches_no_provider(cx: &mut gpui::TestAppContext) {
        use std::time::Duration;
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        if std::env::var_os("TEST_QUIT_PROBE_MARKER").is_none() {
            let scratch = Scratch(
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("../../target/quit-probe-{}", ThreadId::new())),
            );
            std::fs::create_dir_all(&scratch.0).unwrap();
            let binary = scratch.0.join("owned-probe-stand-in");
            std::fs::write(
                &binary,
                concat!(
                    "#!/bin/sh\nprintf launched >> \"$TEST_QUIT_PROBE_MARKER\"\n",
                    "if [ \"$1\" = --version ]; then printf '1.0.0\\n'; exit; fi\n",
                    "read initialize\nprintf '%s\\n' '",
                    "{\"type\":\"control_response\",\"response\":{\"subtype\":\"success\",",
                    "\"request_id\":\"veyra-initialize\",\"response\":{}}}'\n",
                ),
            )
            .unwrap();
            assert!(
                std::process::Command::new("/bin/chmod")
                    .arg("700")
                    .arg(&binary)
                    .status()
                    .unwrap()
                    .success()
            );
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ui::tests::recheck_after_confirm_quit_launches_no_provider",
                ])
                .env("VEYRA_CLAUDE_BIN", &binary)
                .env("TEST_QUIT_PROBE_MARKER", scratch.0.join("marker"))
                .env_remove("VEYRA_FAKE_FIXTURES")
                .env_remove("VEYRA_RECORD")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stdout)
            );
            return;
        }
        let marker = std::path::PathBuf::from(std::env::var_os("TEST_QUIT_PROBE_MARKER").unwrap());
        let scratch = Scratch(
            std::path::PathBuf::from("/tmp").join(format!("veyra-quit-probe-{}", ThreadId::new())),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let engine = veyra_engine::Engine::start_with_processes(
            runtime.handle(),
            veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
            vec![],
        )
        .unwrap();
        let (_sender, events) = tokio::sync::mpsc::unbounded_channel();
        cx.update(gpui_kit::init);
        cx.skip_drawing();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(super::Appearance::Dark, window, cx);
            let mut shell = super::Shell::new(
                engine.clone(),
                runtime.handle().clone(),
                events,
                scratch.0.clone(),
                window,
                cx,
            );
            shell.probes = Default::default();
            shell
        });
        let mut probes = veyra_engine::provider::Probes::default();
        let control = probes.check(
            runtime.handle(),
            ProviderKind::Claude,
            veyra_engine::provider::env::ChildEnv::load(),
            scratch.0.clone(),
        );
        let positive = runtime.block_on(control).unwrap();
        assert_eq!(positive.readiness, Readiness::Ready);
        assert!(
            marker.exists(),
            "positive probe path={:?}; override={:?}; marker={:?}",
            positive.path,
            std::env::var_os("VEYRA_CLAUDE_BIN"),
            marker
        );
        assert!(!std::fs::read_to_string(&marker).unwrap().is_empty());
        std::fs::remove_file(&marker).unwrap();
        shell.update_in(cx, |shell, _, cx| shell.confirm_quit(cx));
        cx.run_until_parked();
        let refused = runtime.block_on(engine.send(
            veyra_engine::commands::Command::SetDefaultMode {
                mode: PermissionMode::AskFirst,
            },
        ));
        assert!(matches!(
            refused.unwrap(),
            Err(veyra_engine::commands::CommandError::Closed)
        ));
        shell.update_in(cx, |shell, window, cx| {
            assert!(shell.quitting);
            shell.recheck(false, window, cx);
        });
        runtime.block_on(async { tokio::time::sleep(Duration::from_secs(2)).await });
        let launched = std::fs::read_to_string(&marker).unwrap_or_default();
        shell.update(cx, |shell, _| shell.probes = Default::default());
        runtime.block_on(engine.shutdown_wait());
        assert!(
            launched.is_empty(),
            "a provider launched after quit began: {launched}"
        );
    }
}
