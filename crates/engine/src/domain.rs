use crate::{live::LiveThread, provider::ProviderKind};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
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
    pub cli_version: Option<String>,
    pub hold: Option<HoldReason>,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub id: TurnId,
    pub thread: ThreadId,
    pub ordinal: u32,
    pub origin: TurnOrigin,
    pub state: TurnState,
}
impl Turn {
    pub fn recency(&self) -> (bool, Millis, u32) {
        (
            self.state == TurnState::Queued,
            self.state.started().unwrap_or(match self.state {
                TurnState::Ended { at, .. } => at,
                _ => i64::MAX,
            }),
            self.ordinal,
        )
    }
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
    Running {
        since: Millis,
    },
    Ended {
        outcome: TurnOutcome,
        started: Option<Millis>,
        at: Millis,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnOutcome {
    Completed,
    Interrupted { reason: String },
    Failed { message: String },
}
impl TurnOutcome {
    pub fn words(&self) -> String {
        match self {
            Self::Completed => "Done".into(),
            Self::Interrupted { reason } => reason.clone(),
            Self::Failed { message } => {
                format!("Failed · {}", message.lines().next().unwrap_or_default())
            }
        }
    }
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
    AssistantText {
        text: String,
        done: bool,
    },
    AsyncQuestions {
        text: String,
        questions: Vec<AsyncQuestion>,
    },
    Reasoning {
        text: String,
        done: bool,
    },
    Tool(ToolItem),
    Plan(Vec<PlanStep>),
    Notice(Notice),
    TaskCard(TaskId),
    TerminalSpan(TerminalSpan),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsyncQuestion {
    pub title: String,
    pub options: Vec<String>,
    pub selected: Option<usize>,
}
#[allow(clippy::disallowed_types)] // Provider wire arguments retain arbitrary JSON for display.
type ToolArguments = serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub veyra_label: bool,
    pub arguments: ToolArguments,
    #[serde(default)]
    pub input_summary: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolItem {
    pub call: ToolCall,
    pub result: Option<ToolResult>,
    #[serde(default)]
    pub output_tail: String,
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum RequestKind {
    Approval { action: ProposedAction },
    Question { questions: Vec<Question> },
}
pub enum AcceptedDecision<'a> {
    Allow {
        grant: Option<&'a GrantKey>,
    },
    Deny {
        reason: Option<&'a str>,
    },
    Answers {
        questions: &'a [Question],
        answers: &'a [Answer],
    },
}
impl RequestKind {
    pub fn grant_key(&self) -> Option<&GrantKey> {
        match self {
            Self::Approval { action } => action.consent.grant_key(),
            _ => None,
        }
    }
    pub fn accepts<'a>(&'a self, decision: &'a Decision) -> Option<AcceptedDecision<'a>> {
        match (self, decision) {
            (
                Self::Approval {
                    action:
                        ProposedAction {
                            consent: ApprovalConsent::Once { .. },
                            ..
                        },
                },
                Decision::AllowOnce,
            ) => Some(AcceptedDecision::Allow { grant: None }),
            (Self::Approval { action }, Decision::Deny) => Some(AcceptedDecision::Deny {
                reason: match &action.consent {
                    ApprovalConsent::DenyOnly { reason } => Some(reason),
                    ApprovalConsent::Once { .. } => None,
                },
            }),
            (
                Self::Approval {
                    action:
                        ProposedAction {
                            consent: ApprovalConsent::Once { grant: Some(key) },
                            ..
                        },
                },
                Decision::AllowInThread,
            ) => Some(AcceptedDecision::Allow { grant: Some(key) }),
            (Self::Question { questions }, Decision::Answers(answers))
                if questions.len() == answers.len() =>
            {
                Some(AcceptedDecision::Answers { questions, answers })
            }
            _ => None,
        }
    }
}
impl<'de> Deserialize<'de> for RequestKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        enum StoredRequest {
            Approval {
                action: Box<ProposedAction>,
                #[serde(default)]
                grant_key: Option<GrantKey>,
            },
            Question {
                questions: Vec<Question>,
            },
        }
        Ok(match StoredRequest::deserialize(deserializer)? {
            StoredRequest::Approval { action, grant_key } => {
                let mut action = *action;
                if let ApprovalConsent::Once { grant } = &mut action.consent {
                    *grant = grant.take().or(grant_key);
                }
                Self::Approval { action }
            }
            StoredRequest::Question { questions } => Self::Question { questions },
        })
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProposedAction {
    pub tool: String,
    pub target: ApprovalTarget,
    #[serde(skip_serializing_if = "ApprovalConsent::is_default")]
    pub consent: ApprovalConsent,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalConsent {
    Once { grant: Option<GrantKey> },
    DenyOnly { reason: String },
}
impl Default for ApprovalConsent {
    fn default() -> Self {
        Self::Once { grant: None }
    }
}
impl ApprovalConsent {
    pub fn grant_key(&self) -> Option<&GrantKey> {
        match self {
            Self::Once { grant } => grant.as_ref(),
            Self::DenyOnly { .. } => None,
        }
    }
    fn is_default(&self) -> bool {
        matches!(self, Self::Once { grant: None })
    }
}
impl ProposedAction {
    pub fn with_grant(mut self, key: Option<GrantKey>) -> Self {
        if let ApprovalConsent::Once { grant } = &mut self.consent {
            *grant = key;
        }
        self
    }
    pub fn new(tool: String, target: ApprovalTarget) -> Self {
        Self {
            tool,
            target,
            consent: ApprovalConsent::default(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApprovalTarget {
    Command {
        command: String,
        #[serde(default)]
        cwd: CommandCwd,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<ApprovalPreview>,
        preview: ApprovalPreview,
    },
    File {
        operation: FileOperation,
        path: PathBuf,
        location: TargetLocation,
        preview: ApprovalPreview,
    },
    Tool {
        preview: ApprovalPreview,
    },
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandCwd {
    #[default]
    ProjectFolder,
    Directory(PathBuf),
    NotVerified,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileOperation {
    Write,
    Edit,
    Read,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetLocation {
    InsideProject,
    OutsideProject,
    NotVerified,
}
pub const TOOL_OUTPUT_CHARS: usize = 10_000;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalPreview {
    pub text: String,
    pub truncated: bool,
}
impl ApprovalPreview {
    pub fn new(mut text: String) -> Self {
        let cut = text
            .char_indices()
            .nth(TOOL_OUTPUT_CHARS)
            .map(|(index, _)| index);
        if let Some(index) = cut {
            text.truncate(index);
        }
        Self {
            text,
            truncated: cut.is_some(),
        }
    }
}
impl<'de> Deserialize<'de> for ProposedAction {
    #[allow(clippy::disallowed_types)] // Legacy stored approval input is decoded only at this compatibility boundary.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StoredAction {
            Typed {
                tool: String,
                target: ApprovalTarget,
                #[serde(default)]
                consent: ApprovalConsent,
            },
            Legacy {
                tool: String,
                command: String,
                #[serde(default)]
                input: Option<serde_json::Value>,
            },
        }
        let (tool, target, consent) = match StoredAction::deserialize(deserializer)? {
            StoredAction::Typed {
                tool,
                target,
                consent,
            } => (tool, target, consent),
            StoredAction::Legacy {
                tool,
                command,
                input,
            } => {
                let target = if let Some(input) = input {
                    ApprovalTarget::Tool {
                        preview: ApprovalPreview::new(
                            serde_json::to_string_pretty(&input)
                                .map_err(serde::de::Error::custom)?,
                        ),
                    }
                } else {
                    ApprovalTarget::Command {
                        command,
                        cwd: Default::default(),
                        reason: None,
                        preview: ApprovalPreview::new(String::new()),
                    }
                };
                (tool, target, ApprovalConsent::default())
            }
        };
        Ok(Self {
            tool,
            target,
            consent,
        })
    }
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
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
        source: Option<ItemId>,
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
    pub turn_ids: BTreeMap<ThreadId, BTreeSet<TurnId>>,
    pub item_ids: BTreeMap<ThreadId, BTreeSet<ItemId>>,
    pub turns: BTreeMap<TurnId, Turn>,
    pub attempted_starts: BTreeMap<TurnId, Millis>,
    pub items: BTreeMap<ItemId, Item>,
    pub requests: BTreeMap<RequestId, Request>,
    pub grants: BTreeSet<(ThreadId, GrantKey)>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub settings: Settings,
}
impl Projection {
    pub fn insert_turn(&mut self, turn: Turn) {
        self.turn_ids
            .entry(turn.thread)
            .or_default()
            .insert(turn.id);
        self.turns.insert(turn.id, turn);
    }
    pub fn insert_item(&mut self, item: Item) {
        self.item_ids
            .entry(item.thread)
            .or_default()
            .insert(item.id);
        self.items.insert(item.id, item);
    }
    pub fn turns_for(&self, thread: ThreadId) -> impl Iterator<Item = &Turn> {
        self.turn_ids
            .get(&thread)
            .into_iter()
            .flatten()
            .filter_map(|id| self.turns.get(id))
    }
    pub fn items_for(&self, thread: ThreadId) -> impl Iterator<Item = &Item> {
        self.item_ids
            .get(&thread)
            .into_iter()
            .flatten()
            .filter_map(|id| self.items.get(id))
    }
}
impl TurnState {
    pub fn started(&self) -> Option<Millis> {
        match self {
            Self::Queued => None,
            Self::Running { since } => Some(*since),
            Self::Ended { started, .. } => *started,
        }
    }
}
pub struct ThreadView<'a> {
    pub thread: &'a Thread,
    pub live: &'a LiveThread,
    pub running: Option<&'a Turn>,
    pub queued: bool,
    pub pending_request: bool,
    pub last: Option<&'a Turn>,
}
impl ThreadView<'_> {
    pub fn turn_busy(&self) -> bool {
        self.running.is_some() || self.live.admission.is_some()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NeedsYou,
    Failed,
    Held,
    Interrupted,
    Working { since: Millis },
    Done { took: Millis },
    Idle,
}
impl Status {
    pub fn words(&self) -> &'static str {
        match self {
            Self::NeedsYou => "NeedsYou",
            Self::Failed => "Failed",
            Self::Held => "Held",
            Self::Interrupted => "Interrupted",
            Self::Working { .. } => "Working",
            Self::Done { .. } => "Done",
            Self::Idle => "Idle",
        }
    }
    fn priority(&self) -> u8 {
        match self {
            Self::NeedsYou => 0,
            Self::Failed => 1,
            Self::Held => 2,
            Self::Interrupted => 3,
            Self::Working { .. } => 4,
            Self::Done { .. } => 6,
            Self::Idle => 7,
        }
    }
}
pub fn thread_status(v: &ThreadView<'_>) -> Status {
    if v.pending_request {
        return Status::NeedsYou;
    }
    if (v.thread.hold == Some(HoldReason::Restarted) && v.thread.lineage == Lineage::Root)
        || (v.thread.hold.is_some() && v.queued)
    {
        return Status::Held;
    }
    if let Some(Turn {
        state: TurnState::Running { since },
        ..
    }) = v.running
    {
        return Status::Working { since: *since };
    }
    if v.live.admission.is_some()
        || v.live.background_work
        || matches!(v.live.live, crate::live::Live::Starting { .. })
    {
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
            started,
        }) => Status::Done {
            took: started.map(|start| at.saturating_sub(start)).unwrap_or(0),
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
    NoNativeSession,
    PendingRequest,
    Held(HoldReason),
    TurnRunning,
    Starting,
    BackgroundWork,
    Stopping,
    Archived,
}
impl std::fmt::Display for WaitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoNativeSession => {
                "Send a message first; the terminal opens the CLI's conversation."
            }
            Self::PendingRequest => "Answer the pending request before switching views",
            Self::Held(HoldReason::Interrupted) => "Held because you pressed Stop",
            Self::Held(HoldReason::Stopped) => "Held because this thread was stopped",
            Self::Held(HoldReason::Restarted) => "Held because Veyra closed while this ran",
            Self::Held(HoldReason::ChildReport) => "Held until you read a child thread's report",
            Self::TurnRunning => "Waits for the current turn",
            Self::Starting => "Waits for the CLI to start",
            Self::BackgroundWork => "Waits for background work",
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
    if v.turn_busy() {
        return Some(WaitReason::TurnRunning);
    }
    if matches!(v.live.live, Live::Starting { .. }) {
        return Some(WaitReason::Starting);
    }
    if v.live.background_work {
        return Some(WaitReason::BackgroundWork);
    }
    if matches!(
        v.live.live,
        Live::Stopping { .. }
            | Live::Structured {
                draining: Some(_),
                ..
            }
    ) {
        return Some(WaitReason::Stopping);
    }
    if v.thread.archived {
        return Some(WaitReason::Archived);
    }
    None
}

pub fn async_question_active<'a>(item: &Item, items: impl IntoIterator<Item = &'a Item>) -> bool {
    matches!(&item.body, ItemBody::AsyncQuestions { questions, .. } if questions.iter().any(|question|
        question.selected.is_none()))
        && !items.into_iter().any(|later| {
            later.thread == item.thread
                && later.seq > item.seq
                && matches!(later.body, ItemBody::UserText(_))
        })
}
