use crate::{live::LiveThread, provider::ProviderKind};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};
use uuid::Uuid;

pub type Millis = i64;
macro_rules! id {
    ($($name:ident),+) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);
        impl $name {
            pub fn new() -> Self { Self(Uuid::now_v7()) }
            pub fn derived(namespace: Uuid, key: &str) -> Self { Self(Uuid::new_v5(&namespace, key.as_bytes())) }
        }
        impl Default for $name { fn default() -> Self { Self::new() } }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { self.0.fmt(f) }
        }
    )+};
}
id!(ProjectId, ThreadId, TurnId, ItemId, RequestId, TaskId);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSessionId(pub String);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRequestRef(pub String);
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct NativeItemKey(pub String);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub path: PathBuf,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub project: ProjectId,
    pub title: String,
    pub lineage: Lineage,
    pub provider: ProviderKind,
    pub config: ThreadConfig,
    pub cwd: PathBuf,
    pub native: Option<NativeSessionId>,
    pub native_cursor: Option<String>,
    pub cli_version: Option<String>,
    pub hold: Option<HoldReason>,
    pub view: View,
    pub import_pending: bool,
    pub archived: bool,
}
impl Thread {
    pub fn root(&self) -> ThreadId {
        match self.lineage {
            Lineage::Root => self.id,
            Lineage::Child { root, .. } => root,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadConfig {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: PermissionMode,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    AskFirst,
    AcceptEdits,
    FullAccess,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    Interrupted,
    Stopped,
    Restarted,
    ChildReport,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lineage {
    Root,
    Child {
        parent: ThreadId,
        relationship: Relationship,
        root: ThreadId,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relationship {
    Subagent,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    Chat,
    Terminal,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub id: TurnId,
    pub thread: ThreadId,
    pub ordinal: u32,
    pub origin: TurnOrigin,
    pub state: TurnState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOrigin {
    User { item: ItemId },
    Task { task: TaskId },
    Wake { tasks: Vec<TaskId> },
    Provider,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnState {
    Queued,
    Running { since: Millis },
    Ended { outcome: TurnOutcome, at: Millis },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOutcome {
    Completed,
    Interrupted { reason: String },
    Failed { message: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub thread: ThreadId,
    pub turn: TurnId,
    pub seq: u32,
    pub body: ItemBody,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemBody {
    UserText(String),
    AssistantText { text: String, done: bool },
    Reasoning { text: String, done: bool },
    Tool(ToolItem),
    Plan(Vec<PlanStep>),
    Notice(Notice),
    TaskCard(TaskId),
    TerminalSpan(TerminalSpan),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolItem {
    pub call: ToolCall,
    pub result: Option<ToolResult>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub ok: bool,
    pub output_tail: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub text: String,
    pub done: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSpan {
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: RequestId,
    pub thread: ThreadId,
    pub turn: TurnId,
    pub native: NativeRequestRef,
    pub kind: RequestKind,
    pub state: RequestState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestKind {
    Approval {
        action: ProposedAction,
        grant_key: Option<GrantKey>,
    },
    Question {
        questions: Vec<Question>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedAction {
    pub tool: String,
    pub command: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub text: String,
    pub options: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub text: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestState {
    Pending,
    Answered(Decision),
    Expired { reason: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    AllowOnce,
    AllowInThread,
    Deny,
    Answers(Vec<Answer>),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantKey {
    pub tool: String,
    pub command: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub parent: ThreadId,
    pub child: ThreadId,
    pub turn: TurnId,
    pub request_key: String,
    pub state: TaskState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskState {
    Running,
    Settled {
        outcome: TurnOutcome,
        summary: String,
        delivery: Delivery,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Delivery {
    Pending { wake: TurnId },
    Delivered { wake: TurnId },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub default_mode: PermissionMode,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            default_mode: PermissionMode::FullAccess,
        }
    }
}
#[derive(Default)]
pub struct Projection {
    pub projects: BTreeMap<ProjectId, Project>,
    pub threads: BTreeMap<ThreadId, Thread>,
    pub turn_started: BTreeMap<TurnId, Millis>,
    pub turns: BTreeMap<TurnId, Turn>,
    pub items: BTreeMap<ItemId, Item>,
    pub requests: BTreeMap<RequestId, Request>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub settings: Settings,
}
pub struct ThreadView<'a> {
    pub thread: &'a Thread,
    pub live: &'a LiveThread,
    pub running: Option<&'a Turn>,
    pub queued: bool,
    pub pending_request: bool,
    pub last: Option<&'a Turn>,
    pub last_started: Option<Millis>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NeedsYou,
    Failed,
    Held,
    Interrupted,
    Working { since: Millis },
    InTerminal,
    Done { took: Millis },
    Idle,
}
impl Status {
    fn priority(&self) -> u8 {
        match self {
            Self::NeedsYou => 0,
            Self::Failed => 1,
            Self::Held => 2,
            Self::Interrupted => 3,
            Self::Working { .. } => 4,
            Self::InTerminal => 5,
            Self::Done { .. } => 6,
            Self::Idle => 7,
        }
    }
}
pub fn thread_status(v: &ThreadView<'_>) -> Status {
    if v.pending_request {
        return Status::NeedsYou;
    }
    if v.thread.hold.is_some() && v.queued {
        return Status::Held;
    }
    if let Some(Turn {
        state: TurnState::Running { since },
        ..
    }) = v.running
    {
        return Status::Working { since: *since };
    }
    if matches!(v.live.live, crate::live::Live::Terminal { .. }) {
        return Status::InTerminal;
    }
    if v.live.background_work || matches!(v.live.live, crate::live::Live::Starting { .. }) {
        return Status::Working { since: 0 };
    }
    match v.last.map(|t| &t.state) {
        Some(TurnState::Ended {
            outcome: TurnOutcome::Failed { .. },
            ..
        }) => Status::Failed,
        Some(TurnState::Ended {
            outcome: TurnOutcome::Interrupted { .. },
            ..
        }) => Status::Interrupted,
        Some(TurnState::Ended {
            outcome: TurnOutcome::Completed,
            at,
        }) => Status::Done {
            took: v
                .last_started
                .map(|start| at.saturating_sub(start))
                .unwrap_or(0),
        },
        _ => Status::Idle,
    }
}
pub fn family_status(root: &ThreadView<'_>, children: &[ThreadView<'_>]) -> Status {
    std::iter::once(thread_status(root))
        .chain(
            children
                .iter()
                .filter(|c| !c.thread.archived)
                .map(thread_status),
        )
        .min_by_key(Status::priority)
        .unwrap()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaitReason {
    Held(HoldReason),
    TurnRunning,
    Starting,
    BackgroundWork,
    TerminalView,
    Stopping,
    Archived,
}
impl std::fmt::Display for WaitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Held(HoldReason::Interrupted) => "Held because you pressed Stop",
            Self::Held(HoldReason::Stopped) => "Held because this thread was stopped",
            Self::Held(HoldReason::Restarted) => "Held because Veyra closed while this ran",
            Self::Held(HoldReason::ChildReport) => "Held until you read a child thread's report",
            Self::TurnRunning => "Waits for the current turn",
            Self::Starting => "Waits for the CLI to start",
            Self::BackgroundWork => "Waits for background work",
            Self::TerminalView => "Waits until you switch back to chat",
            Self::Stopping => "Waits for the CLI to stop",
            Self::Archived => "This thread is archived",
        })
    }
}
pub fn wait_reason(v: &ThreadView<'_>) -> Option<WaitReason> {
    use crate::live::Live;
    if let Some(reason) = v.thread.hold {
        return Some(WaitReason::Held(reason));
    }
    if v.running.is_some() {
        return Some(WaitReason::TurnRunning);
    }
    if matches!(v.live.live, Live::Starting { .. }) {
        return Some(WaitReason::Starting);
    }
    if v.live.background_work {
        return Some(WaitReason::BackgroundWork);
    }
    if v.thread.view == View::Terminal || matches!(v.live.live, Live::Terminal { .. }) {
        return Some(WaitReason::TerminalView);
    }
    if matches!(v.live.live, Live::Stopping { .. }) {
        return Some(WaitReason::Stopping);
    }
    if v.thread.archived {
        return Some(WaitReason::Archived);
    }
    None
}
