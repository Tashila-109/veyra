use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Scope {
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "method")]
pub(super) enum Notification {
    #[serde(rename = "turn/started")]
    TurnStarted {
        #[serde(default)]
        params: TurnNotification,
    },
    #[serde(rename = "turn/completed")]
    TurnCompleted {
        #[serde(default)]
        params: TurnNotification,
    },
    #[serde(rename = "item/agentMessage/delta")]
    AgentDelta {
        #[serde(default)]
        params: Delta,
    },
    #[serde(rename = "item/reasoning/summaryTextDelta")]
    ReasoningSummaryDelta {
        #[serde(default)]
        params: Delta,
    },
    #[serde(rename = "item/reasoning/textDelta")]
    ReasoningDelta {
        #[serde(default)]
        params: Delta,
    },
    #[serde(rename = "item/commandExecution/outputDelta")]
    CommandOutput {
        #[serde(default)]
        params: Delta,
    },
    #[serde(rename = "item/started")]
    ItemStarted {
        #[serde(default)]
        params: ItemNotification,
    },
    #[serde(rename = "item/completed")]
    ItemCompleted {
        #[serde(default)]
        params: ItemNotification,
    },
    #[serde(rename = "turn/plan/updated")]
    PlanUpdated {
        #[serde(default)]
        params: PlanNotification,
    },
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        params: ErrorNotification,
    },
    #[serde(rename = "account/rateLimits/updated")]
    RateLimits {
        #[serde(default)]
        params: RateLimitsNotification,
    },
    #[serde(other)]
    Unknown,
}
impl Notification {
    pub fn scope(&self) -> Option<&Scope> {
        match self {
            Self::TurnStarted { params: p } | Self::TurnCompleted { params: p } => Some(&p.scope),
            Self::AgentDelta { params: p }
            | Self::ReasoningSummaryDelta { params: p }
            | Self::ReasoningDelta { params: p }
            | Self::CommandOutput { params: p } => Some(&p.scope),
            Self::ItemStarted { params: p } | Self::ItemCompleted { params: p } => Some(&p.scope),
            Self::PlanUpdated { params: p } => Some(&p.scope),
            Self::Error { params: p } => Some(&p.scope),
            Self::RateLimits { params: _ } | Self::Unknown => None,
        }
    }
    pub fn turn_id(&self) -> Option<&str> {
        self.scope()?.turn_id.as_deref().or(match self {
            Self::TurnStarted { params: p } | Self::TurnCompleted { params: p } => {
                p.turn.id.as_deref()
            }
            _ => None,
        })
    }
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct TurnNotification {
    #[serde(flatten)]
    pub scope: Scope,
    pub turn: Turn,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Delta {
    #[serde(flatten)]
    pub scope: Scope,
    pub item_id: Option<String>,
    pub delta: Option<String>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct ItemNotification {
    #[serde(flatten)]
    pub scope: Scope,
    pub item: Item,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct PlanNotification {
    #[serde(flatten)]
    pub scope: Scope,
    pub plan: Vec<PlanStep>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct PlanStep {
    pub step: Option<String>,
    pub status: PlanStatus,
}
#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PlanStatus {
    Completed,
    #[default]
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ErrorNotification {
    #[serde(flatten)]
    pub scope: Scope,
    pub will_retry: bool,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct Turn {
    pub id: Option<String>,
    pub status: TurnStatus,
    pub error: Option<Failure>,
    pub items: Option<Vec<Item>>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum TurnStatus {
    InProgress,
    #[default]
    Completed,
    Interrupted,
    Failed,
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct Failure {
    pub message: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct TurnReply {
    pub turn: Turn,
}
#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct ThreadReply {
    pub thread: Thread,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Thread {
    pub id: Option<String>,
    pub cli_version: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct InitializeReply {
    pub user_agent: Option<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum Item {
    AgentMessage(AgentMessage),
    Reasoning(Reasoning),
    CommandExecution(CommandExecution),
    FileChange(FileChange),
    McpToolCall(McpToolCall),
    CollabAgentToolCall(CollabAgentCall),
    WebSearch(Tool),
    ImageView(Tool),
    #[default]
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct AgentMessage {
    pub id: Option<String>,
    pub text: Option<String>,
    pub delivery: Option<AgentDelivery>,
    pub questions: Option<Vec<AsyncQuestion>>,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AgentDelivery {
    Async,
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct AsyncQuestion {
    pub title: Option<String>,
    pub options: Option<Vec<String>>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct CollabAgentCall {
    #[serde(flatten)]
    pub tool_data: Tool,
    #[serde(rename = "tool")]
    pub action: Option<CollabAction>,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum CollabAction {
    SpawnAgent,
    SendInput,
    ResumeAgent,
    Wait,
    CloseAgent,
    SendMessage,
    FollowupTask,
    InterruptAgent,
    ListAgents,
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Reasoning {
    pub id: Option<String>,
    pub summary: Vec<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Tool {
    pub id: Option<String>,
    pub status: ToolStatus,
    pub aggregated_output: Option<String>,
    pub exit_code: Option<i64>,
    pub result: Option<Value>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum ToolStatus {
    InProgress,
    Completed,
    Failed,
    Declined,
    #[default]
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct CommandExecution {
    #[serde(flatten)]
    pub tool: Tool,
    pub command: Option<String>,
    pub cwd: Option<String>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct FileChange {
    #[serde(flatten)]
    pub tool: Tool,
    pub changes: Option<Value>,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct McpToolCall {
    #[serde(flatten)]
    pub tool: Tool,
    pub server: Option<String>,
    #[serde(rename = "tool")]
    pub tool_name: Option<String>,
    pub arguments: Option<Value>,
}
impl Item {
    pub fn id(&self) -> Option<&str> {
        match self {
            Self::AgentMessage(p) => p.id.as_deref(),
            Self::Reasoning(p) => p.id.as_deref(),
            Self::CommandExecution(p) => p.tool.id.as_deref(),
            Self::FileChange(p) => p.tool.id.as_deref(),
            Self::McpToolCall(p) => p.tool.id.as_deref(),
            Self::CollabAgentToolCall(p) => p.tool_data.id.as_deref(),
            Self::WebSearch(p) | Self::ImageView(p) => p.id.as_deref(),
            Self::Unknown => None,
        }
    }
    pub fn tool(&self) -> Option<&Tool> {
        match self {
            Self::CommandExecution(p) => Some(&p.tool),
            Self::FileChange(p) => Some(&p.tool),
            Self::McpToolCall(p) => Some(&p.tool),
            Self::CollabAgentToolCall(p) => Some(&p.tool_data),
            Self::WebSearch(p) | Self::ImageView(p) => Some(p),
            _ => None,
        }
    }
}
#[derive(Clone, Deserialize)]
#[serde(tag = "method")]
pub(super) enum Request {
    #[serde(rename = "item/commandExecution/requestApproval")]
    Command {
        #[serde(default)]
        params: CommandApproval,
    },
    #[serde(rename = "item/fileChange/requestApproval")]
    File {
        #[serde(default)]
        params: FileApproval,
    },
    #[serde(rename = "item/permissions/requestApproval")]
    Permissions {
        #[serde(default)]
        params: PermissionsApproval,
    },
    #[serde(rename = "item/tool/requestUserInput")]
    UserInput {
        #[serde(default)]
        params: UserInput,
    },
    #[serde(rename = "mcpServer/elicitation/request")]
    Elicitation {
        #[serde(default)]
        params: Elicitation,
    },
    #[serde(other)]
    Unknown,
}
impl Request {
    pub fn scope(&self) -> Option<&Scope> {
        match self {
            Self::Command { params: p } => Some(&p.scope),
            Self::File { params: p } => Some(&p.scope),
            Self::Permissions { params: p } => Some(&p.scope),
            Self::UserInput { params: p } => Some(&p.scope),
            Self::Elicitation { params: p } => Some(&p.scope),
            Self::Unknown => None,
        }
    }
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct CommandApproval {
    #[serde(flatten)]
    pub scope: Scope,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub reason: Option<String>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct FileApproval {
    #[serde(flatten)]
    pub scope: Scope,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct PermissionsApproval {
    #[serde(flatten)]
    pub scope: Scope,
    pub permissions: Option<BTreeMap<String, Value>>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct UserInput {
    #[serde(flatten)]
    pub scope: Scope,
    pub questions: Option<Vec<Question>>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct Question {
    pub id: Option<String>,
    pub question: Option<String>,
    pub options: Option<Vec<QuestionOption>>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct QuestionOption {
    pub label: Option<String>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Elicitation {
    #[serde(flatten)]
    pub scope: Scope,
    pub server_name: Option<String>,
    pub message: Option<String>,
    #[serde(rename = "_meta")]
    pub meta: Option<ElicitationMeta>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct ElicitationMeta {
    pub codex_approval_kind: Option<ApprovalKind>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ApprovalKind {
    McpToolCall,
    #[serde(other)]
    Unknown,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct AccountReply {
    pub account: Account,
    #[serde(deserialize_with = "optional_bool")]
    pub requires_openai_auth: bool,
}
#[derive(Default)]
pub(super) enum Account {
    #[default]
    Missing,
    Null,
    Known(Value),
}
impl<'de> Deserialize<'de> for Account {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        Ok(if value.is_null() {
            Self::Null
        } else {
            Self::Known(value)
        })
    }
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct ModelsReply {
    pub data: Vec<Model>,
    pub next_cursor: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Model {
    pub id: Option<String>,
    pub model: Option<String>,
    pub display_name: Option<String>,
    pub supported_reasoning_efforts: Vec<Effort>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct Effort {
    pub reasoning_effort: Option<String>,
}

fn optional_bool<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<bool, D::Error> {
    Ok(matches!(
        Value::deserialize(deserializer)?,
        Value::Bool(true)
    ))
}

#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct RateLimitsNotification {
    pub rate_limits: RateLimits,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct RateLimits {
    pub primary: Option<RateWindow>,
    pub secondary: Option<RateWindow>,
    pub rate_limit_reached_type: Option<RateLimitReachedType>,
    pub spend_control_reached: Option<bool>,
    pub individual_limit: Option<SpendLimit>,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RateLimitReachedType {
    RateLimitReached,
    WorkspaceOwnerCreditsDepleted,
    WorkspaceMemberCreditsDepleted,
    WorkspaceOwnerUsageLimitReached,
    WorkspaceMemberUsageLimitReached,
    #[serde(other)]
    Unknown,
}
impl RateLimitReachedType {
    pub fn description(self) -> Option<&'static str> {
        match self {
            Self::RateLimitReached => Some("its usage limit"),
            Self::WorkspaceOwnerCreditsDepleted => Some("the workspace owner's credit limit"),
            Self::WorkspaceMemberCreditsDepleted => Some("the workspace member's credit limit"),
            Self::WorkspaceOwnerUsageLimitReached => Some("the workspace owner's usage limit"),
            Self::WorkspaceMemberUsageLimitReached => Some("the workspace member's usage limit"),
            Self::Unknown => None,
        }
    }
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct SpendLimit {
    pub resets_at: Option<i64>,
}
#[derive(Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(super) struct RateWindow {
    pub used_percent: f64,
    pub window_duration_mins: Option<i64>,
    pub resets_at: Option<i64>,
}
