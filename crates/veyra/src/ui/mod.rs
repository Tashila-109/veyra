pub mod actions;
mod composer;
mod header;
mod new_thread;
mod notify;
mod provider_text;
mod settings;
mod sidebar;
mod terminal_view;
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
use veyra_engine::domain::View;
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
struct TranscriptOrder {
    ids: Vec<ItemId>,
    positions: BTreeMap<ItemId, usize>,
}
#[derive(Default)]
struct BatchGap {
    maximum_ms: f64,
    last: Option<Instant>,
}
impl BatchGap {
    fn note(&mut self) {
        let now = Instant::now();
        if let Some(last) = self.last {
            self.maximum_ms = self
                .maximum_ms
                .max(now.duration_since(last).as_secs_f64() * 1000.);
        }
        self.last = Some(now);
    }
}
#[cfg(debug_assertions)]
#[derive(Default)]
struct VisiblePerf {
    thread: Option<ThreadId>,
    applied_batches: u64,
    gap: BatchGap,
}
#[cfg(debug_assertions)]
impl VisiblePerf {
    fn apply(&mut self, events: &[UiEvent]) {
        let Some(thread) = self.thread else {
            return;
        };
        if events.iter().any(|event| {
            matches!(event,
            UiEvent::Change { event: ChangeEvent::TurnStarted(turn), .. } if turn.thread == thread)
        }) {
            *self = Self {
                thread: Some(thread),
                ..Self::default()
            };
        }
        if events.iter().any(|event| {
            matches!(event,
            UiEvent::TextAppended { thread: id, .. } if *id == thread)
        }) {
            self.gap.note();
            self.applied_batches += 1;
        }
    }
}
#[cfg(debug_assertions)]
struct TranscriptOpen {
    thread: ThreadId,
    request: SnapshotRequest,
    requested: Instant,
    installed: bool,
    milliseconds: Option<f64>,
}
#[derive(Default)]
struct AppState {
    projects: BTreeMap<ProjectId, Project>,
    threads: BTreeMap<ThreadId, Thread>,
    turns: BTreeMap<TurnId, Turn>,
    tasks: BTreeMap<TaskId, veyra_engine::domain::Task>,
    items: BTreeMap<ThreadId, BTreeMap<ItemId, Item>>,
    item_order: BTreeMap<ThreadId, TranscriptOrder>,
    markdown_text: BTreeMap<ItemId, SharedString>,
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
    gap: BatchGap,
    last_activity: BTreeMap<ThreadId, Millis>,
    #[cfg(debug_assertions)]
    visible_perf: VisiblePerf,
    #[cfg(debug_assertions)]
    transcript_open: Option<TranscriptOpen>,
}
fn stop_target(
    thread: ThreadId,
    turns: &BTreeMap<TurnId, Turn>,
    published: Option<&veyra_engine::WindowThread>,
) -> Option<StopTarget> {
    let turn = turns
        .values()
        .find(|turn| turn.thread == thread && matches!(turn.state, TurnState::Running { .. }))
        .map(|turn| turn.id)
        .or_else(|| published.and_then(|state| state.pending_turn))?;
    Some(StopTarget { thread, turn })
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
            self.visible_perf.apply(&events);
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
            self.gap.last = None;
        }
        if events
            .iter()
            .any(|event| matches!(event, UiEvent::TextAppended { .. }))
        {
            self.gap.note();
        }
        for event in events {
            self.apply(event);
        }
    }
    fn apply(&mut self, event: UiEvent) {
        if let Some((thread, time)) = header::note_activity(&event, veyra_engine::actor::now()) {
            self.last_activity.insert(thread, time);
        }
        if let UiEvent::Change { event, .. } = &event {
            match event {
                ChangeEvent::ProjectAdded(project) => {
                    self.projects.insert(project.id, project.clone());
                }
                ChangeEvent::ThreadCreated(thread) | ChangeEvent::ThreadUpdated(thread) => {
                    self.threads.insert(thread.id, thread.as_ref().clone());
                }
                ChangeEvent::ThreadArchived { thread } => {
                    if let Some(row) = self.threads.get_mut(thread) {
                        row.archived = true;
                    }
                    self.last_activity.remove(thread);
                    self.item_order.remove(thread);
                    if let Some(items) = self.items.remove(thread) {
                        for id in items.keys() {
                            self.markdown_text.remove(id);
                        }
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
                    self.sort_transcript(turn.thread);
                }
                ChangeEvent::SettingsChanged(settings) => self.settings = *settings,
                ChangeEvent::RequestOpened(request) | ChangeEvent::RequestClosed(request) => {
                    if let Some(row) = self.requests.iter_mut().find(|row| row.id == request.id) {
                        *row = request.as_ref().clone();
                    } else {
                        self.requests.push(request.as_ref().clone());
                    }
                }
                ChangeEvent::TaskStarted(task)
                | ChangeEvent::TaskSettled(task)
                | ChangeEvent::TaskDelivered(task) => {
                    self.tasks.insert(task.id, task.clone());
                }
                ChangeEvent::ItemAdded(_) | ChangeEvent::ItemCompleted(_) => {}
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
                let thread = item.thread;
                let id = item.id;
                if let Some(text) = transcript::markdown_text(&item) {
                    self.markdown_text.insert(id, text);
                } else {
                    self.markdown_text.remove(&id);
                }
                let old = self.items.entry(thread).or_default().insert(id, item);
                if old.is_none() {
                    self.sort_transcript(thread);
                }
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
                    if let Some(text) = transcript::markdown_text(item) {
                        self.markdown_text.insert(item.id, text);
                    }
                }
            }
            _ => {}
        }
    }
    fn begin_snapshot(&mut self, thread: ThreadId) -> SnapshotRequest {
        self.snapshot_sequence += 1;
        let request = SnapshotRequest(self.snapshot_sequence);
        #[cfg(debug_assertions)]
        {
            self.transcript_open = Some(TranscriptOpen {
                thread,
                request,
                requested: Instant::now(),
                installed: false,
                milliseconds: None,
            });
        }
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
        #[cfg(debug_assertions)]
        if let Some(open) = &mut self.transcript_open
            && open.thread == thread
            && open.request == request
        {
            open.installed = true;
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
        if let Some(old) = self.items.get(&thread) {
            for id in old.keys() {
                self.markdown_text.remove(id);
            }
        }
        for item in items.values() {
            if let Some(text) = transcript::markdown_text(item) {
                self.markdown_text.insert(item.id, text);
            }
        }
        self.items.insert(thread, items);
        self.sort_transcript(thread);
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
    fn sort_transcript(&mut self, thread: ThreadId) {
        let Some(items) = self.items.get(&thread) else {
            return;
        };
        let mut imports: Vec<_> = items
            .values()
            .filter(|item| matches!(item.body, ItemBody::TerminalSpan(_)))
            .filter_map(|item| self.turns.get(&item.turn).map(|turn| turn.ordinal))
            .collect();
        imports.sort_unstable();
        imports.dedup();
        let mut ids: Vec<_> = items.keys().copied().collect();
        ids.sort_by_key(|id| {
            let item = &items[id];
            let ordinal = self
                .turns
                .get(&item.turn)
                .map_or(u32::MAX, |turn| turn.ordinal);
            let section = imports.partition_point(|&import| import < ordinal) * 2
                + usize::from(imports.binary_search(&ordinal).is_ok());
            (
                section,
                self.turns
                    .get(&item.turn)
                    .map(Turn::recency)
                    .unwrap_or((true, i64::MAX, 0)),
                item.seq,
            )
        });
        let positions = ids
            .iter()
            .enumerate()
            .map(|(index, &id)| (id, index))
            .collect();
        self.item_order
            .insert(thread, TranscriptOrder { ids, positions });
    }
    fn transcript_len(&self, thread: ThreadId) -> usize {
        self.item_order
            .get(&thread)
            .map_or(0, |order| order.ids.len())
    }
    fn transcript_rows(&self, thread: ThreadId) -> usize {
        self.transcript_len(thread) + self.family_offset(thread)
    }
    fn family_offset(&self, thread: ThreadId) -> usize {
        usize::from(!transcript::family_children(self, thread).is_empty())
    }
    fn transcript_position(&self, thread: ThreadId, item: ItemId) -> Option<usize> {
        self.item_order
            .get(&thread)?
            .positions
            .get(&item)
            .map(|index| *index + self.family_offset(thread))
    }
    fn transcript_item(&self, thread: ThreadId, index: usize) -> Option<&Item> {
        let id = self.item_order.get(&thread)?.ids.get(index)?;
        self.items.get(&thread)?.get(id)
    }
    fn text_rows(&self, batch: &[UiEvent], chat: Option<ThreadId>) -> Option<BTreeSet<usize>> {
        batch
            .iter()
            .all(|event| matches!(event, UiEvent::TextAppended { .. }))
            .then(|| {
                batch
                    .iter()
                    .filter_map(|event| {
                        let UiEvent::TextAppended { thread, item, .. } = event else {
                            return None;
                        };
                        if Some(*thread) != chat {
                            return None;
                        }
                        self.transcript_position(*thread, *item)
                    })
                    .collect()
            })
    }
    fn transcript(&self, thread: ThreadId) -> Vec<Item> {
        (0..self.transcript_len(thread))
            .filter_map(|index| self.transcript_item(thread, index).cloned())
            .collect()
    }
    fn parent(&self, thread: ThreadId) -> Option<&Thread> {
        let Lineage::Child { parent, .. } = self.threads.get(&thread)?.lineage else {
            return None;
        };
        self.threads.get(&parent)
    }
    fn quiet_minutes(&self, thread: ThreadId) -> Option<Millis> {
        let open_tool = self.items.get(&thread).is_some_and(|items| {
            items
                .values()
                .any(|item| matches!(&item.body, ItemBody::Tool(tool) if tool.result.is_none()))
        });
        header::quiet_for(
            &self.status(thread),
            self.last_activity.get(&thread).copied(),
            veyra_engine::actor::now(),
            open_tool,
            !self.pending_requests(thread).is_empty(),
        )
    }
    fn header_status(&self, thread: ThreadId) -> String {
        let mut text = header::status_text(&self.status(thread));
        if let Some(minutes) = self.quiet_minutes(thread) {
            text.push_str(&format!(" · Quiet for {minutes} min"));
        }
        text
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
            .max_by_key(|turn| turn.recency())?;
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
    fn failed_turn(&self, thread: ThreadId) -> Option<&Turn> {
        self.turns
            .values()
            .filter(|turn| turn.thread == thread && turn.state != TurnState::Queued)
            .max_by_key(|turn| turn.recency())
            .filter(|turn| {
                matches!(
                    turn.state,
                    TurnState::Ended {
                        outcome: TurnOutcome::Failed { .. },
                        ..
                    }
                )
            })
    }
    fn failure(&self, thread: ThreadId) -> Option<&str> {
        match &self.failed_turn(thread)?.state {
            TurnState::Ended {
                outcome: TurnOutcome::Failed { message },
                ..
            } => Some(message.as_str()),
            _ => None,
        }
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
    values.get(index.map(|index| index + 1).unwrap_or(0))
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct DockTarget {
    request: RequestId,
    question: usize,
}

#[derive(Default)]
struct ApprovalScroll {
    request: Option<RequestId>,
    handle: ScrollHandle,
    folder_handle: ScrollHandle,
}
#[derive(Default)]
struct QuestionScroll {
    target: Option<DockTarget>,
    handle: ScrollHandle,
    options: ScrollHandle,
}
pub struct Shell {
    #[cfg(test)]
    sent_responses: std::rc::Rc<std::cell::RefCell<Vec<(RequestId, Decision)>>>,
    #[cfg(test)]
    sent_interrupts: Vec<StopTarget>,
    #[cfg(test)]
    sent_archives: Vec<(ThreadId, bool)>,
    #[cfg(debug_assertions)]
    repaints: u64,
    terminal: Option<(ThreadId, Entity<terminal_view::TerminalView>)>,
    view_refusal: Option<String>,
    focus: FocusHandle,
    dock_focus: FocusHandle,
    approval_scroll: ApprovalScroll,
    question_scroll: QuestionScroll,
    input_docked: bool,
    dock_focused: bool,
    rendered_stop: Option<StopTarget>,
    quit_focus: FocusHandle,
    quit_sheet: bool,
    window_active: bool,
    notifications_posted: u64,
    question_draft: Option<(RequestId, Vec<Answer>)>,
    sidebar_width: Pixels,
    sidebar_scroll: sidebar::SidebarScroll,
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
    permission_picker_open: bool,
    thread_menu: Option<(ThreadId, Entity<PopupMenu>)>,
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
            shell.begin_quitting();
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
        cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.recv().await {
                let mut batch = vec![first];
                while let Ok(event) = events.try_recv() {
                    batch.push(event);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        let old_default = shell.state.settings.default_mode;
                        for event in &batch {
                            match event {
                                UiEvent::StorageBlocked { error } => {
                                    shell.notice = error.clone().map(Into::into)
                                }
                                UiEvent::RecoveryNotice(notice) => {
                                    shell.notice = Some(notice.clone().into())
                                }
                                _ => {}
                            }
                        }
                        let families: BTreeMap<_, _> = shell
                            .state
                            .threads
                            .values()
                            .map(|thread| (thread.root(), shell.state.family_status(thread.root())))
                            .collect();
                        let text_rows = shell.state.text_rows(&batch, shell.view.chat());
                        let previous_family_offset = shell
                            .view
                            .chat()
                            .map_or(0, |thread| shell.state.family_offset(thread));
                        shell.state.apply_all(batch);
                        shell.leave_archived_thread(window, cx);
                        shell.post_banners(&families, cx);
                        shell.start_branch_lookup(cx);
                        if shell.project.is_none() {
                            shell.project = shell.state.projects.keys().next().copied();
                        }
                        if old_default != shell.state.settings.default_mode
                            && shell.view.chat().is_none()
                        {
                            shell.config.mode = shell.default_run_mode();
                        }
                        let count = shell
                            .view
                            .chat()
                            .map(|thread| shell.state.transcript_rows(thread))
                            .unwrap_or_default();
                        let family_offset = shell
                            .view
                            .chat()
                            .map_or(0, |thread| shell.state.family_offset(thread));
                        shell.scroller.update(cx, |state, cx| {
                            if family_offset > previous_family_offset {
                                state.prepend(cx);
                            }
                            if count > state.item_count() {
                                state.append(count - state.item_count(), cx);
                            } else if count < state.item_count() {
                                state.reset(count, cx);
                            }
                            if let Some(rows) = &text_rows {
                                for &index in rows {
                                    state.remeasure_item(index, cx);
                                }
                            } else {
                                state.remeasure(cx);
                            }
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
                        if header::cadence_due(&shell.state.statuses) {
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
            terminal: None,
            view_refusal: None,
            focus,
            dock_focus: cx.focus_handle(),
            approval_scroll: ApprovalScroll::default(),
            question_scroll: QuestionScroll::default(),
            input_docked: false,
            dock_focused: false,
            rendered_stop: None,
            quit_focus: cx.focus_handle(),
            quit_sheet: false,
            window_active: window.is_window_active(),
            notifications_posted: 0,
            question_draft: None,
            #[cfg(test)]
            sent_responses: Default::default(),
            #[cfg(test)]
            sent_interrupts: Vec::new(),
            #[cfg(test)]
            sent_archives: Vec::new(),
            sidebar_width: cx.global::<Theme>().layout.sidebar_default,
            sidebar_scroll: Default::default(),
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
            permission_picker_open: false,
            thread_menu: None,
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
    fn set_thread_menu(
        &mut self,
        thread: Option<ThreadId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.thread_menu = thread
            .filter(|id| self.state.threads.get(id).is_some_and(|row| !row.archived))
            .map(|id| {
                let keep = sidebar::can_keep_children(&self.state, id);
                let focus = self.focus.clone();
                let menu = PopupMenu::build(window, cx, move |menu, _, _| {
                    let menu = menu
                        .action_context(focus)
                        .menu("Stop thread", Box::new(StopThread { thread: Some(id) }))
                        .menu("Archive", Box::new(Archive { thread: Some(id) }));
                    if keep {
                        menu.menu(
                            "Archive, keep child threads",
                            Box::new(ArchiveKeepChildren { thread: Some(id) }),
                        )
                    } else {
                        menu
                    }
                });
                cx.subscribe_in(&menu, window, |shell, _, _: &DismissEvent, window, cx| {
                    shell.thread_menu = None;
                    shell.input.update(cx, |input, cx| input.focus(window, cx));
                    cx.notify();
                })
                .detach();
                (id, menu)
            });
        cx.notify();
    }
    fn archive(
        &mut self,
        thread: Option<ThreadId>,
        keep_children: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(thread) = thread.or_else(|| self.view.chat()) {
            if keep_children && !sidebar::can_keep_children(&self.state, thread) {
                return;
            }
            self.thread_menu = None;
            self.send_command(
                Command::ArchiveThread {
                    thread,
                    keep_children,
                },
                window,
                cx,
            );
        }
    }
    pub(crate) fn notice(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        let message = message.into();
        tracing::info!(message = message.as_ref(), "window notice");
        self.notice = Some(message);
        cx.notify();
    }
    fn choose_async_option(
        &mut self,
        action: &ChooseAsyncOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(thread) = self.view.chat() else {
            return;
        };
        let Some(items) = self.state.items.get(&thread) else {
            return;
        };
        let latest = self
            .state
            .turns
            .values()
            .filter(|turn| turn.thread == thread)
            .max_by_key(|turn| turn.ordinal)
            .map(|turn| turn.id);
        let item = match action.item {
            Some(id) => items.get(&id),
            None => items
                .values()
                .filter(|item| {
                    Some(item.turn) == latest && async_question_active(item, items.values())
                })
                .max_by_key(|item| item.seq),
        };
        let Some(item) = item.filter(|item| async_question_active(item, items.values())) else {
            return;
        };
        let ItemBody::AsyncQuestions { questions, .. } = &item.body else {
            return;
        };
        let Some(question) = action
            .question
            .or_else(|| questions.iter().position(|q| q.selected.is_none()))
        else {
            return;
        };
        let option = action.option.unwrap_or(0);
        if questions
            .get(question)
            .is_none_or(|q| q.selected.is_some() || option >= q.options.len())
        {
            return;
        }
        self.send_command(
            Command::ChooseAsyncOption {
                item: item.id,
                question,
                option,
            },
            window,
            cx,
        );
    }
    fn send_command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(test)]
        if let Command::ArchiveThread {
            thread,
            keep_children,
        } = &command
        {
            self.sent_archives.push((*thread, *keep_children));
        }
        #[cfg(test)]
        if let Command::Respond { request, decision } = &command {
            self.sent_responses
                .borrow_mut()
                .push((*request, decision.clone()));
        }
        #[cfg(test)]
        if let Command::Interrupt { thread, turn } = &command {
            self.sent_interrupts.push(StopTarget {
                thread: *thread,
                turn: *turn,
            });
        }
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
                    shell.leave_archived_thread(window, cx);
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
        #[cfg(debug_assertions)]
        if self.view.chat() != view.chat() {
            self.state.visible_perf = VisiblePerf {
                thread: view.chat(),
                ..VisiblePerf::default()
            };
        }
        if matches!(view, MainView::Settings { .. }) {
            self.branch.clear();
        } else {
            self.forget_branch();
        }
        self.view_refusal = None;
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
    #[cfg(debug_assertions)]
    pub fn terminal_has_focus(&self, window: &Window, cx: &App) -> bool {
        self.terminal
            .as_ref()
            .is_some_and(|(_, view)| view.focus_handle(cx).is_focused(window))
    }
    fn terminal_selected(&self) -> Option<ThreadId> {
        self.view.chat().filter(|id| {
            self.state
                .threads
                .get(id)
                .is_some_and(|row| row.view == View::Terminal)
        })
    }
    fn sync_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.terminal_selected() else {
            if self.terminal.take().is_some() && self.view.chat().is_some() {
                self.input.update(cx, |input, cx| input.focus(window, cx));
            }
            return;
        };
        let handle = self.engine.terminal(thread);
        let current = self.terminal.as_ref().is_some_and(|(id, view)| {
            *id == thread
                && match (&view.read(cx).handle, &handle) {
                    (Some(old), Some(new)) => old.same(new),
                    (None, None) => true,
                    _ => false,
                }
        });
        if !current {
            let view = cx.new(|cx| terminal_view::TerminalView::new(handle, window, cx));
            window.focus(&view.focus_handle(cx), cx);
            self.terminal = Some((thread, view));
        }
    }
    fn switch_view(&mut self, action: &SwitchView, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread) = self.view.chat() else {
            return;
        };
        let to = if action
            .terminal
            .unwrap_or(self.terminal_selected().is_none())
        {
            View::Terminal
        } else {
            View::Chat
        };
        self.engine
            .set_terminal_colors(cx.global::<Theme>().terminal.engine_colors());
        let reply = self.engine.send(Command::SwitchView { thread, to });
        cx.spawn_in(window, async move |this, cx| {
            let result = reply.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                if shell.view.chat() == Some(thread) {
                    shell.view_refusal = match result {
                        Ok(Ok(_)) => None,
                        Ok(Err(error)) => Some(error.to_string()),
                        Err(_) => Some("The engine closed before answering".into()),
                    };
                    shell.sync_terminal(window, cx);
                    cx.notify();
                }
            });
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
                    let count = shell.state.transcript_rows(thread);
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
        if self.engine.is_closing() {
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
                        if shell.engine.is_closing() || shell.probe_generation != generation {
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
        let text = self.input.read(cx).value().to_string();
        if let Some(target) = self.shown_stop(cx) {
            window.dispatch_action(
                Box::new(Stop {
                    target: Some(target),
                }),
                cx,
            );
            return;
        }
        if let Some(reason) = self.disabled_reason() {
            self.notice(reason, cx);
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
    fn busy_threads(&self) -> Vec<String> {
        let work = self.engine.window_threads();
        self.state
            .threads
            .values()
            .filter(|thread| {
                thread.view == View::Terminal
                    || work.get(&thread.id).is_some_and(|state| state.busy)
            })
            .map(|thread| thread.title.clone())
            .collect()
    }
    fn stop_target(&self) -> Option<StopTarget> {
        let thread = self.view.chat()?;
        self.thread_stop_target(thread)
    }
    fn thread_stop_target(&self, thread: ThreadId) -> Option<StopTarget> {
        stop_target(
            thread,
            &self.state.turns,
            self.engine.window_threads().get(&thread),
        )
    }
    fn family_stop_visible(&self, child: &Thread) -> bool {
        !child.archived && self.thread_stop_target(child.id).is_some()
    }
    fn shown_stop(&self, cx: &App) -> Option<StopTarget> {
        self.rendered_stop
            .filter(|_| self.input.read(cx).value().trim().is_empty())
    }
    fn stop(&mut self, target: Option<StopTarget>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(StopTarget { thread, turn }) = target {
            self.send_command(Command::Interrupt { thread, turn }, window, cx);
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
    fn dock_target(&self) -> Option<DockTarget> {
        let thread = self.view.chat()?;
        let request = self.state.pending_requests(thread).first()?.id;
        let question = self
            .question_draft
            .as_ref()
            .filter(|(id, _)| *id == request)
            .map_or(0, |(_, answers)| answers.len());
        Some(DockTarget { request, question })
    }
    fn pending_request(&self, request: Option<RequestId>) -> Option<&Request> {
        let thread = self.view.chat()?;
        self.state.requests.iter().find(|row| {
            Some(row.id) == request && row.thread == thread && row.state == RequestState::Pending
        })
    }
    fn respond(
        &mut self,
        request: Option<RequestId>,
        decision: Decision,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.pending_request(request) else {
            self.notice("This request is no longer pending", cx);
            return;
        };
        let consent = composer::request_consent(&row.kind);
        let accepted = match decision {
            Decision::AllowOnce => consent.allow_once,
            Decision::AllowInThread => consent.allow_in_thread,
            Decision::Deny | Decision::Answers(_) => row.kind.accepts(&decision).is_some(),
        };
        if !accepted {
            return;
        }
        let request = row.id;
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
    fn answer_question(
        &mut self,
        request: Option<RequestId>,
        question: Option<usize>,
        option: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Request {
            id,
            kind: RequestKind::Question { questions },
            ..
        }) = self.pending_request(request).cloned()
        else {
            self.notice("This request is no longer pending", cx);
            return;
        };
        let index = self
            .question_draft
            .as_ref()
            .filter(|(request, _)| *request == id)
            .map_or(0, |(_, answers)| answers.len());
        if question != Some(index) {
            self.notice("This question is no longer current", cx);
            return;
        }
        let Some(text) = questions
            .get(index)
            .and_then(|question| question.options.get(option))
            .cloned()
        else {
            return;
        };
        if self
            .question_draft
            .as_ref()
            .is_none_or(|(request, _)| *request != id)
        {
            self.question_draft = Some((id, vec![]));
        }
        let (_, answers) = self.question_draft.as_mut().unwrap();
        answers.push(Answer { text });
        if answers.len() == questions.len() {
            let (_, answers) = self.question_draft.take().unwrap();
            self.respond(Some(id), Decision::Answers(answers), window, cx);
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
                    .when(
                        self.state
                            .threads
                            .values()
                            .any(|thread| thread.view == View::Terminal),
                        |sheet| sheet.child("Quitting ends any open CLI session."),
                    )
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
    fn begin_quitting(&mut self) {
        self.engine.shutdown();
        self.probes = Default::default();
        self.forget_branch();
    }
    fn confirm_quit(&mut self, cx: &mut Context<Self>) {
        if self.engine.is_closing() {
            return;
        }
        self.begin_quitting();
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
    fn default_run_mode(&self) -> PermissionMode {
        self.provider.run_mode(self.state.settings.default_mode)
    }
    fn new_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show(MainView::NewThread);
        self.provider_picker = None;
        self.config.mode = self.default_run_mode();
        self.notice = None;
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }
    fn leave_archived_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.view.chat().is_some_and(|id| {
            self.state
                .threads
                .get(&id)
                .is_some_and(|thread| thread.archived)
        }) {
            self.new_thread(window, cx);
        }
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
        let provider = self.chosen_provider();
        let mode = action.mode.unwrap_or_else(|| {
            if matches!(self.view, MainView::Settings { .. }) {
                return settings::next_mode(self.state.settings.default_mode);
            }
            provider.next_mode(self.composer_config().mode)
        });
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
                .projects
                .get(&project.id)
                .into_iter()
                .flat_map(|_| sidebar::ordered_threads(&self.state))
                .map(|id| &self.state.threads[&id])
                .filter(|thread| thread.project == project.id)
            {
                rows.push(serde_json::json!({
                    "kind":"thread", "id":thread.id, "title":thread.title,
                    "status":self.state.status(thread.id).words(),
                    "archived":thread.archived, "mode":thread.config.mode, "lineage":thread.lineage,
                    "row_status":sidebar::row_status(&self.state,thread).words(),
                    "depth":usize::from(sidebar::nested(&self.state,thread)),
                    "quiet_minutes":self.state.quiet_minutes(thread.id),
                    "archive_keep_children":sidebar::can_keep_children(&self.state,thread.id)
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
                    "id":thread.id, "title":thread.title, "lineage":thread.lineage,
                    "native_id":thread.native.as_ref().map(|native| &native.0),
                    "cli_version":thread.cli_version, "config":thread.config,
                    "status":status.words(), "header_status":self.state.header_status(thread.id),
                    "quiet_minutes":self.state.quiet_minutes(thread.id),
                    "breadcrumb":self.state.parent(thread.id).map(|parent| serde_json::json!({"thread":parent.id,"title":parent.title,"linked":!parent.archived})),
                    "parent_attention":header::parent_attention(&self.state,thread.id),
                    "failure":self.state.failure(thread.id), "hold":thread.hold,
                    "native_cursor":thread.native_cursor, "import_pending":thread.import_pending, "view":thread.view,
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
                let first = index == 0 || items[index - 1].turn != item.turn;
                row["turn_label"] = self
                    .state
                    .turns
                    .get(&item.turn)
                    .filter(|_| first)
                    .and_then(|turn| transcript::turn_label(&self.state, turn))
                    .into();
                row["report_delivered"] = transcript::report_delivered(&self.state, item).into();
                if let ItemBody::TaskCard(task) = item.body {
                    row["kind"] = "task_card".into();
                    row["text"] = transcript::task_card_text(&self.state, task).into();
                }
                row["outcome"] =
                    transcript::row_outcome(item, items.get(index + 1), &self.state).into();
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
                if matches!(item.body, ItemBody::AsyncQuestions { .. }) {
                    row["choices_active"] = async_question_active(
                        item,
                        self.state
                            .items
                            .get(&item.thread)
                            .into_iter()
                            .flat_map(|items| items.values()),
                    )
                    .into();
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
            "provider":self.chosen_provider(), "mode":self.chosen_provider().run_mode(self.composer_config().mode),
            "permission_picker_open":self.permission_picker_open,
            "offered_modes":self.chosen_provider().offered_modes(),
            "model":self.composer_config().model, "effort":self.composer_config().effort,
            "send_enabled":reason.is_none(), "disabled_reason":reason,
            "provider_picker_open":self.provider_picker.is_some(),
            "provider_picker_labels":self.provider_picker.as_ref().map(|_| &self.provider_picker_labels),
            "stop_visible":self.shown_stop(cx).is_some(),
            "discard_queued_visible":self.discard_queued_visible(),
            "dock":self.dock_target().and_then(|target| self.pending_request(Some(target.request)).map(|request| {
                let approval = match &request.kind {
                    RequestKind::Approval { action } => Some(action),
                    RequestKind::Question { .. } => None,
                };
                let consent = composer::request_consent(&request.kind);
                let text = approval.map(|action| composer::approval_text(action, self.chosen_provider()));
                let mut dock = serde_json::json!({"request":request, "position":1,
                    "action_text":text.as_ref().map(|(label, _, _)| label),
                    "consequence":text.as_ref().map(|(_, consequence, _)| consequence),
                    "preview":text.as_ref().map(|(_, _, preview)| preview),
                    "grant_caption":consent.caption,
                    "allow_once_enabled":consent.allow_once,
                    "allow_in_thread_enabled":consent.allow_in_thread,
                    "consent_reason":consent.reason,
                    "provider_text":approval.map(|action| composer::approval_quotes(action, self.chosen_provider().label())).unwrap_or_default(),
                    "count":self.state.pending_requests(request.thread).len(), "focused":self.dock_focused,
                    "question_index":target.question});
                if self.approval_scroll.request == Some(request.id) {
                    dock["details_scroll"] = serde_json::json!({"offset_y":f32::from(self.approval_scroll.handle.offset().y), "max_offset_y":f32::from(self.approval_scroll.handle.max_offset().y)});
                }
                dock
            }))
        })
    }
    #[cfg(debug_assertions)]
    pub fn dump(&self, data_dir: &std::path::Path, cx: &App) -> serde_json::Value {
        let perf = self.engine.perf();
        let mut transcript = self.scroller.read(cx).dump();
        transcript["items"] = serde_json::json!(self.dump_transcript());
        transcript["family_card"] = self.view.chat().filter(|thread|self.state.family_offset(*thread)>0)
            .map(|thread|serde_json::json!({"title":transcript::family_title(transcript::family_children(&self.state,thread).len()),
                "rows":transcript::family_children(&self.state,thread).into_iter().map(|child|serde_json::json!({
                    "thread":child.id,"title":child.title,"glyph":sidebar::glyph(&self.state.status(child.id)),
                    "status":self.state.status(child.id).words(),"open_action":"veyra::SelectThread",
                    "stop_action":self.family_stop_visible(child).then_some("veyra::StopThread")
                })).collect::<Vec<_>>()})).into();
        serde_json::json!({
            "pid":std::process::id(), "build_rev":env!("VEYRA_BUILD_REV"), "data_dir":data_dir,
            "appearance":cx.global::<Theme>().appearance.name(), "view":self.view_name(),
            "header_branch":self.branch, "view_refusal":self.view_refusal,
            "terminal":self.terminal.as_ref().map(|(_,view)|view.read(cx).dump()),
            "sidebar":{"width":f32::from(self.sidebar_width), "rows":self.dump_rows(), "menu_thread":self.thread_menu.as_ref().map(|(id,_)|id)},
            "selected_thread":self.dump_selected(), "transcript":transcript,
            "composer_text":self.input.read(cx).value().to_string(), "composer":self.dump_composer(cx),
            "providers":self.dump_providers(), "settings":{"default_mode":self.state.settings.default_mode},
            "awake":self.engine.awake(), "processes":self.engine.processes(),
            "perf":{"applied_batches":self.state.applied_batches, "max_batch_gap_ms":self.state.gap.maximum_ms,
                "visible":{"thread":self.state.visible_perf.thread,"applied_batches":self.state.visible_perf.applied_batches,
                    "max_batch_gap_ms":self.state.visible_perf.gap.maximum_ms},
                "transcript_open_ms":self.state.transcript_open.as_ref().and_then(|open|open.milliseconds),
                "mailbox_free":perf.mailbox_free,
                "commits":perf.commits, "actor_max_ms":perf.actor_max_ms, "actor_busy_ms":perf.actor_busy_ms,
                "repaints":self.repaints, "max_batch_events":self.state.max_batch_events,
                "flood":self.terminal.as_ref().and_then(|(_,view)|view.read(cx).flood())},
            "window_active":self.window_active, "notifications":{"posted":self.notifications_posted},
            "quit_sheet":{"open":self.quit_sheet, "threads":self.busy_threads()},
            "requests":self.state.requests,
            "tasks":self.state.tasks.values().collect::<Vec<_>>(),
            "notice":self.notice.as_ref().map(|notice| notice.as_ref())
        })
    }
    #[cfg(debug_assertions)]
    fn view_name(&self) -> &'static str {
        match self.view {
            MainView::Settings { .. } => "settings",
            MainView::NewThread => "new_thread",
            MainView::Chat(_) if self.terminal_selected().is_some() => "terminal",
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_terminal(window, cx);
        #[cfg(debug_assertions)]
        {
            self.repaints += 1;
        }
        let theme = cx.global::<Theme>().clone();
        let dock_target = self.dock_target();
        if self.question_scroll.target != dock_target {
            self.question_scroll = QuestionScroll {
                target: dock_target,
                handle: ScrollHandle::new(),
                options: ScrollHandle::new(),
            };
        }
        let dock_request = dock_target.map(|target| target.request);
        if self.input_docked != dock_target.is_some() {
            self.input_docked = dock_target.is_some();
            self.input.update(cx, |input, cx| {
                if self.input_docked {
                    input.set_auto_grow(1, 1, cx);
                } else {
                    input.set_auto_grow(2, 10, cx);
                }
            });
        }
        if self.approval_scroll.request != dock_request {
            self.approval_scroll = ApprovalScroll {
                request: dock_request,
                handle: ScrollHandle::new(),
                folder_handle: ScrollHandle::new(),
            };
        }
        let dock_question = dock_target.map(|target| target.question);
        self.rendered_stop = self.stop_target();
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
            .on_action(cx.listener(|shell, action: &SwitchView, window, cx| {
                shell.switch_view(action, window, cx)
            }))
            .on_action(
                cx.listener(|shell, action: &ChooseAsyncOption, window, cx| {
                    shell.choose_async_option(action, window, cx)
                }),
            )
            .on_action(cx.listener(|shell, action: &Stop, window, cx| {
                shell.stop(action.target.or(shell.rendered_stop), window, cx)
            }))
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
            .on_action(cx.listener(|shell, action: &OpenThreadMenu, window, cx| {
                shell.set_thread_menu(action.thread.or_else(|| shell.view.chat()), window, cx);
            }))
            .on_action(cx.listener(|shell, action: &Archive, window, cx| {
                shell.archive(action.thread, false, window, cx);
            }))
            .on_action(
                cx.listener(|shell, action: &ArchiveKeepChildren, window, cx| {
                    shell.archive(action.thread, true, window, cx);
                }),
            )
            .on_action(cx.listener(|shell, _: &NextNeedsYou, window, cx| {
                if let Some(thread) = sidebar::next_needs_you(
                    &sidebar::ordered_threads(&shell.state),
                    shell.view.selected(),
                    &shell.state.statuses,
                ) {
                    shell.select_thread(thread, window, cx);
                }
            }))
            .on_action(cx.listener(move |shell, action: &AllowOnce, window, cx| {
                shell.respond(
                    action.request.or(dock_request),
                    Decision::AllowOnce,
                    window,
                    cx,
                )
            }))
            .on_action(
                cx.listener(move |shell, action: &AllowInThread, window, cx| {
                    shell.respond(
                        action.request.or(dock_request),
                        Decision::AllowInThread,
                        window,
                        cx,
                    )
                }),
            )
            .on_action(cx.listener(move |shell, action: &Deny, window, cx| {
                shell.respond(action.request.or(dock_request), Decision::Deny, window, cx)
            }))
            .on_action(
                cx.listener(move |shell, action: &AnswerQuestion, window, cx| {
                    let (request, question) = if action.request.is_some() {
                        (action.request, action.question)
                    } else {
                        (dock_request, dock_question)
                    };
                    shell.answer_question(
                        request,
                        question,
                        action.option.unwrap_or(0),
                        window,
                        cx,
                    );
                }),
            )
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
                    *cycle(&ProviderKind::ALL, Some(&shell.provider))
                        .unwrap_or(&ProviderKind::ALL[0])
                });
                shell.provider_picker = None;
                shell.input.update(cx, |input, cx| input.focus(window, cx));
                shell.config.model = None;
                shell.config.effort = None;
                shell.config.mode = shell.default_run_mode();
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
                let grok_efforts = (shell.chosen_provider() == ProviderKind::Grok).then(|| {
                    shell
                        .probe(ProviderKind::Grok)
                        .and_then(|probe| {
                            probe
                                .models
                                .iter()
                                .find(|entry| Some(&entry.value) == model.as_ref())
                        })
                        .map(|entry| entry.effort_levels.clone())
                        .unwrap_or_default()
                });
                shell.update_config(window, cx, |config| {
                    config.model = model;
                    if grok_efforts.is_some_and(|levels| {
                        config
                            .effort
                            .as_ref()
                            .is_some_and(|effort| !levels.contains(effort))
                    }) {
                        config.effort = None;
                    }
                });
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
                        item.and_then(|item| shell.state.transcript_position(thread, item))
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
                if shell.engine.is_closing() {
                    return;
                }
                shell.quit_sheet = false;
                if let Some((_, terminal)) = &shell.terminal {
                    window.focus(&terminal.focus_handle(cx), cx);
                } else {
                    shell.input.update(cx, |input, cx| input.focus(window, cx));
                }
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
                        MainView::Chat(_) if self.terminal.is_some() => {
                            self.terminal.as_ref().unwrap().1.clone().into_any_element()
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
        let root = root.on_action(cx.listener(|shell, action: &DebugScrollApproval, _, cx| {
            if action.pixels.is_finite() {
                let handle = &shell.approval_scroll.handle;
                let offset = handle.offset();
                handle.set_offset(gpui::point(
                    offset.x,
                    offset.y + gpui::Pixels::from(action.pixels),
                ));
                cx.notify();
            }
        }));
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
            #[cfg(debug_assertions)]
            terminal_view::init(cx);
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
    use crate::tests::Scratch;
    use gpui::{
        AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
        Styled, Window, div,
    };
    use std::collections::{BTreeMap, BTreeSet};
    use veyra_engine::{
        domain::*,
        events::{ChangeEvent, ThreadSnapshot, UiEvent},
        provider::{Probe, ProviderKind, Readiness},
    };

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn failed_banner_renders_own_cause_and_one_sign_in_instruction(cx: &mut gpui::TestAppContext) {
        for (message, instruction) in [
            (
                "Your organization cannot use this model. Choose another model and send again.",
                false,
            ),
            (
                "Claude took this message but never started a turn for it. Send it again.",
                false,
            ),
            (
                "Claude is signed out. Sign in from a terminal with `claude`, then send again.",
                true,
            ),
            (
                "Codex is signed out. Sign in from a terminal with `codex login`, then send again.",
                true,
            ),
        ] {
            let text = std::sync::Arc::new(crate::tests::RenderedText::default());
            let mut test = gpui::TestAppContext::build_with_text_system(
                cx.dispatcher.clone(),
                None,
                text.clone(),
            );
            with_docked_shell(&mut test, |shell, cx, _, _| {
                shell.update(cx, |shell, _| {
                    let (_, turn) =
                        configure_dock_thread(shell, ProviderKind::Claude, false, false);
                    shell.state.requests.clear();
                    shell.state.turns.get_mut(&turn).unwrap().state = TurnState::Ended {
                        at: 1,
                        started: Some(0),
                        outcome: TurnOutcome::Failed {
                            message: message.into(),
                        },
                    };
                });
                draw(cx);
                let lines = text.lines();
                assert!(lines.contains(message), "banner lost its cause: {lines:?}");
                assert!(
                    lines.contains("The thread failed"),
                    "banner was not rendered"
                );
                let instructions = lines
                    .iter()
                    .filter(|line| line.to_ascii_lowercase().contains("sign in"))
                    .map(String::as_str)
                    .collect::<Vec<_>>();
                assert_eq!(
                    instructions,
                    if instruction { vec![message] } else { vec![] },
                    "unexpected sign-in text: {lines:?}"
                );
                assert!(cx.debug_bounds("caption-Retry").is_some());
            });
        }
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn family_card_renders_user_status_words(cx: &mut gpui::TestAppContext) {
        for (status, expected, raw) in [
            (Status::NeedsYou, "Needs you", "NeedsYou"),
            (Status::InTerminal, "In terminal", "InTerminal"),
        ] {
            let text = std::sync::Arc::new(crate::tests::RenderedText::default());
            let mut test = gpui::TestAppContext::build_with_text_system(
                cx.dispatcher.clone(),
                None,
                text.clone(),
            );
            with_docked_shell(&mut test, |shell, cx, _, _| {
                shell.update(cx, |shell, cx| {
                    let (parent, child, _) = add_family(shell, cx);
                    shell.state.statuses.insert(child, status);
                    shell.state.requests.clear();
                    shell.show(super::MainView::Chat(parent));
                });
                draw(cx);
                let lines = text.lines();
                assert!(cx.debug_bounds("family-card").is_some());
                assert!(
                    lines.contains(expected) && !lines.contains(raw),
                    "card words: {lines:?}"
                );
            });
        }
    }

    #[gpui::test]
    fn sidebar_tooltip_uses_shared_status_display(_: &mut gpui::TestAppContext) {
        assert!(
            include_str!("sidebar.rs").contains(".tooltip(super::header::status_text(&status))")
        );
        for (status, expected) in [
            (Status::NeedsYou, "Needs you"),
            (Status::InTerminal, "In terminal"),
        ] {
            assert_eq!(super::header::status_text(&status), expected);
        }
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn sidebar_overflow_keeps_row_height_and_reveals_selection(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let (first, last, row_height) = shell.update(cx, |shell, cx| {
                let (first, _) = configure_dock_thread(shell, ProviderKind::Claude, false, false);
                shell.state.requests.clear();
                let mut last = first;
                let template = shell.state.threads[&first].clone();
                for index in 0..80 {
                    let mut row = template.clone();
                    row.id = ThreadId::new();
                    row.title = format!("Overflow {index}");
                    last = row.id;
                    shell.state.threads.insert(row.id, row);
                }
                cx.notify();
                (first, last, cx.global::<super::Theme>().layout.thread_row)
            });
            draw(cx);
            let row = cx.debug_bounds("sidebar-thread-row").unwrap();
            assert_eq!(row.size.height, row_height);
            for thread in [last, first] {
                shell.update_in(cx, |shell, window, cx| {
                    shell.select_thread(thread, window, cx)
                });
                draw(cx);
                let title = if thread == last {
                    "caption-Overflow 79"
                } else {
                    "caption-T"
                };
                let bounds = cx.debug_bounds(title).unwrap();
                let viewport = cx.debug_bounds("sidebar-rows").unwrap();
                assert!(
                    bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom(),
                    "selected row is outside sidebar: {bounds:?}, {viewport:?}"
                );
            }
        });
    }

    #[test]
    fn terminal_header_uses_display_words() {
        assert_eq!(
            super::header::status_text(&Status::InTerminal),
            "In terminal"
        );
        assert_eq!(Status::InTerminal.words(), "InTerminal");
    }

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
                let switched = self.dispatched.clone();
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
                    .capture_action(move |action: &super::SwitchView, _, _| {
                        switched.borrow_mut().push(match action.terminal {
                            Some(true) => "Terminal",
                            Some(false) => "Chat",
                            None => "SwitchView",
                        });
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
        let engine = veyra_engine::Engine::start_with_env(
            runtime.handle(),
            veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
            veyra_engine::provider::env::ChildEnv::isolated(),
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

    #[test]
    fn stop_target_uses_running_or_pending_turn() {
        let thread = ThreadId::new();
        let running = TurnId::new();
        let pending = TurnId::new();
        let other_thread = ThreadId::new();
        let other_turn = TurnId::new();
        let mut turns = BTreeMap::from([
            (
                running,
                Turn {
                    id: running,
                    thread,
                    ordinal: 1,
                    origin: TurnOrigin::Provider,
                    state: TurnState::Running { since: 0 },
                },
            ),
            (
                other_turn,
                Turn {
                    id: other_turn,
                    thread: other_thread,
                    ordinal: 1,
                    origin: TurnOrigin::Provider,
                    state: TurnState::Running { since: 0 },
                },
            ),
        ]);
        let mut published = veyra_engine::WindowThread {
            wait_reason: Some(WaitReason::TurnRunning),
            busy: true,
            pending_turn: Some(pending),
        };
        for (state, pending_turn, expected) in [
            (
                TurnState::Running { since: 0 },
                Some(pending),
                Some(running),
            ),
            (TurnState::Queued, Some(pending), Some(pending)),
            (TurnState::Queued, None, None),
        ] {
            turns.get_mut(&running).unwrap().state = state;
            published.pending_turn = pending_turn;
            let target = super::stop_target(thread, &turns, Some(&published));
            assert_eq!(
                target.map(|target| (target.thread, target.turn)),
                expected.map(|turn| (thread, turn))
            );
        }
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn shown_stop_follows_rendered_target_and_empty_draft(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, _| {
            let target = shell.read_with(cx, |shell, _| shell.rendered_stop.unwrap());
            shell.update_in(cx, |shell, window, cx| {
                shell.state.turns.clear();
                assert!(shell.stop_target().is_none());
                for (draft, expected) in [("", true), ("draft", false), (" \n", true)] {
                    shell
                        .input
                        .update(cx, |input, cx| input.set_value(draft, window, cx));
                    assert_eq!(shell.dump_composer(cx)["stop_visible"], expected);
                }
                shell.send(window, cx);
            });
            cx.run_until_parked();
            assert_eq!(dispatched.borrow().as_slice(), ["Stop"]);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell
                    .sent_interrupts
                    .last()
                    .map(|target| (target.thread, target.turn))),
                Some((target.thread, target.turn))
            );
        });
    }

    #[cfg(debug_assertions)]
    fn text_batch(thread: ThreadId) -> Vec<UiEvent> {
        vec![UiEvent::TextAppended {
            thread,
            item: ItemId::new(),
            text: "text".into(),
            counter: 1,
            #[cfg(debug_assertions)]
            delta_count: 1,
        }]
    }

    fn add_family(
        shell: &mut super::Shell,
        cx: &mut Context<super::Shell>,
    ) -> (ThreadId, ThreadId, TurnId) {
        let root = shell.view.chat().unwrap();
        let project = ProjectId::new();
        shell.state.projects.insert(
            project,
            Project {
                id: project,
                path: "/tmp".into(),
                name: "Project".into(),
            },
        );
        let parent = Thread {
            id: root,
            project,
            title: "Root title".into(),
            lineage: Lineage::Root,
            provider: ProviderKind::Claude,
            config: shell.config.clone(),
            cwd: "/tmp".into(),
            native: None,
            native_cursor: None,
            cli_version: None,
            hold: None,
            view: View::Chat,
            import_pending: false,
            archived: false,
        };
        shell.state.threads.insert(root, parent.clone());
        let child = ThreadId::new();
        shell.state.threads.insert(
            child,
            Thread {
                id: child,
                title: "File reporter".into(),
                lineage: Lineage::Child {
                    parent: root,
                    root,
                    relationship: Relationship::Subagent,
                },
                ..parent
            },
        );
        let turn = TurnId::new();
        shell.state.turns.insert(
            turn,
            Turn {
                id: turn,
                thread: child,
                ordinal: 1,
                origin: TurnOrigin::Provider,
                state: TurnState::Running { since: 1 },
            },
        );
        shell
            .state
            .statuses
            .insert(child, Status::Working { since: 1 });
        shell.state.requests.clear();
        shell.scroller.update(cx, |state, cx| {
            state.reset(shell.state.transcript_rows(root), cx)
        });
        cx.notify();
        (root, child, turn)
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn sidebar_titles_stay_left_and_quiet_controls_stay_on_one_line(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| super::theme::register_fonts(cx).unwrap());
        with_docked_shell(cx, |shell, cx, _, _| {
            let child = shell.update(cx, |shell, cx| {
                let child = add_family(shell, cx).1;
                shell.state.threads.get_mut(&child).unwrap().title = "File".into();
                shell
                    .state
                    .last_activity
                    .insert(child, veyra_engine::actor::now());
                child
            });
            draw(cx);
            let button = cx.debug_bounds("sidebar-thread-title").unwrap();
            let caption = cx.debug_bounds("caption-File").unwrap();
            let padding = cx.update(|_, cx| cx.global::<super::Theme>().space.0[3]);
            assert!(
                f32::from(caption.left() - button.left() - padding).abs() < 1.,
                "centered title: {caption:?} in {button:?}"
            );
            shell.update(cx, |shell, cx| {
                shell
                    .state
                    .last_activity
                    .insert(child, veyra_engine::actor::now() - 600_000);
                cx.notify();
            });
            draw(cx);
            let row = cx.debug_bounds("sidebar-thread-row").unwrap();
            let title = cx.debug_bounds("sidebar-thread-title").unwrap();
            let hint = cx.debug_bounds("sidebar-quiet").unwrap();
            let stop = cx.debug_bounds("sidebar-thread-stop").unwrap();
            let menu = cx.debug_bounds("sidebar-thread-menu").unwrap();
            let height = cx.update(|_, cx| cx.global::<super::Theme>().layout.thread_row);
            assert_eq!(row.size.height, height, "quiet row grew: {row:?}");
            for control in [title, hint, stop, menu] {
                assert!(
                    f32::from(control.center().y - title.center().y).abs() < 1.,
                    "control left row: {control:?}"
                );
                assert!(
                    control.right() <= row.right(),
                    "control left sidebar: {control:?}"
                );
            }
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn queued_task_report_badge_uses_its_report_source(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let (parent, child, first_turn) = add_family(shell, cx);
                let second_turn = TurnId::new();
                let user_turn = TurnId::new();
                let first = TaskId::new();
                let second = TaskId::new();
                let report = ItemId::new();
                for (task, turn) in [(first, first_turn), (second, second_turn)] {
                    shell.state.tasks.insert(
                        task,
                        veyra_engine::domain::Task {
                            id: task,
                            parent,
                            child,
                            turn,
                            request_key: task.to_string(),
                            state: TaskState::Settled {
                                outcome: TurnOutcome::Completed,
                                summary: "Reported follow-up".into(),
                                source: Some(report),
                                delivery: Delivery::Delivered {
                                    wake: TurnId::new(),
                                },
                            },
                        },
                    );
                }
                for (seq, turn, text) in [
                    (1, first_turn, "First report"),
                    (2, second_turn, "Reported follow-up"),
                    (3, user_turn, "Reported follow-up"),
                ] {
                    shell.state.turns.insert(
                        turn,
                        Turn {
                            id: turn,
                            thread: child,
                            ordinal: seq,
                            origin: if turn == user_turn {
                                TurnOrigin::User {
                                    item: ItemId::new(),
                                }
                            } else {
                                TurnOrigin::Task {
                                    task: if turn == first_turn { first } else { second },
                                }
                            },
                            state: TurnState::Ended {
                                started: Some(seq as i64),
                                at: seq as i64 + 1,
                                outcome: TurnOutcome::Completed,
                            },
                        },
                    );
                    shell.state.apply(UiEvent::Change {
                        counter: None,
                        event: ChangeEvent::ItemAdded(Item {
                            id: if turn == second_turn {
                                report
                            } else {
                                ItemId::new()
                            },
                            thread: child,
                            turn,
                            seq,
                            body: ItemBody::AssistantText {
                                text: text.into(),
                                done: true,
                            },
                        }),
                    });
                }
                shell.show(super::MainView::Chat(child));
                let rows = shell.dump_transcript();
                assert_eq!(
                    rows.iter()
                        .map(|row| row["report_delivered"].as_bool().unwrap())
                        .collect::<Vec<_>>(),
                    [false, true, false]
                );
            });
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn empty_child_report_never_badges_later_work(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let (parent, child, first_turn) = add_family(shell, cx);
                let task = TaskId::new();
                shell.state.turns.get_mut(&first_turn).unwrap().state = TurnState::Ended {
                    started: Some(1),
                    at: 2,
                    outcome: TurnOutcome::Interrupted {
                        reason: "Stopped by you".into(),
                    },
                };
                shell.state.tasks.insert(
                    task,
                    veyra_engine::domain::Task {
                        id: task,
                        parent,
                        child,
                        turn: first_turn,
                        request_key: "empty report".into(),
                        state: TaskState::Settled {
                            outcome: TurnOutcome::Interrupted {
                                reason: "Stopped by you".into(),
                            },
                            summary: String::new(),
                            source: None,
                            delivery: Delivery::Delivered {
                                wake: TurnId::new(),
                            },
                        },
                    },
                );
                let later_turn = TurnId::new();
                let mut later = Item {
                    id: ItemId::new(),
                    thread: child,
                    turn: later_turn,
                    seq: 1,
                    body: ItemBody::AssistantText {
                        text: String::new(),
                        done: false,
                    },
                };
                shell.state.turns.insert(
                    later_turn,
                    Turn {
                        id: later_turn,
                        thread: child,
                        ordinal: 2,
                        origin: TurnOrigin::User {
                            item: ItemId::new(),
                        },
                        state: TurnState::Running { since: 3 },
                    },
                );
                shell.show(super::MainView::Chat(child));
                for (text, done) in [("", false), ("", true), ("Later unrelated work", true)] {
                    later.body = ItemBody::AssistantText {
                        text: text.into(),
                        done,
                    };
                    shell.state.apply(UiEvent::Change {
                        counter: None,
                        event: ChangeEvent::ItemCompleted(later.clone()),
                    });
                    assert!(!super::transcript::report_delivered(&shell.state, &later));
                    let rows = shell.dump_transcript();
                    assert_eq!(rows.len(), 1);
                    assert_eq!(rows[0]["report_delivered"], false);
                }
            });
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn quiet_row_retains_title_and_untruncated_hint(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| super::theme::register_fonts(cx).unwrap());
        with_docked_shell(cx, |shell, cx, _, _| {
            let (_, child, _) = shell.update(cx, |shell, cx| {
                let family = add_family(shell, cx);
                shell
                    .state
                    .last_activity
                    .insert(family.1, veyra_engine::actor::now() - 600_000);
                shell.sidebar_width = cx.global::<super::Theme>().layout.sidebar_default;
                family
            });
            draw(cx);
            let layout = cx.update(|_, cx| cx.global::<super::Theme>().layout.clone());
            for width in [layout.sidebar_default, layout.sidebar_min] {
                shell.update(cx, |shell, cx| {
                    shell.sidebar_width = width;
                    cx.notify();
                });
                draw(cx);
                let title = cx.debug_bounds("sidebar-thread-title").unwrap();
                let hint = cx.debug_bounds("sidebar-quiet").unwrap();
                assert!(
                    title.size.width >= layout.small_icon_button,
                    "title vanished at sidebar width {width}: {title:?}; hint {hint:?}"
                );
                assert!(
                    hint.size.width >= layout.status_glyph * 6.,
                    "quiet hint shrank: {hint:?}"
                );
                assert!(hint.right() <= width, "hint left sidebar: {hint:?}");
            }
            shell.update(cx, |shell, cx| {
                shell.show(super::MainView::Chat(child));
                cx.notify();
            });
            draw(cx);
            assert!(cx.debug_bounds("header-title").unwrap().size.width > layout.small_icon_button);
            assert!(
                cx.debug_bounds("header-status").unwrap().size.width > layout.status_glyph * 10.
            );
            assert!(
                shell
                    .read_with(cx, |shell, _| shell.state.header_status(child))
                    .contains("Working")
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn family_card_stop_tracks_the_shared_turn_target(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let (_, child, turn) = shell.update(cx, add_family);
            draw(cx);
            assert!(cx.debug_bounds("family-child-stop").is_some());
            for outcome in [
                TurnOutcome::Completed,
                TurnOutcome::Interrupted {
                    reason: "Stopped by you".into(),
                },
            ] {
                shell.update(cx, |shell, cx| {
                    shell.state.turns.get_mut(&turn).unwrap().state = TurnState::Ended {
                        started: Some(1),
                        at: 2,
                        outcome,
                    };
                    cx.notify();
                });
                draw(cx);
                assert!(
                    cx.debug_bounds("family-child-stop").is_none(),
                    "ended child still shows Stop"
                );
                assert!(shell.read_with(cx, |shell, cx| {
                    shell.dump(std::path::Path::new("/tmp"), cx)
                        ["transcript"]["family_card"]["rows"][0]["stop_action"]
                        .is_null()
                }));
            }
            shell.update(cx, |shell, cx| {
                shell.state.turns.get_mut(&turn).unwrap().state = TurnState::Running { since: 3 };
                shell.state.threads.get_mut(&child).unwrap().archived = true;
                cx.notify();
            });
            draw(cx);
            assert!(cx.debug_bounds("family-child-stop").is_none());
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn family_card_uses_singular_and_plural_agent_labels(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let (_, child, _) = add_family(shell, cx);
                assert_eq!(shell.dump(std::path::Path::new("/tmp"), cx)["transcript"]["family_card"]["title"], "Started 1 agent");
                let mut second = shell.state.threads[&child].clone();
                second.id = ThreadId::new();
                shell.state.threads.insert(second.id, second);
                assert_eq!(shell.dump(std::path::Path::new("/tmp"), cx)["transcript"]["family_card"]["title"], "Started 2 agents");
            });
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn failed_admitted_prompt_keeps_its_cause_and_retry(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let (_, child, turn) = add_family(shell, cx);
                let item = ItemId::new();
                shell.state.turns.get_mut(&turn).unwrap().origin = TurnOrigin::User { item };
                shell.state.turns.get_mut(&turn).unwrap().state = TurnState::Ended {
                    started: Some(300),
                    at: 400,
                    outcome: TurnOutcome::Failed {
                        message: "admitted prompt failed".into(),
                    },
                };
                shell.state.apply(UiEvent::Change {
                    counter: None,
                    event: ChangeEvent::ItemAdded(Item {
                        id: item,
                        thread: child,
                        turn,
                        seq: 0,
                        body: ItemBody::UserText("Retry this prompt".into()),
                    }),
                });
                let earlier = Turn {
                    id: TurnId::new(),
                    thread: child,
                    ordinal: 2,
                    origin: TurnOrigin::Provider,
                    state: TurnState::Ended {
                        started: Some(100),
                        at: 200,
                        outcome: TurnOutcome::Completed,
                    },
                };
                shell.state.turns.insert(earlier.id, earlier);
                shell.show(super::MainView::Chat(child));
                assert_eq!(shell.state.failure(child), Some("admitted prompt failed"));
                assert_eq!(
                    shell.state.retry_prompt(child),
                    Some("Retry this prompt".into())
                );
                assert_eq!(shell.view_name(), "failed_thread");
            });
        });
    }

    #[gpui::test]
    fn archive_keep_children_key_acts_only_on_an_eligible_root(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let (root, child, _) = shell.update(cx, add_family);
            draw(cx);
            cx.simulate_keystrokes("secondary-alt-shift-a");
            cx.run_until_parked();
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.sent_archives.clone()),
                [(root, true)]
            );
            shell.update(cx, |shell, cx| {
                shell.show(super::MainView::Chat(child));
                cx.notify();
            });
            draw(cx);
            cx.simulate_keystrokes("secondary-alt-shift-a");
            cx.run_until_parked();
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.sent_archives.clone()),
                [(root, true)]
            );
            shell.update(cx, |shell, cx| {
                shell.state.threads.get_mut(&child).unwrap().archived = true;
                shell.show(super::MainView::Chat(root));
                cx.notify();
            });
            draw(cx);
            cx.simulate_keystrokes("secondary-alt-shift-a");
            cx.run_until_parked();
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.sent_archives.clone()),
                [(root, true)]
            );
        });
    }

    #[gpui::test]
    fn archived_selection_resets_draft_mode_picker_and_branch(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, events| {
            let root = shell.update_in(cx, |shell, window, cx| {
                let (root, _, _) = add_family(shell, cx);
                shell.state.settings.default_mode = PermissionMode::AskFirst;
                shell.config.mode = PermissionMode::FullAccess;
                shell.branch = "selected-branch".into();
                shell.notice = Some("selected notice".into());
                shell.input.update(cx, |input, cx| {
                    input.set_value("selected draft", window, cx)
                });
                shell.set_provider_picker(true, window, cx);
                root
            });
            events
                .send(UiEvent::Change {
                    counter: None,
                    event: ChangeEvent::ThreadArchived { thread: root },
                })
                .unwrap();
            cx.run_until_parked();
            shell.read_with(cx, |shell, cx| {
                assert!(matches!(shell.view, super::MainView::NewThread));
                assert_eq!(shell.config.mode, PermissionMode::AskFirst);
                assert_eq!(shell.input.read(cx).value().as_ref(), "");
                assert!(shell.provider_picker.is_none());
                assert!(shell.branch.is_empty());
                assert!(shell.notice.is_none());
            });
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn delivered_report_stays_on_its_task_after_later_user_work(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let (parent, child, turn) = add_family(shell, cx);
                let task = TaskId::new();
                let report_item = ItemId::new();
                shell.state.tasks.insert(
                    task,
                    veyra_engine::domain::Task {
                        id: task,
                        parent,
                        child,
                        turn,
                        request_key: "start_thread:report".into(),
                        state: TaskState::Settled {
                            outcome: TurnOutcome::Completed,
                            summary: "delivered report".into(),
                            source: Some(report_item),
                            delivery: Delivery::Delivered {
                                wake: TurnId::new(),
                            },
                        },
                    },
                );
                let report = Item {
                    id: report_item,
                    thread: child,
                    turn,
                    seq: 1,
                    body: ItemBody::AssistantText {
                        text: "delivered report".into(),
                        done: true,
                    },
                };
                let user_turn = TurnId::new();
                shell.state.turns.insert(
                    user_turn,
                    Turn {
                        id: user_turn,
                        thread: child,
                        ordinal: 2,
                        origin: TurnOrigin::User {
                            item: ItemId::new(),
                        },
                        state: TurnState::Running { since: 2 },
                    },
                );
                let later = Item {
                    id: ItemId::new(),
                    thread: child,
                    turn: user_turn,
                    seq: 2,
                    body: ItemBody::AssistantText {
                        text: "later user reply".into(),
                        done: true,
                    },
                };
                for item in [&report, &later] {
                    shell.state.apply(UiEvent::Change {
                        counter: None,
                        event: ChangeEvent::ItemAdded(item.clone()),
                    });
                }
                shell.show(super::MainView::Chat(child));
                let rows = shell.dump_transcript();
                assert_eq!(rows[0]["report_delivered"], true);
                assert_eq!(rows[1]["report_delivered"], false);
            });
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn visible_perf_counts_only_selected_text_and_resets(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let thread = shell.view.chat().unwrap();
                let other = ThreadId::new();
                shell.show(super::MainView::NewThread);
                shell.show(super::MainView::Chat(thread));
                shell.state.apply_all(text_batch(other));
                shell.state.apply_all(text_batch(thread));
                let dump = shell.dump(std::path::Path::new("/tmp"), cx);
                assert_eq!(dump["perf"]["visible"]["thread"], serde_json::json!(thread));
                assert_eq!(dump["perf"]["visible"]["applied_batches"], 1);
                let running = shell.state.turns.values().next().unwrap().clone();
                shell.state.apply_all(vec![UiEvent::Change {
                    counter: None,
                    event: ChangeEvent::TurnStarted(Turn { thread: other, ..running.clone() }),
                }]);
                assert_eq!(shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["visible"]["applied_batches"], 1);
                shell.state.apply_all(vec![UiEvent::Change {
                    counter: None,
                    event: ChangeEvent::TurnStarted(running),
                }]);
                assert_eq!(shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["visible"]["applied_batches"], 0);
                shell.state.apply_all(text_batch(thread));
                std::thread::sleep(std::time::Duration::from_millis(120));
                shell.state.apply_all(text_batch(thread));
                assert!(shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["visible"]["max_batch_gap_ms"].as_f64().unwrap() >= 100.);
                shell.show(super::MainView::Chat(other));
                let visible = &shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["visible"];
                assert_eq!(visible["thread"], serde_json::json!(other));
                assert_eq!(visible["applied_batches"], 0);
                assert_eq!(visible["max_batch_gap_ms"], 0.);
            });
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn visible_perf_sees_a_pause_despite_hidden_text(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let thread = shell.update(cx, |shell, _| {
                let thread = shell.view.chat().unwrap();
                shell.show(super::MainView::NewThread);
                shell.show(super::MainView::Chat(thread));
                shell.state.apply_all(text_batch(thread));
                thread
            });
            let hidden = ThreadId::new();
            for _ in 0..6 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                shell.update(cx, |shell, _| shell.state.apply_all(text_batch(hidden)));
            }
            shell.update(cx, |shell, cx| {
                shell.state.apply_all(text_batch(thread));
                let dump = shell.dump(std::path::Path::new("/tmp"), cx);
                assert_eq!(dump["perf"]["visible"]["applied_batches"], 2);
                assert!(dump["perf"]["visible"]["max_batch_gap_ms"].as_f64().unwrap() >= 100.);
                let running = shell.state.turns.values().next().unwrap().clone();
                shell.state.apply_all(vec![UiEvent::Change {
                    counter: None,
                    event: ChangeEvent::TurnStarted(running),
                }]);
                assert_eq!(shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["visible"]["max_batch_gap_ms"], 0.);
                shell.state.apply_all(text_batch(thread));
                assert_eq!(shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["visible"]["max_batch_gap_ms"], 0.);
            });
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn transcript_open_perf_stamps_first_installed_render_and_replaces(
        cx: &mut gpui::TestAppContext,
    ) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let empty = || ThreadSnapshot {
                items: vec![],
                buffered_text: BTreeMap::new(),
                counter: 1,
                #[cfg(debug_assertions)]
                delta_counts: BTreeMap::new(),
            };
            let (thread, request) = shell.update(cx, |shell, cx| {
                assert!(
                    shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["transcript_open_ms"]
                        .is_null()
                );
                let thread = shell.view.chat().unwrap();
                (thread, shell.state.begin_snapshot(thread))
            });
            std::thread::sleep(std::time::Duration::from_millis(120));
            draw(cx);
            shell.update(cx, |shell, cx| {
                assert!(
                    shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["transcript_open_ms"]
                        .is_null()
                );
                assert!(shell.state.finish_snapshot(thread, request, empty()));
                assert!(
                    shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["transcript_open_ms"]
                        .is_null()
                );
                cx.notify();
            });
            draw(cx);
            let first = shell.read_with(cx, |shell, cx| {
                shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["transcript_open_ms"]
                    .as_f64()
                    .unwrap()
            });
            assert!(first >= 100.);
            draw(cx);
            shell.update(cx, |shell, cx| {
                assert_eq!(
                    shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["transcript_open_ms"],
                    first
                );
                let old = shell.state.begin_snapshot(thread);
                let current = shell.state.begin_snapshot(thread);
                assert!(!shell.state.finish_snapshot(thread, old, empty()));
                assert!(shell.state.finish_snapshot(thread, current, empty()));
                assert!(
                    shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["transcript_open_ms"]
                        .is_null()
                );
                cx.notify();
            });
            draw(cx);
            let second = shell.read_with(cx, |shell, cx| {
                shell.dump(std::path::Path::new("/tmp"), cx)["perf"]["transcript_open_ms"]
                    .as_f64()
                    .unwrap()
            });
            assert_ne!(second, first);
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn empty_send_stops_while_provider_is_checking(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, _| {
            let target = shell.update(cx, |shell, cx| {
                shell.providers.clear();
                cx.notify();
                shell.stop_target().unwrap()
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.disabled_reason()),
                Some("Checking Claude Code…".into())
            );
            assert_eq!(
                shell.read_with(cx, |shell, cx| shell.dump_composer(cx)["stop_visible"]
                    .clone()),
                true
            );
            cx.simulate_keystrokes("secondary-enter");
            cx.run_until_parked();
            draw(cx);
            assert_eq!(dispatched.borrow().as_slice(), ["Stop"]);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell
                    .sent_interrupts
                    .iter()
                    .map(|target| (target.thread, target.turn))
                    .collect::<Vec<_>>()),
                [(target.thread, target.turn)]
            );
        });
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
            cx.dispatch_action(super::Stop::default());
            cx.run_until_parked();
            draw(cx);
            assert_eq!(dispatched.borrow().as_slice(), ["Stop", "Stop"]);
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn empty_send_without_a_turn_is_refused(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, _| {
            shell.update(cx, |shell, cx| {
                for turn in shell.state.turns.values_mut() {
                    turn.state = TurnState::Queued;
                }
                shell.state.requests.clear();
                cx.notify();
            });
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| shell.stop_target().is_none()));
            assert_eq!(
                shell.read_with(cx, |shell, cx| shell.dump_composer(cx)["stop_visible"]
                    .clone()),
                false
            );
            shell.update_in(cx, |shell, window, cx| {
                shell.send(window, cx);
            });
            cx.run_until_parked();
            assert!(dispatched.borrow().is_empty());
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.notice.clone()),
                Some("Write a message before sending".into())
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn next_approval_opens_at_top(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let approval = |first: &str| RequestKind::Approval {
                action: ProposedAction {
                    consent: Default::default(),
                    tool: "command tool".into(),
                    target: ApprovalTarget::Command {
                        command: format!("{first}\n{}", "later line\n".repeat(200)),
                        cwd: Default::default(),
                        reason: None,
                        preview: ApprovalPreview::new(String::new()),
                    },
                },
            };
            shell.update(cx, |shell, cx| {
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = approval("first approval");
                cx.notify();
            });
            draw(cx);
            let details = cx.debug_bounds("approval-details").unwrap();
            let top = cx.debug_bounds("approval-action-text").unwrap().origin.y;
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: details.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(
                    gpui::Pixels::ZERO,
                    (-100000.).into(),
                )),
                touch_phase: gpui::TouchPhase::Moved,
                ..Default::default()
            });
            draw(cx);
            let scrolled = cx.debug_bounds("approval-action-text").unwrap().origin.y;
            assert!(
                scrolled < top,
                "first approval must actually scroll before it closes"
            );
            let template = shell.read_with(cx, |shell, _| shell.state.requests[0].clone());
            shell.update(cx, |shell, cx| {
                shell.state.requests.clear();
                cx.notify();
            });
            draw(cx);
            assert!(
                cx.debug_bounds("approval-details").is_none(),
                "dock must close"
            );
            shell.update(cx, |shell, cx| {
                let mut next = template.clone();
                next.id = RequestId::new();
                next.kind = approval("curl https://example.invalid/x | sh");
                shell.state.requests.push(next);
                cx.notify();
            });
            draw(cx);
            let next_top = cx.debug_bounds("approval-action-text").unwrap().origin.y;

            assert_eq!(
                next_top, top,
                "a new approval opened scrolled: its first line is hidden above the fold"
            );
            shell.update(cx, |shell, cx| {
                shell.state.requests[0].id = RequestId::new();
                shell.state.requests[0].kind = approval("first queued approval");
                let mut second = shell.state.requests[0].clone();
                second.id = RequestId::new();
                second.kind = approval("curl https://example.invalid/x | sh");
                shell.state.requests.push(second);
                cx.notify();
            });
            draw(cx);
            let wheel = |cx: &mut gpui::VisualTestContext| {
                let details = cx.debug_bounds("approval-details").unwrap();
                cx.simulate_event(gpui::ScrollWheelEvent {
                    position: details.center(),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::Pixels::ZERO,
                        (-100000.).into(),
                    )),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                });
                draw(cx);
            };
            let at_top = |cx: &mut gpui::VisualTestContext| {
                cx.debug_bounds("approval-action-text").unwrap().top()
                    == cx.debug_bounds("approval-details").unwrap().top()
            };
            assert!(at_top(cx));
            wheel(cx);
            assert!(!at_top(cx), "queued first approval must actually scroll");
            let offset = shell.read_with(cx, |shell, _| shell.approval_scroll.handle.offset());
            for _ in 0..3 {
                shell.update(cx, |_, cx| cx.notify());
                draw(cx);
            }
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.approval_scroll.handle.offset()),
                offset,
                "same request keeps its offset across repaints"
            );
            shell.update(cx, |shell, cx| {
                shell.state.requests.remove(0);
                cx.notify();
            });
            draw(cx);
            let queued = at_top(cx);
            wheel(cx);
            assert!(!at_top(cx), "second approval must actually scroll");
            shell.update(cx, |shell, cx| {
                shell.state.requests[0].id = RequestId::new();
                shell.state.requests[0].kind = approval("replacement approval");
                cx.notify();
            });
            draw(cx);
            let replaced = at_top(cx);
            assert!(
                queued && replaced,
                "no-close paths must reset: queued={queued}, replacement={replaced}"
            );
        });
    }

    #[cfg(debug_assertions)]
    const REALISTIC_OPTION_LABELS: [&str; 4] = [
        "Use PostgreSQL (Recommended)",
        "Keep SQLite for now",
        "Add the migration later",
        "Ask me after the tests pass",
    ];
    #[cfg(debug_assertions)]
    const LONG_OPTION_LABEL: &str =
        "use PostgreSQL for the service and keep the existing migration history";

    #[cfg(debug_assertions)]
    const DOCK_OPTIONS: [(&str, &str); 9] = [
        ("dock-option-0", "dock-option-caption-0"),
        ("dock-option-1", "dock-option-caption-1"),
        ("dock-option-2", "dock-option-caption-2"),
        ("dock-option-3", "dock-option-caption-3"),
        ("dock-option-4", "dock-option-caption-4"),
        ("dock-option-5", "dock-option-caption-5"),
        ("dock-option-6", "dock-option-caption-6"),
        ("dock-option-7", "dock-option-caption-7"),
        ("dock-option-8", "dock-option-caption-8"),
    ];

    #[cfg(debug_assertions)]
    fn configure_dock_thread(
        shell: &mut super::Shell,
        provider: ProviderKind,
        hold: bool,
        queued: bool,
    ) -> (ThreadId, TurnId) {
        let thread = shell.view.chat().unwrap();
        shell
            .state
            .turns
            .retain(|_, turn| turn.thread != thread || turn.state != TurnState::Queued);
        let turn = shell
            .state
            .turns
            .values()
            .find(|turn| turn.thread == thread && matches!(turn.state, TurnState::Running { .. }))
            .unwrap()
            .id;
        let project = ProjectId::new();
        shell.state.projects.clear();
        shell.state.projects.insert(
            project,
            Project {
                id: project,
                path: "/tmp".into(),
                name: "Project".into(),
            },
        );
        shell.state.threads.insert(
            thread,
            Thread {
                id: thread,
                project,
                title: "T".into(),
                lineage: Lineage::Root,
                provider,
                config: shell.config.clone(),
                cwd: "/tmp".into(),
                native: None,
                native_cursor: None,
                cli_version: None,
                hold: hold.then_some(HoldReason::ChildReport),
                view: View::Chat,
                import_pending: false,
                archived: false,
            },
        );
        shell.providers = vec![Probe {
            provider,
            path: None,
            version: None,
            readiness: Readiness::Ready,
            models: vec![],
            account: None,
        }];
        if queued {
            let id = TurnId::new();
            shell.state.turns.insert(
                id,
                Turn {
                    id,
                    thread,
                    ordinal: 2,
                    origin: TurnOrigin::User {
                        item: ItemId::new(),
                    },
                    state: TurnState::Queued,
                },
            );
        }
        (thread, turn)
    }

    #[cfg(debug_assertions)]
    fn dock_layout_issues(
        shell: &Entity<super::Shell>,
        cx: &mut gpui::VisualTestContext,
    ) -> Vec<String> {
        let viewport = cx.update(|window, _| window.viewport_size());
        let button_height = cx.update(|_, cx| cx.global::<super::Theme>().layout.icon_button);
        let inside = |bounds: gpui::Bounds<gpui::Pixels>| {
            bounds.top() >= gpui::Pixels::ZERO
                && bounds.left() >= gpui::Pixels::ZERO
                && bounds.bottom() <= viewport.height
                && bounds.right() <= viewport.width
        };
        let contains = |outer: gpui::Bounds<gpui::Pixels>, inner: gpui::Bounds<gpui::Pixels>| {
            inner.top() >= outer.top()
                && inner.left() >= outer.left()
                && inner.bottom() <= outer.bottom()
                && inner.right() <= outer.right()
        };
        let overlaps = |a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>| {
            a.left() < b.right()
                && a.right() > b.left()
                && a.top() < b.bottom()
                && a.bottom() > b.top()
        };
        let mut issues = Vec::new();
        let mut regions: Vec<_> = [
            "composer-hold",
            "dock-count",
            "approval-consequence",
            "approval-consent-reason",
            "approval-working-folder",
            "approval-details",
            "dock-provider-question",
            "dock-options",
            "approval-decisions",
            "dock-focus-hint",
            "composer-input",
            "composer-toolbar",
        ]
        .into_iter()
        .filter_map(|name| cx.debug_bounds(name).map(|bounds| (name, bounds)))
        .collect();
        let options: Vec<_> = DOCK_OPTIONS
            .iter()
            .filter_map(|(name, _)| cx.debug_bounds(name).map(|bounds| (*name, bounds)))
            .collect();
        if cx.debug_bounds("dock-options").is_none() {
            regions.extend(options.iter().copied());
        }
        for (name, bounds) in &regions {
            if !inside(*bounds) {
                issues.push(format!("region {name} outside window: {bounds:?}"));
            }
        }
        for (index, (name, bounds)) in regions.iter().enumerate() {
            for (other, other_bounds) in &regions[index + 1..] {
                if overlaps(*bounds, *other_bounds) {
                    issues.push(format!(
                        "regions overlap: {name} {bounds:?} / {other} {other_bounds:?}"
                    ));
                }
            }
        }
        let mut controls: Vec<_> = [
            "composer-continue",
            "composer-discard",
            "dock-focus-hint",
            "approval-deny-button",
            "approval-grant-button",
            "approval-allow-button",
            "composer-provider",
            "composer-model",
            "composer-effort",
            "composer-permission",
            "composer-submit-button",
        ]
        .into_iter()
        .filter_map(|name| cx.debug_bounds(name).map(|bounds| (name, bounds)))
        .collect();
        let option_viewport = cx.debug_bounds("dock-options");
        for (name, bounds) in &options {
            if bounds.size.height < button_height || bounds.size.width <= gpui::Pixels::ZERO {
                issues.push(format!("control {name} collapsed: {bounds:?}"));
            }
            if let Some(content) = cx.debug_bounds("dock-options-content")
                && !contains(content, *bounds)
            {
                issues.push(format!(
                    "option escapes list: {name} {bounds:?} / {content:?}"
                ));
            }
        }

        controls.extend(options.into_iter().filter_map(|(name, bounds)| {
            let visible = option_viewport.map_or(bounds, |viewport| bounds.intersect(&viewport));
            (visible.size.height > gpui::Pixels::ZERO && visible.size.width > gpui::Pixels::ZERO)
                .then_some((name, visible))
        }));
        for (index, (name, bounds)) in controls.iter().enumerate() {
            if !DOCK_OPTIONS.iter().any(|(option, _)| option == name)
                && (bounds.size.height < button_height || bounds.size.width <= gpui::Pixels::ZERO)
            {
                issues.push(format!("control {name} collapsed: {bounds:?}"));
            }
            if !inside(*bounds) {
                issues.push(format!("control {name} outside window: {bounds:?}"));
            }
            for (other, other_bounds) in &controls[index + 1..] {
                if overlaps(*bounds, *other_bounds) {
                    issues.push(format!(
                        "controls overlap: {name} {bounds:?} / {other} {other_bounds:?}"
                    ));
                }
            }
        }
        if let Some(wrapper) = cx
            .debug_bounds("composer-input")
            .filter(|_| shell.read_with(cx, |shell, _| shell.dock_target().is_some()))
        {
            let editor = shell.read_with(cx, |shell, cx| shell.input.read(cx).input_bounds());
            if !contains(wrapper, editor) {
                issues.push(format!(
                    "input editor escapes its viewport: wrapper={wrapper:?}, editor={editor:?}"
                ));
            }
        }
        if let Some(quote) = cx.debug_bounds("dock-provider-question") {
            let label = cx.debug_bounds("dock-provider-question-label").unwrap();
            let value = cx.debug_bounds("dock-provider-question-value").unwrap();
            let visible = cx.debug_bounds("dock-question-scroll").unwrap_or(value);
            if !contains(quote, label) || !contains(quote, visible) || overlaps(label, visible) {
                issues.push(format!(
                    "question paint escapes quote: quote={quote:?}, \
                     label={label:?}, value={visible:?}"
                ));
            }
        }
        let text_pairs = [
            ("composer-hold", "composer-hold-text"),
            ("dock-count", "dock-count-text"),
            ("approval-consequence", "approval-consequence-text"),
            ("approval-action-text", "approval-action-content"),
            ("approval-preview", "approval-preview-content"),
            ("approval-grant-button", "approval-grant-caption"),
            ("dock-focus-hint", "dock-focus-hint-text"),
            ("approval-working-folder", "approval-working-folder-label"),
            (
                "approval-working-folder-label",
                "approval-working-folder-label-content",
            ),
            (
                "approval-working-folder-text",
                "approval-working-folder-content",
            ),
            ("approval-working-folder", "approval-working-folder-scroll"),
            ("approval-provider-reason", "approval-provider-reason-label"),
            (
                "approval-provider-reason-label",
                "approval-provider-reason-label-content",
            ),
            (
                "approval-provider-reason-value",
                "approval-provider-reason-content",
            ),
            ("approval-provider-reason", "approval-provider-reason-value"),
            ("approval-provider-tool", "approval-provider-tool-label"),
            (
                "approval-provider-tool-label",
                "approval-provider-tool-label-content",
            ),
            (
                "approval-provider-tool-value",
                "approval-provider-tool-content",
            ),
            ("approval-provider-tool", "approval-provider-tool-value"),
            ("approval-provider-file", "approval-provider-file-label"),
            (
                "approval-provider-file-label",
                "approval-provider-file-label-content",
            ),
            (
                "approval-provider-file-value",
                "approval-provider-file-content",
            ),
            ("approval-provider-file", "approval-provider-file-value"),
            ("dock-provider-question", "dock-provider-question-label"),
            (
                "dock-provider-question-label",
                "dock-provider-question-label-content",
            ),
            (
                "dock-provider-question-value",
                "dock-provider-question-content",
            ),
            ("dock-provider-question", "dock-question-scroll"),
        ];
        for (region, content) in text_pairs {
            match (cx.debug_bounds(region), cx.debug_bounds(content)) {
                (Some(bounds), Some(text)) => {
                    if !contains(bounds, text) {
                        issues.push(format!(
                            "text escapes {region}: region={bounds:?}, content={text:?}"
                        ));
                    }
                }
                (None, None) => {}
                _ => issues.push(format!(
                    "missing text selector: region={region}, content={content}"
                )),
            }
        }
        let labels = shell.read_with(cx, |shell, _| {
            shell
                .dock_target()
                .and_then(|target| {
                    shell
                        .state
                        .requests
                        .iter()
                        .find(|r| r.id == target.request)
                        .and_then(|r| match &r.kind {
                            RequestKind::Question { questions } => {
                                questions.get(target.question).map(|q| q.options.clone())
                            }
                            _ => None,
                        })
                })
                .unwrap_or_default()
        });
        if labels.len() <= 4
            && let Some(first) = cx.debug_bounds(DOCK_OPTIONS[0].0)
        {
            for (button, _) in &DOCK_OPTIONS[1..labels.len().max(1)] {
                if let Some(bounds) = cx.debug_bounds(button)
                    && (bounds.left() != first.left() || bounds.size.width != first.size.width)
                {
                    issues.push(format!(
                        "up to four options require one column: \
                         first={first:?}, other={bounds:?}"
                    ));
                }
            }
        }
        for ((button, caption), (index, label)) in
            DOCK_OPTIONS.iter().zip(labels.iter().enumerate())
        {
            let button = cx.debug_bounds(button).unwrap();
            let caption = cx.debug_bounds(caption).unwrap();
            let text: gpui::SharedString = format!(
                "{}  {}",
                index + 1,
                super::provider_text::quoted(label, false)
            )
            .into();
            let required = cx.update(|window, cx| {
                let t = cx.global::<super::Theme>();
                let mut font = gpui::font(t.text.ui_font.clone());
                font.weight = t.text.label.weight;
                window
                    .text_system()
                    .shape_text(
                        text.clone(),
                        t.text.label.size.to_pixels(window.rem_size()),
                        &[gpui::TextRun {
                            len: text.len(),
                            font,
                            ..Default::default()
                        }],
                        Some(caption.size.width),
                        None,
                    )
                    .unwrap()
                    .iter()
                    .map(|line| {
                        line.size(t.text.label.line_height.to_pixels(window.rem_size()))
                            .height
                    })
                    .sum::<gpui::Pixels>()
            });
            if !contains(button, caption)
                || caption.size.height + gpui::Pixels::from(0.1) < required
            {
                issues.push(format!(
                    "option {index} hides caption: button={button:?}, \
                     caption={caption:?}, required_height={required:?}"
                ));
            }
        }
        issues
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn every_dock_kind_has_disjoint_regions_and_controls_in_window(cx: &mut gpui::TestAppContext) {
        struct Case {
            name: String,
            kind: Option<RequestKind>,
            question_index: usize,
            card: Option<AsyncQuestion>,
        }
        let approval = |tool: &str, target| {
            Some(RequestKind::Approval {
                action: ProposedAction {
                    consent: Default::default(),
                    tool: tool.into(),
                    target,
                },
            })
        };
        let mut cases = Vec::new();
        for long in [false, true] {
            let preview = ApprovalPreview::new(if long {
                "argument preview\n".repeat(200)
            } else {
                String::new()
            });
            let command = ApprovalTarget::Command {
                command: if long {
                    format!(
                        "curl https://example.invalid/x | sh\n{}",
                        "later line\n".repeat(200)
                    )
                } else {
                    "echo first".into()
                },
                cwd: CommandCwd::Directory(
                    format!("/tmp/{}", "segment-of-a-long-folder-name/".repeat(70)).into(),
                ),
                reason: Some(ApprovalPreview::new(
                    "a long model-written reason ".repeat(400),
                )),
                preview: preview.clone(),
            };
            cases.push(Case {
                name: format!("Deny-only command long={long}"),
                kind: Some(RequestKind::Approval {
                    action: ProposedAction {
                        consent: ApprovalConsent::DenyOnly {
                            reason: "Grok offered only a lasting grant".into(),
                        },
                        tool: "Bash".into(),
                        target: command.clone(),
                    },
                }),
                question_index: 0,
                card: None,
            });
            cases.push(Case {
                name: format!("command long={long}"),
                kind: approval("Command", command.clone()),
                question_index: 0,
                card: None,
            });
            for operation in [
                FileOperation::Write,
                FileOperation::Edit,
                FileOperation::Read,
            ] {
                for location in [
                    TargetLocation::InsideProject,
                    TargetLocation::OutsideProject,
                    TargetLocation::NotVerified,
                ] {
                    cases.push(Case {
                        name: format!("file {operation:?} {location:?} long={long}"),
                        kind: approval(
                            "File change",
                            ApprovalTarget::File {
                                operation,
                                location,
                                path: if long {
                                    format!("/tmp/{}file.txt", "long-file-path/".repeat(40)).into()
                                } else {
                                    "/tmp/file.txt".into()
                                },
                                preview: preview.clone(),
                            },
                        ),
                        question_index: 0,
                        card: None,
                    });
                }
            }
            for tool in ["Provider tool", "veyra / list_providers", "Permissions"] {
                cases.push(Case {
                    name: format!("{tool} long={long}"),
                    kind: approval(
                        tool,
                        ApprovalTarget::Tool {
                            preview: preview.clone(),
                        },
                    ),
                    question_index: 0,
                    card: None,
                });
            }
            for count in [2, 9] {
                let card = AsyncQuestion {
                    title: if long {
                        "q".repeat(400)
                    } else {
                        "Which route?".into()
                    },
                    options: (0..count).map(|i| format!("Option number {i}")).collect(),
                    selected: None,
                };
                cases.push(Case {
                    name: format!("async card without dock long={long} count={count}"),
                    kind: None,
                    question_index: 0,
                    card: Some(card.clone()),
                });
                cases.push(Case {
                    name: format!("async card with command dock long={long} count={count}"),
                    kind: approval("Command", command.clone()),
                    question_index: 0,
                    card: Some(card),
                });
            }
            for (count, realistic) in [(2, false), (3, true), (4, true), (9, false), (9, true)] {
                for question_index in 0..3 {
                    let question = Question {
                        text: if long {
                            "Which route should the agent take through this long question text? "
                                .repeat(8)
                                .chars()
                                .take(400)
                                .collect()
                        } else {
                            "Choose a route".into()
                        },
                        options: (0..count)
                            .map(|i| {
                                if !realistic {
                                    format!("Option number {i}")
                                } else if count <= 4 {
                                    REALISTIC_OPTION_LABELS[i].into()
                                } else {
                                    format!("Option {}: {LONG_OPTION_LABEL}", i + 1)
                                }
                            })
                            .collect(),
                    };
                    cases.push(Case {
                        name: format!("question count={count} realistic={realistic} long={long} multi={question_index}"),
                        kind: Some(RequestKind::Question {
                            questions: vec![question; if question_index == 0 { 1 } else { 2 }],
                        }),
                        question_index: usize::from(question_index == 2),
                        card: None,
                    });
                }
            }
        }
        with_docked_shell(cx, |shell, cx, _, _| {
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            let mut failures = Vec::new();
            let mut checked = 0;
            let mut question_failures = 0;
            for viewport in [theme.layout.window_min, theme.layout.window_default] {
                cx.simulate_resize(viewport);
                for provider in ProviderKind::ALL {
                    for case in &cases {
                        for flags in 0..16 {
                            shell.update_in(cx, |shell, window, cx| {
                                let (thread, turn) =
                                    configure_dock_thread(shell, provider, flags & 1 != 0, false);
                                shell.state.requests.clear();
                                shell.question_draft = None;
                                if let Some(kind) = &case.kind {
                                    let id = RequestId::new();
                                    shell.state.requests.push(Request {
                                        id,
                                        thread,
                                        turn,
                                        native: NativeRequestRef("matrix-request".into()),
                                        state: RequestState::Pending,
                                        kind: kind.clone(),
                                    });
                                    if case.question_index == 1 {
                                        shell.question_draft = Some((
                                            id,
                                            vec![Answer {
                                                text: "Option number 0".into(),
                                            }],
                                        ));
                                    }
                                    if flags & 2 != 0 {
                                        let mut second = shell.state.requests[0].clone();
                                        second.id = RequestId::new();
                                        shell.state.requests.push(second);
                                    }
                                }
                                shell.state.items.clear();
                                shell.state.item_order.clear();
                                shell.state.markdown_text.clear();
                                if let Some(card) = &case.card {
                                    shell.state.apply(UiEvent::Change {
                                        counter: None,
                                        event: ChangeEvent::ItemAdded(Item {
                                            id: ItemId::new(),
                                            thread,
                                            turn,
                                            seq: 1,
                                            body: ItemBody::AsyncQuestions {
                                                text: String::new(),
                                                questions: vec![card.clone()],
                                            },
                                        }),
                                    });
                                }
                                let count = shell.state.transcript_rows(thread);
                                shell
                                    .scroller
                                    .update(cx, |state, cx| state.reset(count, cx));
                                shell.input.update(cx, |input, cx| {
                                    input.set_value(
                                        if flags & 8 != 0 {
                                            "draft line\n".repeat(40)
                                        } else {
                                            String::new()
                                        },
                                        window,
                                        cx,
                                    )
                                });
                                shell.dock_focused = flags & 4 != 0;
                                if shell.dock_focused {
                                    window.focus(&shell.dock_focus, cx);
                                } else {
                                    shell.input.update(cx, |input, cx| input.focus(window, cx));
                                }
                                cx.notify();
                            });
                            draw(cx);
                            let mut issues = dock_layout_issues(&shell, cx);
                            assert!(cx.debug_bounds("composer-discard").is_none());
                            if case.card.is_some() && viewport == theme.layout.window_default {
                                assert!(cx.debug_bounds("async-question-title").is_some());
                            }
                            assert_eq!(
                                shell.read_with(cx, |shell, _| shell.dock_target().is_some()),
                                case.kind.is_some()
                            );
                            if let Some(RequestKind::Question { questions }) = &case.kind {
                                for (name, _) in
                                    &DOCK_OPTIONS[..questions[case.question_index].options.len()]
                                {
                                    assert!(cx.debug_bounds(name).is_some(), "missing {name}");
                                }
                                if !issues.is_empty() {
                                    question_failures += 1;
                                }
                            }
                            checked += 1;
                            if !issues.is_empty() {
                                issues.truncate(4);
                                failures.push(format!(
                                    "{viewport:?} {provider:?} {} flags={flags:04b}: {issues:?}",
                                    case.name
                                ));
                            }
                        }
                    }
                }
            }
            eprintln!(
                "all dock matrix: {checked} combinations, {} failures, {question_failures} question failures",
                failures.len()
            );
            assert_eq!(checked, cases.len() * ProviderKind::ALL.len() * 32);
            assert!(
                failures.is_empty(),
                "{} failures; first cases: {:#?}",
                failures.len(),
                &failures[..failures.len().min(12)]
            );
        });
        question_quote_minimum_matrix(cx);
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn held_and_queued_controls_stay_in_window(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            let mut checked = 0;
            for viewport in [theme.layout.window_min, theme.layout.window_default] {
                cx.simulate_resize(viewport);
                for provider in [ProviderKind::Claude, ProviderKind::Codex] {
                    shell.update(cx, |shell, cx| {
                        configure_dock_thread(shell, provider, true, true);
                        cx.notify();
                    });
                    draw(cx);
                    assert!(cx.debug_bounds("composer-continue").is_some());
                    assert!(cx.debug_bounds("composer-discard").is_some());
                    let issues = dock_layout_issues(&shell, cx);
                    assert!(issues.is_empty(), "{viewport:?} {provider:?}: {issues:?}");
                    checked += 1;
                }
            }
            assert_eq!(checked, 4);
            eprintln!("held and queued controls: {checked} window/provider cases passed");
        });
    }

    #[cfg(debug_assertions)]
    fn question_quote_minimum_matrix(cx: &mut gpui::TestAppContext) {
        use gpui_kit::{component::StyledExt, prelude::*, *};
        struct QuoteBudget {
            text: String,
            question: ScrollHandle,
            draft: ScrollHandle,
        }
        impl Render for QuoteBudget {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let t = cx.global::<super::Theme>();
                div()
                    .v_flex()
                    .line_height(t.text.ui.line_height)
                    .h(t.layout.dock_question_min_height
                        + t.layout.icon_button
                        + t.layout.composer_min_height)
                    .child(
                        super::provider_text::QuotedText::new(
                            super::provider_text::QuoteKind::Question,
                            "Question from the provider".into(),
                            &self.text,
                            false,
                        )
                        .render_question(&self.question, cx),
                    )
                    .child(
                        div()
                            .flex_none()
                            .h(t.layout.icon_button)
                            .debug_selector(|| "minimum-choice".into())
                            .child("Option"),
                    )
                    .child(super::provider_text::bounded_scroll(
                        "minimum-draft",
                        "minimum-draft",
                        &self.draft,
                        div().child("draft line\n".repeat(40)),
                        t.layout.composer_min_height.into(),
                        t.layout.composer_max_height.into(),
                        cx,
                    ))
            }
        }
        let (probe, cx) = cx.add_window_view(|window, cx| {
            super::Theme::apply(super::theme::Appearance::Dark, window, cx);
            QuoteBudget {
                text: String::new(),
                question: ScrollHandle::new(),
                draft: ScrollHandle::new(),
            }
        });
        let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
        let mut checked = 0;
        for viewport in [theme.layout.window_min, theme.layout.window_default] {
            cx.simulate_resize(viewport);
            for text in ["Choose a route".into(), "q".repeat(400)] {
                probe.update(cx, |probe, cx| {
                    probe.text = text;
                    cx.notify();
                });
                draw(cx);
                let quote = cx.debug_bounds("dock-provider-question").unwrap();
                let label = cx.debug_bounds("dock-provider-question-label").unwrap();
                let text = cx.debug_bounds("dock-question-scroll").unwrap();
                let option = cx.debug_bounds("minimum-choice").unwrap();
                let draft = cx.debug_bounds("minimum-draft").unwrap();
                assert!(
                    text.size.height
                        >= cx.update(|window, _| {
                            theme.text.ui.line_height.to_pixels(window.rem_size())
                        })
                        && label.bottom() <= text.top()
                        && text.bottom() <= quote.bottom()
                        && quote.bottom() <= option.top()
                        && option.bottom() <= draft.top(),
                    "one-line flex budget: quote={quote:?}, label={label:?}, \
                     text={text:?}, option={option:?}, draft={draft:?}"
                );
                checked += 1;
            }
        }
        eprintln!("question minimum matrix: {checked} flex-pressure cases passed");
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn docked_draft_stays_in_its_viewport_and_scrolls_to_the_end(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            cx.simulate_resize(theme.layout.window_min);
            let draft = "draft line\n".repeat(40);
            shell.update_in(cx, |shell, window, cx| {
                shell.input.update(cx, |input, cx| {
                    input.set_value(draft.clone(), window, cx);
                });
                cx.notify();
            });
            draw(cx);
            let viewport = cx.debug_bounds("composer-input").unwrap();
            let toolbar = cx.debug_bounds("composer-toolbar").unwrap();
            let editor = shell.read_with(cx, |shell, cx| shell.input.read(cx).input_bounds());
            assert!(
                editor.top() >= viewport.top()
                    && editor.bottom() <= viewport.bottom()
                    && viewport.bottom() <= toolbar.top(),
                "docked editor escapes input: editor={editor:?}, viewport={viewport:?}, \
                 toolbar={toolbar:?}"
            );
            shell.update(cx, |shell, cx| {
                shell.input.update(cx, |input, cx| {
                    input.set_scroll_offset(gpui::point(gpui::Pixels::ZERO, (-100000.).into()), cx);
                });
            });
            draw(cx);
            shell.read_with(cx, |shell, cx| {
                let input = shell.input.read(cx);
                assert_eq!(input.value().as_ref(), draft);
                assert!(input.scroll_offset().y < gpui::Pixels::ZERO);
                assert!(input.visible_row_range().unwrap().end >= 40);
            });
            shell.update(cx, |shell, cx| {
                shell.state.requests.clear();
                cx.notify();
            });
            draw(cx);
            assert!(cx.debug_bounds("composer-input").unwrap().size.height > viewport.size.height);
            assert_eq!(
                shell.read_with(cx, |shell, cx| shell.input.read(cx).value()),
                draft
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn question_scroll_resets_for_each_question_and_request(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            cx.simulate_resize(theme.layout.window_min);
            shell.update(cx, |shell, cx| {
                configure_dock_thread(shell, ProviderKind::Claude, true, false);
                shell.state.requests[0].kind = RequestKind::Question {
                    questions: (0..2)
                        .map(|i| Question {
                            text: format!("Question {i}: {}", "q".repeat(400)),
                            options: (0..9).map(|i| format!("Option {i}")).collect(),
                        })
                        .collect(),
                };
                cx.notify();
            });
            draw(cx);
            let label = cx.debug_bounds("dock-provider-question-label").unwrap();
            let option = cx.debug_bounds("dock-option-0").unwrap();
            let wheel = |cx: &mut gpui::VisualTestContext| {
                let viewport = cx.debug_bounds("dock-question-scroll").unwrap();
                cx.simulate_event(gpui::ScrollWheelEvent {
                    position: viewport.center(),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::Pixels::ZERO,
                        (-100000.).into(),
                    )),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                });
                draw(cx);
            };
            wheel(cx);
            let offset = shell.read_with(cx, |shell, _| shell.question_scroll.handle.offset());
            assert!(offset.y < gpui::Pixels::ZERO, "long question must scroll");
            assert_eq!(
                cx.debug_bounds("dock-provider-question-label").unwrap(),
                label
            );
            assert_eq!(cx.debug_bounds("dock-option-0").unwrap(), option);
            shell.update(cx, |_, cx| cx.notify());
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.question_scroll.handle.offset()),
                offset
            );
            shell.update(cx, |shell, cx| {
                shell.question_draft = Some((
                    shell.state.requests[0].id,
                    vec![Answer {
                        text: "Option 0".into(),
                    }],
                ));
                cx.notify();
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.question_scroll.handle.offset().y),
                gpui::Pixels::ZERO
            );
            wheel(cx);
            assert!(
                shell.read_with(cx, |shell, _| shell.question_scroll.handle.offset().y)
                    < gpui::Pixels::ZERO
            );
            shell.update(cx, |shell, cx| {
                shell.state.requests[0].id = RequestId::new();
                shell.question_draft = None;
                cx.notify();
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.question_scroll.handle.offset().y),
                gpui::Pixels::ZERO
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn option_scroll_resets_for_each_question_and_request(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            cx.simulate_resize(theme.layout.window_min);
            shell.update(cx, |shell, cx| {
                configure_dock_thread(shell, ProviderKind::Claude, true, true);
                shell.state.requests[0].kind = RequestKind::Question {
                    questions: (0..2)
                        .map(|i| Question {
                            text: format!("Question {i}: {}", "q".repeat(400)),
                            options: (0..9)
                                .map(|i| format!("Option {i}: {LONG_OPTION_LABEL}"))
                                .collect(),
                        })
                        .collect(),
                };
                cx.notify();
            });
            draw(cx);
            let scroll = |cx: &mut gpui::VisualTestContext| {
                let viewport = cx.debug_bounds("dock-options").unwrap();
                cx.simulate_event(gpui::ScrollWheelEvent {
                    position: viewport.center(),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::Pixels::ZERO,
                        (-100000.).into(),
                    )),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                });
                draw(cx);
                shell.read_with(cx, |shell, _| {
                    assert!(
                        shell.question_scroll.options.max_offset().y > gpui::Pixels::ZERO,
                        "overflow draws the shared always-visible scrollbar"
                    );
                    assert!(shell.question_scroll.options.offset().y < gpui::Pixels::ZERO);
                    assert_eq!(
                        shell.question_scroll.handle.offset().y,
                        gpui::Pixels::ZERO,
                        "option scroll must not scroll the question"
                    );
                });
            };
            scroll(cx);
            let offset = shell.read_with(cx, |shell, _| shell.question_scroll.options.offset());
            shell.update(cx, |_, cx| cx.notify());
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.question_scroll.options.offset()),
                offset
            );
            shell.update(cx, |shell, cx| {
                shell.question_draft = Some((
                    shell.state.requests[0].id,
                    vec![Answer {
                        text: "first choice".into(),
                    }],
                ));
                cx.notify();
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.question_scroll.options.offset().y),
                gpui::Pixels::ZERO
            );
            scroll(cx);
            shell.update(cx, |shell, cx| {
                shell.state.requests[0].id = RequestId::new();
                shell.question_draft = None;
                cx.notify();
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.question_scroll.options.offset().y),
                gpui::Pixels::ZERO
            );
            shell.update(cx, |shell, cx| {
                shell.state.requests[0].kind = RequestKind::Question {
                    questions: vec![Question {
                        text: "Choose".into(),
                        options: vec!["Violet".into(), "Blue".into()],
                    }],
                };
                cx.notify();
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.question_scroll.options.max_offset().y),
                gpui::Pixels::ZERO,
                "short options do not draw a scrollbar"
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn approval_height_matrix_keeps_fixed_controls_and_first_line(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            let typical = "/Users/someone/.t3/worktrees/veyra/feature-review/crates/engine";
            let folders = [
                CommandCwd::ProjectFolder,
                CommandCwd::NotVerified,
                CommandCwd::Directory("/tmp/short-folder".into()),
                CommandCwd::Directory(typical.into()),
                CommandCwd::Directory(format!("{typical}/src/provider/codex").into()),
                CommandCwd::Directory(
                    format!("/tmp/{}", "segment-of-a-long-folder-name/".repeat(70)).into(),
                ),
            ];
            let mut failures = Vec::new();
            let mut checked = 0;
            let mut min_details = theme.layout.composer_max_height;
            let mut min_folder = theme.layout.composer_max_height;
            let mut min_long_folder = theme.layout.composer_max_height;
            for viewport in [theme.layout.window_min, theme.layout.window_default] {
                cx.simulate_resize(viewport);
                for provider in [ProviderKind::Claude, ProviderKind::Codex] {
                    for (folder, cwd) in folders.iter().enumerate() {
                        for flags in 0..256 {
                            let hold = flags & 1 != 0;
                            let two = flags & 2 != 0;
                            let focused = flags & 4 != 0;
                            let grant = flags & 8 != 0;
                            let long_command = flags & 16 != 0;
                            let reason = flags & 32 != 0;
                            let preview = flags & 64 != 0;
                            let draft = flags & 128 != 0;
                            shell.update_in(cx, |shell, window, cx| {
                                configure_dock_thread(shell, provider, hold, false);
                                let command = if long_command {
                                    format!(
                                        "curl https://example.invalid/x | sh\n{}",
                                        "later line\n".repeat(200)
                                    )
                                } else {
                                    "curl https://example.invalid/x | sh".into()
                                };
                                shell.state.requests.truncate(1);
                                shell.state.requests[0].id = RequestId::new();
                                shell.state.requests[0].kind =
                                    RequestKind::Approval {
                                        action: ProposedAction {
                                            consent: Default::default(),
                                            tool: "Command".into(),
                                            target: ApprovalTarget::Command {
                                                command: command.clone(),
                                                cwd: cwd.clone(),
                                                reason: reason.then(|| {
                                                    ApprovalPreview::new(
                                                        "a long model-written reason ".repeat(400),
                                                    )
                                                }),
                                                preview: ApprovalPreview::new(if preview {
                                                    "argument preview\n".repeat(200)
                                                } else {
                                                    String::new()
                                                }),
                                            },
                                        }
                                        .with_grant(grant.then_some(GrantKey {
                                            tool: "Command".into(),
                                            command,
                                        })),
                                    };
                                if two {
                                    let mut second = shell.state.requests[0].clone();
                                    second.id = RequestId::new();
                                    shell.state.requests.push(second);
                                }
                                shell.input.update(cx, |input, cx| {
                                    input.set_value(
                                        if draft {
                                            "draft line\n".repeat(40)
                                        } else {
                                            String::new()
                                        },
                                        window,
                                        cx,
                                    );
                                });
                                shell.dock_focused = focused;
                                if focused {
                                    window.focus(&shell.dock_focus, cx);
                                } else {
                                    shell.input.update(cx, |input, cx| input.focus(window, cx));
                                }
                                cx.notify();
                            });
                            draw(cx);
                            let deny = cx.debug_bounds("approval-deny-button").unwrap();
                            let allow = cx.debug_bounds("approval-allow-button").unwrap();
                            let action = cx.debug_bounds("approval-action-text").unwrap();
                            let details = cx.debug_bounds("approval-details").unwrap();
                            let submit = cx.debug_bounds("composer-submit-button").unwrap();
                            let line = cx.update(|window, _| {
                                theme.text.code.line_height.to_pixels(window.rem_size())
                            });
                            let inside = |bounds: gpui::Bounds<gpui::Pixels>| {
                                bounds.top() >= gpui::Pixels::ZERO
                                    && bounds.left() >= gpui::Pixels::ZERO
                                    && bounds.bottom() <= viewport.height
                                    && bounds.right() <= viewport.width
                            };
                            let first_line = action.top() == details.top()
                                && details.size.height >= line
                                && inside(gpui::Bounds::new(
                                    action.origin,
                                    gpui::size(details.size.width, line),
                                ));
                            min_details = min_details.min(details.size.height);
                            let folder_visible = cx
                                .debug_bounds("approval-working-folder-scroll")
                                .is_none_or(|folder| {
                                    min_folder = min_folder.min(folder.size.height);
                                    let long = matches!(cwd,
                                        CommandCwd::Directory(path) if path.as_os_str().len() > 2000
                                    );
                                    if long {
                                        min_long_folder = min_long_folder.min(folder.size.height);
                                    }
                                    let line = cx.update(|window, _| {
                                        theme.text.label.line_height.to_pixels(window.rem_size())
                                    });
                                    let height = if long {
                                        folder.size.height == line * 3.
                                    } else {
                                        folder.size.height >= line
                                            && folder.size.height <= line * 3.
                                    };
                                    height && inside(folder) && folder.bottom() <= details.top()
                                });
                            let fixed = [
                                "approval-consequence",
                                "approval-working-folder",
                                "caption-Continue",
                                "caption-Focus to use shortcuts",
                                "approval-grant-button",
                            ]
                            .into_iter()
                            .filter_map(|selector| cx.debug_bounds(selector))
                            .all(inside);
                            let geometry = dock_layout_issues(&shell, cx);
                            assert!(cx.debug_bounds("composer-discard").is_none());
                            checked += 1;
                            if !(geometry.is_empty()
                                && inside(deny)
                                && inside(allow)
                                && inside(submit)
                                && first_line
                                && inside(details)
                                && folder_visible
                                && fixed
                                && details.size.height <= theme.layout.composer_max_height)
                            {
                                failures.push(format!(
                                    "{viewport:?} {provider:?} folder={folder} flags={flags:08b} \
                                     deny={deny:?} allow={allow:?} submit={submit:?} details={details:?} \
                                     first_line={first_line} folder_visible={folder_visible} \
                                     fixed={fixed} geometry={geometry:?}"
                                ));
                            }
                        }
                    }
                }
            }
            eprintln!(
                "approval height matrix: {checked} combinations, {} failures; \
                 min_details={min_details:?}, min_folder={min_folder:?}, \
                 min_long_folder={min_long_folder:?}; \
                 flags=hold,two,focused,grant,long_command,reason,preview,draft",
                failures.len()
            );
            assert_eq!(checked, 6144);
            assert!(
                failures.is_empty(),
                "{} failures; first cases: {:#?}",
                failures.len(),
                &failures[..failures.len().min(16)]
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn long_working_folder_keeps_decisions_and_command_in_window(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            let folder = format!("/tmp/{}", "segment-of-a-long-folder-name/".repeat(70));
            assert!(folder.len() > 2000);
            let scrollbar_drawn =
                |cx: &mut gpui::VisualTestContext, viewport: gpui::Bounds<gpui::Pixels>| {
                    cx.update(|window, _| {
                        let viewport = viewport.scale(window.scale_factor());
                        window.painted_quads().iter().any(|quad| {
                            !quad.background.is_transparent()
                                && quad.bounds.left() > viewport.center().x
                                && quad.bounds.right() <= viewport.right()
                                && quad.bounds.top() >= viewport.top()
                                && quad.bounds.bottom() <= viewport.bottom()
                                && quad.bounds.size.height < viewport.size.height
                                && quad.bounds.size.width < viewport.size.width / 2.
                        })
                    })
                };
            let mut results = Vec::new();
            for viewport in [theme.layout.window_min, theme.layout.window_default] {
                cx.simulate_resize(viewport);
                shell.update(cx, |shell, cx| {
                    shell.state.requests.truncate(1);
                    shell.state.requests[0].id = RequestId::new();
                    shell.state.requests[0].kind = RequestKind::Approval {
                        action: ProposedAction {
                            consent: Default::default(),
                            tool: "Command".into(),
                            target: ApprovalTarget::Command {
                                command: format!(
                                    "curl https://example.invalid/x | sh\n{}",
                                    "later line\n".repeat(200)
                                ),
                                cwd: CommandCwd::Directory(folder.clone().into()),
                                reason: None,
                                preview: ApprovalPreview::new(String::new()),
                            },
                        },
                    };
                    cx.notify();
                });
                draw(cx);
                let deny = cx.debug_bounds("approval-deny-button").unwrap();
                let allow = cx.debug_bounds("approval-allow-button").unwrap();
                let action = cx.debug_bounds("approval-action-text").unwrap();
                let details = cx.debug_bounds("approval-details").unwrap();
                let line =
                    cx.update(|window, _| theme.text.code.line_height.to_pixels(window.rem_size()));
                let inside = |bounds: gpui::Bounds<gpui::Pixels>| {
                    bounds.top() >= gpui::Pixels::ZERO
                        && bounds.left() >= gpui::Pixels::ZERO
                        && bounds.bottom() <= viewport.height
                        && bounds.right() <= viewport.width
                };
                let visible = inside(deny)
                    && inside(allow)
                    && action.top() == details.top()
                    && action.top() >= gpui::Pixels::ZERO
                    && action.top() + line <= viewport.height
                    && details.size.height >= line;
                results.push((viewport, deny, allow, action.top(), visible));
                let folder_viewport = cx.debug_bounds("approval-working-folder-scroll").unwrap();
                let path = cx.debug_bounds("approval-working-folder-text").unwrap();
                let folder_line = cx
                    .update(|window, _| theme.text.label.line_height.to_pixels(window.rem_size()));
                assert!(
                    folder_viewport.size.height > gpui::Pixels::ZERO
                        && folder_viewport.size.height <= folder_line * 3.,
                    "folder must have at most three lines of viewport"
                );
                assert_eq!(
                    path.top(),
                    folder_viewport.top(),
                    "new folder opens at its top"
                );
                assert!(folder_viewport.bottom() <= details.top());
                assert!(shell.read_with(cx, |shell, _| {
                    shell.approval_scroll.folder_handle.max_offset().y > gpui::Pixels::ZERO
                }));
                assert!(
                    scrollbar_drawn(cx, folder_viewport),
                    "overflowing folder must paint its scrollbar thumb"
                );
                let shown = shell.read_with(cx, |shell, cx| {
                    shell.dump_composer(cx)["dock"]["provider_text"][0]["text"].clone()
                });
                assert_eq!(
                    shown,
                    serde_json::json!(format!("“{folder}”")),
                    "the complete marked path must not be elided"
                );
                cx.simulate_event(gpui::ScrollWheelEvent {
                    position: folder_viewport.center(),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::Pixels::ZERO,
                        (-100000.).into(),
                    )),
                    touch_phase: gpui::TouchPhase::Moved,
                    ..Default::default()
                });
                draw(cx);
                let end = cx.debug_bounds("approval-working-folder-text").unwrap();
                assert!(end.top() < path.top(), "folder must scroll independently");
                assert!(
                    end.bottom() <= folder_viewport.bottom()
                        && end.bottom() > folder_viewport.top(),
                    "path end must be readable in its viewport"
                );
                assert_eq!(
                    cx.debug_bounds("approval-action-text").unwrap().top(),
                    action.top(),
                    "folder scrolling must leave the command at its first line"
                );
                assert_eq!(cx.debug_bounds("approval-deny-button").unwrap(), deny);
                assert_eq!(cx.debug_bounds("approval-allow-button").unwrap(), allow);
                shell.update(cx, |shell, cx| {
                    shell.state.requests[0].id = RequestId::new();
                    cx.notify();
                });
                draw(cx);
                assert_eq!(
                    cx.debug_bounds("approval-working-folder-text")
                        .unwrap()
                        .top(),
                    cx.debug_bounds("approval-working-folder-scroll")
                        .unwrap()
                        .top(),
                    "replacement request must reset the folder's own scroll"
                );
                shell.update(cx, |shell, cx| {
                    shell.state.requests[0].id = RequestId::new();
                    if let RequestKind::Approval { action, .. } = &mut shell.state.requests[0].kind
                        && let ApprovalTarget::Command { cwd, .. } = &mut action.target
                    {
                        *cwd = CommandCwd::Directory("/tmp/short-folder".into());
                    }
                    cx.notify();
                });
                draw(cx);
                let short_viewport = cx.debug_bounds("approval-working-folder-scroll").unwrap();
                assert_eq!(
                    shell.read_with(cx, |shell, _| {
                        shell.approval_scroll.folder_handle.max_offset().y
                    }),
                    gpui::Pixels::ZERO
                );
                assert!(
                    !scrollbar_drawn(cx, short_viewport),
                    "a folder that fits must not paint a scrollbar thumb"
                );
            }
            assert!(
                results.iter().all(|result| result.4),
                "long cwd hid command or decisions: {results:?}"
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn approval_scroll_dump_requires_rendered_request_identity(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            draw(cx);
            assert!(shell.read_with(cx, |shell, cx| {
                shell.dump_composer(cx)["dock"]
                    .as_object()
                    .unwrap()
                    .contains_key("details_scroll")
            }));
            let changed = shell.update(cx, |shell, cx| {
                shell.state.requests[0].id = RequestId::new();
                cx.notify();
                shell.dump_composer(cx)["dock"].clone()
            });
            assert!(
                !changed.as_object().unwrap().contains_key("details_scroll"),
                "stale scroll state must not be paired with a new request"
            );
            draw(cx);
            assert!(shell.read_with(cx, |shell, cx| {
                shell.dump_composer(cx)["dock"]
                    .as_object()
                    .unwrap()
                    .contains_key("details_scroll")
            }));
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn outside_working_folder_stays_above_scrolling_details(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        consent: Default::default(),
                        tool: "Command".into(),
                        target: ApprovalTarget::Command {
                            command: format!("echo first\n{}", "later line\n".repeat(200)),
                            cwd: CommandCwd::Directory("/tmp/elsewhere".into()),
                            reason: Some(ApprovalPreview::new("Read the full command".into())),
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
                };
                cx.notify();
            });
            draw(cx);
            let details = cx.debug_bounds("approval-details").unwrap();
            let folder = cx.debug_bounds("approval-working-folder").unwrap();
            assert!(
                folder.bottom() <= details.top(),
                "working folder must be visible above the scroll area"
            );
            let reason = cx.debug_bounds("approval-provider-reason").unwrap();
            cx.simulate_event(gpui::ScrollWheelEvent {
                position: details.center(),
                delta: gpui::ScrollDelta::Pixels(gpui::point(
                    gpui::Pixels::ZERO,
                    (-100000.).into(),
                )),
                touch_phase: gpui::TouchPhase::Moved,
                ..Default::default()
            });
            draw(cx);
            assert_eq!(cx.debug_bounds("approval-working-folder").unwrap(), folder);
            assert!(
                cx.debug_bounds("approval-provider-reason").unwrap().top() < reason.top(),
                "reason must remain inside scrolling details"
            );
        });
    }

    #[cfg(debug_assertions)]
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
                        consent: Default::default(),
                        tool: "command tool".into(),
                        target: ApprovalTarget::Command {
                            command: command.clone(),
                            cwd: Default::default(),
                            reason: None,
                            preview: ApprovalPreview::new("argument preview\n".repeat(200)),
                        },
                    }
                    .with_grant(Some(GrantKey {
                        tool: "command tool".into(),
                        command: command.clone(),
                    })),
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
            let consequence = cx.debug_bounds("approval-consequence").unwrap();
            assert!(
                consequence.bottom() <= text.top(),
                "line count must stay above the scrolling command"
            );
            assert!(
                dock["consequence"]
                    .as_str()
                    .unwrap()
                    .contains("202-line command")
            );
            assert!(
                shell.read_with(cx, |shell, _| shell.approval_scroll.handle.max_offset().y
                    > gpui::Pixels::ZERO),
                "overflow must make the scrollbar draw"
            );
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
            assert_eq!(
                cx.debug_bounds("approval-consequence").unwrap(),
                consequence
            );
            assert_eq!(cx.debug_bounds("approval-deny-button").unwrap(), deny);
            assert_eq!(cx.debug_bounds("approval-allow-button").unwrap(), allow);
        });
    }

    #[cfg(debug_assertions)]
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
                            consent: Default::default(),
                            tool: "file tool".into(),
                            target: ApprovalTarget::File {
                                operation,
                                location,
                                path: "/tmp/x".into(),
                                preview: ApprovalPreview::new("contents to approve".into()),
                            },
                        },
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
                        consent: Default::default(),
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: command.into(),
                            cwd: Default::default(),
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
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
                        consent: Default::default(),
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: "echo ok".into(),
                            cwd: Default::default(),
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
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

    #[cfg(debug_assertions)]
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

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn allow_in_thread_chip_follows_keyboard_focus(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, sender| {
            shell.update(cx, |shell, cx| {
                for request in &mut shell.state.requests {
                    request.kind = RequestKind::Approval {
                        action: ProposedAction {
                            consent: Default::default(),
                            tool: "Bash".into(),
                            target: ApprovalTarget::Command {
                                command: "echo ok".into(),
                                cwd: Default::default(),
                                reason: None,
                                preview: ApprovalPreview::new(String::new()),
                            },
                        }
                        .with_grant(Some(GrantKey {
                            tool: "Bash".into(),
                            command: "echo ok".into(),
                        })),
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
                    event: ChangeEvent::RequestClosed(Box::new(answered)),
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

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn grok_deny_only_dock_disables_both_allows_and_keeps_deny(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let responses = shell.read_with(cx, |shell, _| shell.sent_responses.clone());
            let request = shell.update(cx, |shell, cx| {
                configure_dock_thread(shell, ProviderKind::Grok, false, false);
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        consent: ApprovalConsent::DenyOnly {
                            reason: "Grok offered only a lasting grant".into(),
                        },
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: "touch veyra-check.txt".into(),
                            cwd: CommandCwd::ProjectFolder,
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
                };
                cx.notify();
                shell.state.requests[0].id
            });
            draw(cx);
            assert!(cx.debug_bounds("approval-consent-reason").is_some());
            for selector in ["approval-allow-button", "approval-grant-button"] {
                let button = cx.debug_bounds(selector).unwrap().center();
                cx.simulate_click(button, gpui::Modifiers::none());
                draw(cx);
                assert!(
                    responses.borrow().is_empty(),
                    "disabled {selector} answered"
                );
            }
            cx.simulate_keystrokes("secondary-shift-y");
            draw(cx);
            for chord in ["secondary-enter", "secondary-alt-enter"] {
                cx.simulate_keystrokes(chord);
                draw(cx);
                assert!(responses.borrow().is_empty(), "disabled {chord} answered");
                assert!(shell.read_with(cx, |shell, _| shell.dock_focused));
            }
            cx.dispatch_action(super::AllowOnce {
                request: Some(request),
            });
            cx.dispatch_action(super::AllowInThread {
                request: Some(request),
            });
            draw(cx);
            assert!(responses.borrow().is_empty());
            let dump = shell.read_with(cx, |shell, cx| shell.dump_composer(cx));
            assert_eq!(dump["dock"]["allow_once_enabled"], false);
            assert_eq!(dump["dock"]["allow_in_thread_enabled"], false);
            assert_eq!(
                dump["dock"]["consent_reason"],
                "Grok offered only a lasting grant"
            );
            assert_eq!(dump["dock"]["grant_caption"], "Allow in this thread");
            let button = cx.debug_bounds("approval-deny-button").unwrap().center();
            cx.simulate_click(button, gpui::Modifiers::none());
            draw(cx);
            assert_eq!(responses.borrow().as_slice(), &[(request, Decision::Deny)]);
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn grok_no_catalog_menus_offer_only_default(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update_in(cx, |shell, window, cx| {
                configure_dock_thread(shell, ProviderKind::Grok, false, false);
                shell.state.requests.clear();
                shell.new_thread(window, cx);
                shell.provider = ProviderKind::Grok;
                shell.config.model = Some("earlier-model".into());
                shell.config.effort = Some("earlier-effort".into());
                cx.notify();
            });
            draw(cx);
            let button = cx.debug_bounds("composer-model").unwrap().center();
            cx.simulate_click(button, gpui::Modifiers::none());
            draw(cx);
            cx.simulate_keystrokes("down down enter");
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.config.model.clone()),
                None
            );
            let button = cx.debug_bounds("composer-effort").unwrap().center();
            cx.simulate_click(button, gpui::Modifiers::none());
            draw(cx);
            cx.simulate_keystrokes("down down enter");
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.config.effort.clone()),
                None
            );
            assert!(cx.debug_bounds("caption-Default model").is_some());
            assert!(cx.debug_bounds("caption-Effort").is_some());
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn chip_lists_offered_modes(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update_in(cx, |shell, window, cx| {
                shell.state.requests.clear();
                shell.provider = ProviderKind::Grok;
                shell.state.settings.default_mode = PermissionMode::AcceptEdits;
                shell.new_thread(window, cx);
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.config.mode),
                PermissionMode::AskFirst
            );
            assert!(cx.debug_bounds("caption-Ask first").is_some());
            let button = cx.debug_bounds("composer-permission").unwrap().center();
            cx.simulate_click(button, gpui::Modifiers::none());
            draw(cx);
            cx.simulate_keystrokes("down down enter");
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.config.mode),
                PermissionMode::FullAccess
            );
            shell.update_in(cx, |shell, window, cx| {
                shell.pick_permission(&super::PickPermission { mode: None }, window, cx);
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.config.mode),
                PermissionMode::AskFirst
            );
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.state.settings.default_mode),
                PermissionMode::AcceptEdits
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn permission_menu_opens_from_keyboard(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update_in(cx, |shell, window, cx| {
                shell.state.requests.clear();
                shell.provider = ProviderKind::Grok;
                shell.new_thread(window, cx);
            });
            draw(cx);
            for index in 0..20 {
                cx.update(|window, cx| window.focus_next(cx));
                draw(cx);
                cx.simulate_keystrokes("space");
                draw(cx);
                if shell.read_with(cx, |shell, _| shell.permission_picker_open) {
                    println!("permission menu opened after {} tab steps", index + 1);
                    return;
                }
                cx.simulate_keystrokes("escape");
                draw(cx);
            }
            panic!(
                "permission menu did not open; draft={:?}",
                shell.read_with(cx, |shell, cx| shell.input.read(cx).value().to_string())
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn grok_model_and_effort_cycles_include_default(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update_in(cx,|shell,window,cx| {
                configure_dock_thread(shell,ProviderKind::Grok,false,false);
                shell.state.requests.clear();
                shell.new_thread(window,cx);
                shell.provider=ProviderKind::Grok;
                shell.providers[0].models=serde_json::from_value(serde_json::json!([
                    {"value":"grok-4.7","displayName":"Grok 4.7","supportedEffortLevels":["high","medium"]},
                    {"value":"grok-4.5","displayName":"Grok 4.5","supportedEffortLevels":["high","medium"]}
                ])).unwrap();
                shell.config.model=None;
                shell.config.effort=None;
                window.dispatch_action(Box::new(super::PickModel::default()),cx);
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |s, _| s.config.model.clone()),
                Some("grok-4.7".into())
            );
            cx.simulate_keystrokes("secondary-alt-m");
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |s, _| s.config.model.clone()),
                Some("grok-4.5".into())
            );
            cx.simulate_keystrokes("secondary-alt-m");
            draw(cx);
            assert_eq!(shell.read_with(cx, |s, _| s.config.model.clone()), None);
            assert!(cx.debug_bounds("caption-Default model").is_some());
            shell.update_in(cx, |_, window, cx| {
                window.dispatch_action(Box::new(super::PickEffort::default()), cx)
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |s, _| s.config.effort.clone()),
                Some("high".into())
            );
            cx.simulate_keystrokes("secondary-alt-e");
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |s, _| s.config.effort.clone()),
                Some("medium".into())
            );
            cx.simulate_keystrokes("secondary-alt-e");
            draw(cx);
            assert_eq!(shell.read_with(cx, |s, _| s.config.effort.clone()), None);
            assert!(cx.debug_bounds("caption-Effort").is_some());
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn grok_model_change_clears_an_unadvertised_effort(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update_in(cx, |shell, window, cx| {
                configure_dock_thread(shell, ProviderKind::Grok, false, false);
                shell.state.requests.clear();
                shell.new_thread(window, cx);
                shell.provider = ProviderKind::Grok;
                shell.providers[0].models = serde_json::from_value(serde_json::json!([
                    {"value":"grok-4.7","displayName":"Grok 4.7","supportedEffortLevels":["high","xhigh","medium","low"]},
                    {"value":"grok-4.5","displayName":"Grok 4.5","supportedEffortLevels":["high","medium","low"]}
                ])).unwrap();
                shell.config.model = Some("grok-4.7".into());
                shell.config.effort = Some("xhigh".into());
                window.dispatch_action(Box::new(super::PickModel { model:Some("grok-4.5".into()) }), cx);
            });
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.config.effort.clone()),
                None
            );
            let button = cx.debug_bounds("composer-effort").unwrap().center();
            cx.simulate_click(button, gpui::Modifiers::none());
            draw(cx);
            cx.simulate_keystrokes("down enter");
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, _| shell
                    .config
                    .effort
                    .as_deref()
                    .map(str::to_owned)),
                Some("high".into())
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn approval_chips_follow_keyboard_focus(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, _| {
            shell.update(cx, |shell, cx| {
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        consent: Default::default(),
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: "touch veyra-check.txt".into(),
                            cwd: Default::default(),
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
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
                    event: ChangeEvent::RequestClosed(Box::new(answered)),
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
        let scratch = Scratch(
            std::path::PathBuf::from("/tmp").join(format!("veyra-quit-{}", ThreadId::new())),
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let engine = veyra_engine::Engine::start_with_env(
            runtime.handle(),
            veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
            veyra_engine::provider::env::ChildEnv::isolated(),
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

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn chat_branch_loads_when_the_thread_row_follows_selection(cx: &mut gpui::TestAppContext) {
        use std::{path::PathBuf, process::Command, time::Duration};

        let thread = ThreadId::new();
        let path = PathBuf::from("/tmp").join(format!("veyra-branch-{thread}"));
        let scratch = Scratch(path);
        std::fs::create_dir(&scratch.0).unwrap();
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
        let engine = veyra_engine::Engine::start_with_env(
            runtime.handle(),
            veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
            veyra_engine::provider::env::ChildEnv::isolated(),
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
                event: ChangeEvent::ThreadCreated(Box::new(Thread {
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
                })),
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
            event: ChangeEvent::ThreadCreated(Box::new(row.clone())),
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
            event: ChangeEvent::ThreadCreated(Box::new(Thread {
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
            })),
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
                done: false,
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
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let data = shell.read_with(cx, |shell, _| shell.probe_cwd.clone());
            let engine = veyra_engine::Engine::start_with_env(
                runtime.handle(),
                veyra_engine::data_dir::DataDir::at(data).unwrap(),
                veyra_engine::provider::env::ChildEnv::isolated(),
            )
            .unwrap();
            shell.update_in(cx, |shell, _, _| {
                shell.engine = engine.clone();
                shell.runtime = runtime.handle().clone();
            });
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
            assert!(shell.read_with(cx, |shell, _| shell.engine.is_closing()));
            cx.simulate_keystrokes("escape");
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| shell.quit_sheet));
            runtime.block_on(engine.shutdown_wait());
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn recheck_after_confirm_quit_launches_no_provider(cx: &mut gpui::TestAppContext) {
        use std::time::Duration;
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
        let engine = veyra_engine::Engine::start_with_env(
            runtime.handle(),
            veyra_engine::data_dir::DataDir::at(scratch.0.clone()).unwrap(),
            veyra_engine::provider::env::ChildEnv::isolated(),
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
            veyra_engine::provider::env::ChildEnv::isolated().with_binary(
                ProviderKind::Claude,
                std::env::var_os("VEYRA_CLAUDE_BIN").unwrap().into(),
            ),
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
            assert!(shell.engine.is_closing());
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
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn stop_mouse_release_does_not_retarget_after_repaint(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            draw(cx);
            let first = shell.read_with(cx, |shell, _| shell.stop_target().unwrap());
            let button = cx.debug_bounds("composer-submit-button").unwrap().center();
            cx.simulate_mouse_down(button, gpui::MouseButton::Left, gpui::Modifiers::none());
            let second = shell.update(cx, |shell, cx| {
                shell.state.turns.get_mut(&first.turn).unwrap().state = TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    started: Some(0),
                    at: 1,
                };
                let mut turn = shell.state.turns[&first.turn].clone();
                turn.id = TurnId::new();
                turn.ordinal += 1;
                turn.state = TurnState::Running { since: 1 };
                let id = turn.id;
                shell.state.turns.insert(id, turn);
                cx.notify();
                id
            });
            draw(cx);
            cx.simulate_mouse_up(button, gpui::MouseButton::Left, gpui::Modifiers::none());
            cx.run_until_parked();
            assert!(
                shell.read_with(cx, |shell, _| shell
                    .sent_interrupts
                    .iter()
                    .all(|target| target.turn != second)),
                "releasing A's Stop press after repaint must not interrupt B"
            );
            draw(cx);
            let button = cx.debug_bounds("composer-submit-button").unwrap().center();
            cx.simulate_click(button, gpui::Modifiers::none());
            cx.run_until_parked();
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.sent_interrupts.len()),
                1
            );
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.sent_interrupts[0].turn),
                second
            );
        });
    }

    #[gpui::test]
    fn stop_and_control_c_keep_the_rendered_turn(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            draw(cx);
            let first = shell.read_with(cx, |shell, _| shell.stop_target().unwrap());
            shell.update(cx, |shell, _| {
                shell.state.turns.get_mut(&first.turn).unwrap().state = TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    started: Some(0),
                    at: 1,
                };
                let mut second = shell.state.turns[&first.turn].clone();
                second.id = TurnId::new();
                second.ordinal += 1;
                second.state = TurnState::Running { since: 1 };
                shell.state.turns.insert(second.id, second);
            });
            cx.dispatch_action(super::Stop::default());
            cx.run_until_parked();
            assert!(shell.read_with(cx, |shell, _| {
                shell
                    .sent_interrupts
                    .iter()
                    .all(|target| target.thread == first.thread && target.turn == first.turn)
            }));
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.sent_interrupts.len()),
                1
            );
            draw(cx);
            let second = shell.read_with(cx, |shell, _| shell.stop_target().unwrap());
            shell.update_in(cx, |shell, window, cx| {
                shell.input.update(cx, |input, cx| input.focus(window, cx))
            });
            cx.simulate_keystrokes("ctrl-c");
            cx.run_until_parked();
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.sent_interrupts.last().unwrap().turn),
                second.turn
            );
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.sent_interrupts.len()),
                2
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn old_rendered_allow_button_does_not_approve_new_request(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let responses = shell.read_with(cx, |shell, _| shell.sent_responses.clone());
            shell.update(cx, |shell, cx| {
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        consent: Default::default(),
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: "touch request-a".into(),
                            cwd: Default::default(),
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
                };
                cx.notify();
            });
            draw(cx);
            let visible = shell.read_with(cx, |shell, _| shell.state.requests[0].id);
            let button = cx.debug_bounds("approval-allow-button").unwrap().center();
            cx.simulate_click(button, gpui::Modifiers::none());
            cx.run_until_parked();
            assert_eq!(
                responses.borrow().as_slice(),
                &[(visible, Decision::AllowOnce)]
            );
            eprintln!("unchanged-render control selected the displayed request");
            draw(cx);
            let button = cx.debug_bounds("approval-allow-button").unwrap().center();
            let next = RequestId::new();
            shell.update(cx, |shell, _| {
                let old_turn = shell.state.requests[0].turn;
                shell.state.requests[0].state = RequestState::Expired {
                    reason: "The turn ended".into(),
                };
                shell.state.turns.get_mut(&old_turn).unwrap().state = TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    started: Some(0),
                    at: 1,
                };
                let mut turn = shell.state.turns[&old_turn].clone();
                turn.id = TurnId::new();
                turn.ordinal += 1;
                turn.state = TurnState::Running { since: 1 };
                let mut request = shell.state.requests[0].clone();
                request.id = next;
                request.turn = turn.id;
                request.native = NativeRequestRef("request-b".into());
                request.state = RequestState::Pending;
                request.kind = RequestKind::Approval {
                    action: ProposedAction {
                        consent: Default::default(),
                        tool: "Bash".into(),
                        target: ApprovalTarget::Command {
                            command: "touch request-b".into(),
                            cwd: Default::default(),
                            reason: None,
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
                };
                shell.state.turns.insert(turn.id, turn);
                shell.state.requests.push(request);
            });
            responses.borrow_mut().clear();
            cx.simulate_click(button, gpui::Modifiers::none());
            cx.run_until_parked();
            let responses = responses.borrow().clone();
            eprintln!("visible={visible} next={next} selected={responses:?}");
            assert!(
                !responses.iter().any(|(request, _)| *request == next),
                "clicking A's rendered Allow once button approved B from a later turn"
            );
        });
    }
    #[gpui::test]
    fn approval_decisions_reject_stale_request_ids(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let responses = shell.read_with(cx, |shell, _| shell.sent_responses.clone());
            let (expired, pending) = shell.update(cx, |shell, cx| {
                for request in &mut shell.state.requests {
                    request.kind = RequestKind::Approval {
                        action: ProposedAction {
                            consent: Default::default(),
                            tool: "Bash".into(),
                            target: ApprovalTarget::Command {
                                command: "echo exact".into(),
                                cwd: Default::default(),
                                reason: None,
                                preview: ApprovalPreview::new(String::new()),
                            },
                        }
                        .with_grant(Some(GrantKey {
                            tool: "Bash".into(),
                            command: "echo exact".into(),
                        })),
                    };
                }
                shell.state.requests[0].state = RequestState::Expired {
                    reason: "The turn ended".into(),
                };
                cx.notify();
                (shell.state.requests[0].id, shell.state.requests[1].id)
            });
            draw(cx);
            for request in [expired, pending] {
                cx.dispatch_action(super::AllowOnce {
                    request: Some(request),
                });
                cx.run_until_parked();
                cx.dispatch_action(super::Deny {
                    request: Some(request),
                });
                cx.run_until_parked();
                cx.dispatch_action(super::AllowInThread {
                    request: Some(request),
                });
                cx.run_until_parked();
            }
            assert_eq!(
                responses.borrow().as_slice(),
                &[
                    (pending, Decision::AllowOnce),
                    (pending, Decision::Deny),
                    (pending, Decision::AllowInThread),
                ]
            );
        });
    }

    #[gpui::test]
    fn question_choice_keeps_request_and_question_identity(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            let responses = shell.read_with(cx, |shell, _| shell.sent_responses.clone());
            let request = shell.update(cx, |shell, cx| {
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Question {
                    questions: vec![
                        Question {
                            text: "Route".into(),
                            options: vec!["Short".into()],
                        },
                        Question {
                            text: "Next".into(),
                            options: vec!["Continue".into()],
                        },
                    ],
                };
                cx.notify();
                shell.state.requests[0].id
            });
            draw(cx);
            for (id, question) in [(request, 0), (request, 0), (RequestId::new(), 1)] {
                cx.dispatch_action(super::AnswerQuestion {
                    request: Some(id),
                    question: Some(question),
                    option: Some(0),
                });
                cx.run_until_parked();
            }
            assert!(responses.borrow().is_empty());
            assert_eq!(
                shell.read_with(cx, |shell, _| shell.question_draft.clone()),
                Some((
                    request,
                    vec![Answer {
                        text: "Short".into()
                    }]
                ))
            );
            cx.dispatch_action(super::AnswerQuestion {
                request: Some(request),
                question: Some(1),
                option: Some(0),
            });
            cx.run_until_parked();
            assert_eq!(
                responses.borrow().as_slice(),
                &[(
                    request,
                    Decision::Answers(vec![
                        Answer {
                            text: "Short".into()
                        },
                        Answer {
                            text: "Continue".into()
                        },
                    ])
                )]
            );
        });
    }
    #[test]
    fn text_rows_selects_only_open_thread_positions() {
        let thread = ThreadId::new();
        let other = ThreadId::new();
        let turn = TurnId::new();
        let assistant = Item {
            id: ItemId::new(),
            thread,
            turn,
            seq: 0,
            body: ItemBody::AssistantText {
                text: "Reply".into(),
                done: false,
            },
        };
        let reasoning = Item {
            id: ItemId::new(),
            thread,
            turn,
            seq: 1,
            body: ItemBody::Reasoning {
                text: "Thinking".into(),
                done: false,
            },
        };
        let elsewhere = Item {
            id: ItemId::new(),
            thread: other,
            turn: TurnId::new(),
            ..assistant.clone()
        };
        let mut state = AppState::default();
        for item in [&assistant, &reasoning, &elsewhere] {
            state.apply(UiEvent::Change {
                counter: None,
                event: ChangeEvent::ItemAdded(item.clone()),
            });
        }
        let text = |thread, item| UiEvent::TextAppended {
            thread,
            item,
            counter: 1,
            text: " more".into(),
            #[cfg(debug_assertions)]
            delta_count: 1,
        };
        assert_eq!(
            state.text_rows(
                &[
                    text(thread, reasoning.id),
                    text(thread, assistant.id),
                    text(thread, reasoning.id),
                    text(other, elsewhere.id),
                    text(thread, ItemId::new()),
                ],
                Some(thread),
            ),
            Some(BTreeSet::from([0, 1]))
        );
        assert_eq!(
            state.text_rows(&[text(other, elsewhere.id)], Some(thread)),
            Some(BTreeSet::new())
        );
        assert_eq!(
            state.text_rows(
                &[
                    text(thread, assistant.id),
                    UiEvent::Change {
                        counter: None,
                        event: ChangeEvent::ItemCompleted(assistant.clone()),
                    },
                ],
                Some(thread),
            ),
            None
        );
        assert_eq!(
            state.text_rows(&[text(thread, ItemId::new())], Some(thread)),
            Some(BTreeSet::new())
        );
    }
    #[test]
    fn family_rows_labels_and_delivery_read_the_projection() {
        let root = ThreadId::new();
        let child = ThreadId::new();
        let project = ProjectId::new();
        let mut state = AppState::default();
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ProjectAdded(Project {
                id: project,
                path: "/tmp".into(),
                name: "Project".into(),
            }),
        });
        let parent = Thread {
            id: root,
            project,
            title: "Parent".into(),
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
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ThreadCreated(Box::new(parent.clone())),
        });
        let turn = TurnId::new();
        let item = ItemId::new();
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::TurnStarted(Turn {
                id: turn,
                thread: root,
                ordinal: 1,
                origin: TurnOrigin::Provider,
                state: TurnState::Running { since: 1000 },
            }),
        });
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ItemAdded(Item {
                id: item,
                thread: root,
                turn,
                seq: 1,
                body: ItemBody::AssistantText {
                    text: "Root reply".into(),
                    done: false,
                },
            }),
        });
        assert_eq!(state.transcript_position(root, item), Some(0));
        let mut worker = parent;
        worker.id = child;
        worker.title = "Child".into();
        worker.lineage = Lineage::Child {
            parent: root,
            relationship: Relationship::Subagent,
            root,
        };
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ThreadCreated(Box::new(worker)),
        });
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ThreadStatusChanged {
                thread: root,
                status: Status::Done { took: 1 },
            },
        });
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ThreadStatusChanged {
                thread: child,
                status: Status::Working { since: 1000 },
            },
        });
        assert_eq!(super::sidebar::ordered_threads(&state), [root, child]);
        assert!(super::sidebar::nested(&state, &state.threads[&child]));
        assert!(super::sidebar::can_keep_children(&state, root));
        assert_eq!(state.family_status(root), Status::Working { since: 1000 });
        assert_eq!(state.transcript_position(root, item), Some(1));
        assert_eq!(state.transcript_rows(root), 2);
        let batch = vec![UiEvent::TextAppended {
            thread: root,
            item,
            text: " text".into(),
            counter: 1,
            #[cfg(debug_assertions)]
            delta_count: 1,
        }];
        assert_eq!(
            state.text_rows(&batch, Some(root)),
            Some(BTreeSet::from([1]))
        );
        let report_item = ItemId::new();
        let task = TaskId::new();
        let task_turn = TurnId::new();
        let wake = TurnId::new();
        let mut task = veyra_engine::domain::Task {
            id: task,
            parent: root,
            child,
            turn: task_turn,
            request_key: "start_thread:labels".into(),
            state: TaskState::Settled {
                outcome: TurnOutcome::Completed,
                summary: "report".into(),
                source: Some(report_item),
                delivery: Delivery::Pending { wake },
            },
        };
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::TaskSettled(task.clone()),
        });
        let origin = Turn {
            id: task_turn,
            thread: child,
            ordinal: 1,
            origin: TurnOrigin::Task { task: task.id },
            state: TurnState::Ended {
                outcome: TurnOutcome::Completed,
                started: Some(1000),
                at: 2000,
            },
        };
        assert_eq!(
            super::transcript::turn_label(&state, &origin),
            Some("Task from “Parent”".into())
        );
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::TurnEnded(origin.clone()),
        });
        let mut report = origin;
        report.id = wake;
        report.thread = root;
        report.origin = TurnOrigin::Wake {
            tasks: vec![task.id],
        };
        assert_eq!(
            super::transcript::turn_label(&state, &report),
            Some("Reports from 1 child thread".into())
        );
        let report_row = Item {
            id: report_item,
            thread: child,
            turn: task_turn,
            seq: 3,
            body: ItemBody::AssistantText {
                text: "report".into(),
                done: true,
            },
        };
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ItemAdded(report_row.clone()),
        });
        assert!(!super::transcript::report_delivered(&state, &report_row));
        task.state = TaskState::Settled {
            outcome: TurnOutcome::Completed,
            summary: "report".into(),
            source: Some(report_item),
            delivery: Delivery::Delivered { wake },
        };
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::TaskDelivered(task),
        });
        assert!(super::transcript::report_delivered(&state, &report_row));
        let mut followup = state.tasks.values().next().unwrap().clone();
        followup.id = TaskId::new();
        followup.turn = TurnId::new();
        followup.state = TaskState::Settled {
            outcome: TurnOutcome::Completed,
            summary: "new report".into(),
            source: None,
            delivery: Delivery::Pending { wake },
        };
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::TurnEnded(Turn {
                id: followup.turn,
                thread: child,
                ordinal: 2,
                origin: TurnOrigin::Task { task: followup.id },
                state: TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    started: Some(3000),
                    at: 4000,
                },
            }),
        });
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::TaskSettled(followup.clone()),
        });
        assert!(super::transcript::report_delivered(&state, &report_row));
        report.origin = TurnOrigin::Wake {
            tasks: state.tasks.keys().copied().collect(),
        };
        assert_eq!(
            super::transcript::turn_label(&state, &report),
            Some("Reports from 1 child thread".into())
        );
        followup.state = TaskState::Settled {
            outcome: TurnOutcome::Completed,
            summary: "new report".into(),
            source: None,
            delivery: Delivery::Delivered { wake },
        };
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::TaskDelivered(followup),
        });
        assert!(super::transcript::report_delivered(&state, &report_row));
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ThreadArchived { thread: root },
        });
        assert_eq!(super::sidebar::ordered_threads(&state), [child]);
        assert!(!super::sidebar::nested(&state, &state.threads[&child]));
        assert!(state.parent(child).unwrap().archived);
    }
    #[test]
    fn transcript_orders_started_turns_before_queued_turns() {
        let thread = ThreadId::new();
        let user = TurnId::new();
        let provider = TurnId::new();
        let wake = TurnId::new();
        let task = TurnId::new();
        let mut state = AppState::default();
        let mut items = Vec::new();
        for (id, ordinal, origin, turn_state, text) in [
            (
                user,
                1,
                TurnOrigin::User {
                    item: ItemId::new(),
                },
                TurnState::Queued,
                "queued user",
            ),
            (
                provider,
                2,
                TurnOrigin::Provider,
                TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    started: Some(2000),
                    at: 2500,
                },
                "provider reply first",
            ),
            (
                wake,
                3,
                TurnOrigin::Wake {
                    tasks: vec![TaskId::new()],
                },
                TurnState::Running { since: 4000 },
                "wake",
            ),
            (
                task,
                4,
                TurnOrigin::Task {
                    task: TaskId::new(),
                },
                TurnState::Queued,
                "queued task",
            ),
        ] {
            state.apply(UiEvent::Change {
                counter: None,
                event: ChangeEvent::TurnQueued(Turn {
                    id,
                    thread,
                    ordinal,
                    origin,
                    state: turn_state,
                }),
            });
            let item = Item {
                id: ItemId::new(),
                thread,
                turn: id,
                seq: 1,
                body: ItemBody::UserText(text.into()),
            };
            items.push(item.clone());
            state.apply(UiEvent::Change {
                counter: None,
                event: ChangeEvent::ItemAdded(item),
            });
        }
        let order = |state: &AppState| {
            state
                .transcript(thread)
                .iter()
                .map(|item| item.turn)
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&state), [provider, wake, user, task]);
        let mut admitted = state.turns[&user].clone();
        admitted.state = TurnState::Ended {
            outcome: TurnOutcome::Completed,
            started: Some(3000),
            at: 3500,
        };
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::TurnEnded(admitted),
        });
        assert_eq!(order(&state), [provider, user, wake, task]);
        let request = state.begin_snapshot(thread);
        state.finish_snapshot(
            thread,
            request,
            ThreadSnapshot {
                items,
                buffered_text: BTreeMap::new(),
                #[cfg(debug_assertions)]
                delta_counts: BTreeMap::new(),
                counter: 1,
            },
        );
        assert_eq!(order(&state), [provider, user, wake, task]);
    }
    #[test]
    fn transcript_cache_keeps_order_text_and_completion() {
        let thread = ThreadId::new();
        let first = TurnId::new();
        let last = TurnId::new();
        let mut state = AppState::default();
        for (id, ordinal) in [(first, 1), (last, 2)] {
            state.apply(UiEvent::Change {
                counter: None,
                event: ChangeEvent::TurnStarted(Turn {
                    id,
                    thread,
                    ordinal,
                    origin: TurnOrigin::Provider,
                    state: TurnState::Running { since: 1 },
                }),
            });
        }
        let old = Item {
            id: ItemId::new(),
            thread,
            turn: first,
            seq: 9,
            body: ItemBody::AssistantText {
                text: "Earlier".into(),
                done: true,
            },
        };
        let mut growing = Item {
            id: ItemId::new(),
            thread,
            turn: last,
            seq: 1,
            body: ItemBody::AssistantText {
                text: "New".into(),
                done: false,
            },
        };
        for item in [&old, &growing] {
            state.apply(UiEvent::Change {
                counter: None,
                event: ChangeEvent::ItemAdded(item.clone()),
            });
        }
        let reasoning = Item {
            id: ItemId::new(),
            seq: 2,
            body: ItemBody::Reasoning {
                text: "Thinking".into(),
                done: false,
            },
            ..growing.clone()
        };
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ItemAdded(reasoning.clone()),
        });
        assert_eq!(state.markdown_text[&growing.id].as_ref(), "New ▏");
        assert_eq!(state.markdown_text[&reasoning.id].as_ref(), "Thinking");
        let request = state.begin_snapshot(thread);
        assert!(state.finish_snapshot(
            thread,
            request,
            ThreadSnapshot {
                items: vec![growing.clone(), old.clone()],
                buffered_text: BTreeMap::new(),
                #[cfg(debug_assertions)]
                delta_counts: BTreeMap::new(),
                counter: 1,
            }
        ));
        assert_eq!(state.transcript_len(thread), 2);
        assert_eq!(state.transcript_item(thread, 0), Some(&old));
        assert_eq!(state.item_order[&thread].positions[&growing.id], 1);
        state.apply(UiEvent::TextAppended {
            thread,
            item: growing.id,
            counter: 2,
            text: " text".into(),
            #[cfg(debug_assertions)]
            delta_count: 1,
        });
        assert_eq!(state.markdown_text[&growing.id].as_ref(), "New text ▏");
        assert_eq!(state.markdown_text[&old.id].as_ref(), "Earlier");
        growing.body = ItemBody::AssistantText {
            text: "New text".into(),
            done: true,
        };
        state.apply(UiEvent::Change {
            counter: Some(3),
            event: ChangeEvent::ItemCompleted(growing.clone()),
        });
        assert_eq!(state.markdown_text[&growing.id].as_ref(), "New text");
        assert_eq!(state.transcript(thread), [old.clone(), growing.clone()]);
        state.apply(UiEvent::Change {
            counter: None,
            event: ChangeEvent::ThreadArchived { thread },
        });
        assert_eq!(state.transcript_len(thread), 0);
        assert!(!state.markdown_text.contains_key(&old.id));
        assert!(!state.markdown_text.contains_key(&growing.id));
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn command_digit_ignores_question_from_an_older_turn(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let thread = shell.view.chat().unwrap();
                let first = shell.state.turns.values().next().unwrap().id;
                let card = Item {
                    id: ItemId::new(),
                    thread,
                    turn: first,
                    seq: 1,
                    body: ItemBody::AsyncQuestions {
                        text: "Colour".into(),
                        questions: vec![AsyncQuestion {
                            title: "Colour".into(),
                            options: vec!["Violet".into()],
                            selected: None,
                        }],
                    },
                };
                shell
                    .state
                    .items
                    .insert(thread, BTreeMap::from([(card.id, card)]));
                let next = Turn {
                    id: TurnId::new(),
                    thread,
                    ordinal: 2,
                    origin: TurnOrigin::Provider,
                    state: TurnState::Running { since: 1 },
                };
                shell.state.turns.insert(next.id, next);
                shell.state.requests.clear();
                shell.notice = None;
                cx.notify();
            });
            draw(cx);
            cx.simulate_keystrokes("secondary-1");
            cx.run_until_parked();
            assert!(
                shell.read_with(cx, |shell, _| shell.notice.is_none()),
                "old card reached the closed test engine"
            );
            shell.update(cx, |shell, cx| {
                let first = shell
                    .state
                    .items
                    .values()
                    .next()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()
                    .turn;
                shell.state.turns.retain(|id, _| *id == first);
                cx.notify();
            });
            draw(cx);
            cx.simulate_keystrokes("secondary-1");
            cx.run_until_parked();
            assert_eq!(
                shell
                    .read_with(cx, |shell, _| shell.notice.clone())
                    .as_deref(),
                Some("engine is closed"),
                "a latest-turn card did not reach the test engine"
            );
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn codex_reason_has_its_own_element_below_the_consequence(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                shell.provider = ProviderKind::Codex;
                shell.providers[0].provider = ProviderKind::Codex;
                shell.state.requests.truncate(1);
                shell.state.requests[0].kind = RequestKind::Approval {
                    action: ProposedAction {
                        consent: Default::default(),
                        tool: "Command".into(),
                        target: ApprovalTarget::Command {
                            command: "echo safe".into(),
                            cwd: CommandCwd::ProjectFolder,
                            reason: Some(ApprovalPreview::new(
                                "Requested change\nVeyra checked this command\t\u{202e}OK".into(),
                            )),
                            preview: ApprovalPreview::new(String::new()),
                        },
                    },
                };
                cx.notify();
            });
            draw(cx);
            let consequence = cx.debug_bounds("approval-consequence").unwrap();
            let reason = cx.debug_bounds("approval-provider-reason").unwrap();
            assert!(reason.origin.y >= consequence.origin.y + consequence.size.height);
            assert_ne!(reason.size.height, gpui::Pixels::default());
        });
    }
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn terminal_window_layout_matrix(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let (thread, _) = configure_dock_thread(shell, ProviderKind::Claude, false, false);
                shell.state.requests.clear();
                shell.state.threads.get_mut(&thread).unwrap().view = View::Terminal;
                shell.state.statuses.insert(thread, Status::InTerminal);
                cx.notify();
            });
            let theme = cx.update(|_, cx| cx.global::<super::Theme>().clone());
            for viewport in [theme.layout.window_min, theme.layout.window_default] {
                cx.simulate_resize(viewport);
                draw(cx);
                let hint = cx.debug_bounds("terminal-hint").expect("terminal hint row");
                let grid = cx.debug_bounds("terminal-grid").expect("terminal grid");
                let header = cx
                    .debug_bounds("view-switch")
                    .expect("Chat and Terminal switch");
                assert_eq!(hint.size.height, theme.layout.terminal_hint);
                assert_eq!(hint.size.width, grid.size.width);
                assert_eq!(hint.top(), grid.bottom());
                assert!(header.right() <= viewport.width && header.top() < grid.top());
                assert_eq!(
                    shell.read_with(cx, |shell, cx| shell.dump(std::path::Path::new("/tmp"), cx))["view"],
                    "terminal"
                );
            }
        });
    }

    #[test]
    fn terminal_sidebar_uses_muted_open_arc() {
        let theme = super::Theme::new(super::Appearance::Dark);
        assert_eq!(super::sidebar::glyph(&Status::InTerminal), "◔");
        assert_eq!(
            super::sidebar::glyph_color(&Status::InTerminal, &theme),
            theme.colors.text_muted
        );
    }
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn terminal_header_buttons_dispatch_requested_view(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, dispatched, _| {
            shell.update(cx, |shell, cx| {
                configure_dock_thread(shell, ProviderKind::Claude, false, false);
                cx.notify();
            });
            draw(cx);
            for button in ["view-terminal", "view-chat"] {
                let bounds = cx.debug_bounds(button).expect("header view button");
                cx.simulate_click(bounds.center(), gpui::Modifiers::none());
                draw(cx);
            }
            assert_eq!(*dispatched.borrow(), ["Terminal", "Chat"]);
        });
    }
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn terminal_metadata_switches_surface_and_restores_composer_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::Focusable;
        with_docked_shell(cx, |shell, cx, _, events| {
            let mut row = shell.update(cx, |shell, cx| {
                let (thread, _) = configure_dock_thread(shell, ProviderKind::Claude, false, false);
                shell.state.requests.clear();
                cx.notify();
                shell.state.threads[&thread].clone()
            });
            row.view = View::Terminal;
            events
                .send(UiEvent::Change {
                    counter: Some(1),
                    event: ChangeEvent::ThreadUpdated(Box::new(row.clone())),
                })
                .unwrap();
            draw(cx);
            assert_eq!(
                shell.read_with(cx, |shell, cx| shell.dump(std::path::Path::new("/tmp"), cx))["view"],
                "terminal"
            );
            assert_eq!(
                shell.read_with(cx, |shell, cx| shell.dump(std::path::Path::new("/tmp"), cx))["terminal"]
                    ["focused"],
                true
            );
            row.view = View::Chat;
            row.native_cursor = Some("imported-native-turn".into());
            events
                .send(UiEvent::Change {
                    counter: Some(2),
                    event: ChangeEvent::ThreadUpdated(Box::new(row)),
                })
                .unwrap();
            draw(cx);
            let dump =
                shell.read_with(cx, |shell, cx| shell.dump(std::path::Path::new("/tmp"), cx));
            assert_eq!(dump["view"], "chat");
            assert!(dump["terminal"].is_null());
            assert_eq!(
                dump["selected_thread"]["native_cursor"],
                "imported-native-turn"
            );
            assert!(shell.update_in(cx, |shell, window, cx| {
                shell.input.focus_handle(cx).is_focused(window)
            }));
        });
    }
    #[cfg(debug_assertions)]
    #[gpui::test]
    fn open_terminal_uses_the_quit_sheet_and_command_new_thread(cx: &mut gpui::TestAppContext) {
        with_docked_shell(cx, |shell, cx, _, _| {
            shell.update(cx, |shell, cx| {
                let (thread, _) = configure_dock_thread(shell, ProviderKind::Claude, false, false);
                shell.state.requests.clear();
                shell.state.threads.get_mut(&thread).unwrap().view = View::Terminal;
                cx.notify();
            });
            draw(cx);
            assert!(shell.read_with(cx, |shell, cx| {
                shell.terminal.as_ref().unwrap().1.read(cx).dump()["focused"] == true
            }));
            cx.simulate_keystrokes("secondary-q");
            draw(cx);
            assert!(
                shell.read_with(cx, |shell, _| shell.quit_sheet),
                "open terminal bypassed quit confirmation"
            );
            shell.update_in(cx, |shell, window, cx| {
                shell.quit_sheet = false;
                use gpui::Focusable;
                window.focus(&shell.terminal.as_ref().unwrap().1.focus_handle(cx), cx);
                cx.notify();
            });
            draw(cx);
            cx.simulate_keystrokes("secondary-n");
            draw(cx);
            assert!(shell.read_with(cx, |shell, _| matches!(
                shell.view,
                super::MainView::NewThread
            )));
        });
    }

    #[cfg(debug_assertions)]
    #[gpui::test]
    fn cancel_quit_restores_the_visible_thread_surface(cx: &mut gpui::TestAppContext) {
        use gpui::Focusable;
        with_docked_shell(cx, |shell, cx, _, _| {
            struct Shutdown {
                engine: veyra_engine::Engine,
                runtime: tokio::runtime::Runtime,
            }
            impl Drop for Shutdown {
                fn drop(&mut self) {
                    self.runtime.block_on(self.engine.shutdown_wait());
                }
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let data = shell.read_with(cx, |shell, _| shell.probe_cwd.clone());
            let engine = veyra_engine::Engine::start_with_env(
                runtime.handle(),
                veyra_engine::data_dir::DataDir::at(data).unwrap(),
                veyra_engine::provider::env::ChildEnv::isolated(),
            )
            .unwrap();
            shell.update(cx, |shell, _| {
                shell.engine = engine.clone();
                shell.runtime = runtime.handle().clone();
            });
            let _shutdown = Shutdown { engine, runtime };
            for view in [View::Terminal, View::Chat] {
                shell.update(cx, |shell, cx| {
                    let (thread, _) =
                        configure_dock_thread(shell, ProviderKind::Claude, false, false);
                    shell.state.requests.clear();
                    shell.state.threads.get_mut(&thread).unwrap().view = view;
                    cx.notify();
                });
                draw(cx);
                shell.update_in(cx, |shell, window, cx| {
                    shell.quit_sheet = true;
                    window.focus(&shell.quit_focus, cx);
                    cx.notify();
                });
                draw(cx);
                cx.simulate_keystrokes("escape");
                draw(cx);
                assert!(!shell.read_with(cx, |shell, _| shell.quit_sheet));
                assert!(
                    shell.update_in(cx, |shell, window, cx| match view {
                        View::Terminal => shell
                            .terminal
                            .as_ref()
                            .unwrap()
                            .1
                            .focus_handle(cx)
                            .is_focused(window),
                        View::Chat => shell.input.focus_handle(cx).is_focused(window),
                    }),
                    "cancel did not restore {view:?} focus"
                );
            }
        });
    }

    #[test]
    fn terminal_spans_bound_transcript_order_independently_of_time() {
        for later_time in [0, 100, 200] {
            let thread = ThreadId::new();
            let imported = TurnId::new();
            let queued = TurnId::new();
            let mut state = AppState::default();
            for (turn, ordinal, at, body) in [
                (
                    queued,
                    2,
                    later_time,
                    ItemBody::UserText("new prompt".into()),
                ),
                (
                    imported,
                    1,
                    100,
                    ItemBody::TerminalSpan(TerminalSpan {
                        text: "terminal first".into(),
                    }),
                ),
            ] {
                state.apply(UiEvent::Change {
                    counter: None,
                    event: ChangeEvent::TurnEnded(Turn {
                        id: turn,
                        thread,
                        ordinal,
                        origin: TurnOrigin::Provider,
                        state: TurnState::Ended {
                            outcome: TurnOutcome::Completed,
                            started: Some(at),
                            at,
                        },
                    }),
                });
                state.apply(UiEvent::Change {
                    counter: None,
                    event: ChangeEvent::ItemAdded(Item {
                        id: ItemId::new(),
                        thread,
                        turn,
                        seq: ordinal,
                        body,
                    }),
                });
            }
            assert_eq!(
                state
                    .transcript(thread)
                    .iter()
                    .map(|item| item.turn)
                    .collect::<Vec<_>>(),
                [imported, queued]
            );
        }
    }
}
