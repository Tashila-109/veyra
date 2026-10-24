#![allow(clippy::disallowed_types)] // ACP envelopes and tolerant catalog fields are provider wire JSON.
use super::{
    ActiveTurn, Decoded, Outbound, ProviderError, ProviderEvent, SessionEvent, SessionSpec,
    StopReason, TurnEvent,
    jsonrpc::{self, Frame, Peer},
};
use crate::{domain::*, mcp::McpConfig, platform::Signal};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::time::Instant;

pub fn args(config: &ThreadConfig) -> Vec<String> {
    let mut args = vec!["--no-auto-update".into()];
    match config.mode {
        PermissionMode::AskFirst | PermissionMode::AcceptEdits => {
            args.extend(["--permission-mode".into(), "default".into(), "agent".into()])
        }
        PermissionMode::FullAccess => args.extend(["agent".into(), "--always-approve".into()]),
    }
    if let Some(model) = &config.model {
        args.extend(["-m".into(), model.clone()]);
    }
    if let Some(effort) = &config.effort {
        args.extend(["--reasoning-effort".into(), effort.clone()]);
    }
    args.extend(["--no-leader".into(), "stdio".into()]);
    args
}
pub fn initialize() -> Value {
    json!({"protocolVersion":1,"clientInfo":{"name":"veyra","version":env!("CARGO_PKG_VERSION")},
        "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
        "_meta":{"clientType":"extension"}})
}
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Initialization {
    auth_methods: Vec<AuthMethod>,
    agent_capabilities: Capabilities,
    agent_info: AgentInfo,
    #[serde(rename = "_meta")]
    meta: Value,
}
impl Initialization {
    fn version(&self) -> Option<String> {
        self.agent_info
            .version
            .as_ref()
            .filter(|v| !v.is_empty())
            .map(String::as_str)
            .or(self.meta["agentVersion"].as_str().filter(|v| !v.is_empty()))
            .map(str::to_owned)
    }
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct AuthMethod {
    id: String,
}
#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Capabilities {
    session_capabilities: SessionCapabilities,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct SessionCapabilities {
    resume: Option<Value>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct AgentInfo {
    version: Option<String>,
}
#[derive(Clone)]
enum Call {
    Initialize,
    Authenticate,
    Open,
    Prompt(String),
}
enum PromptOrigin {
    Veyra(TurnId),
    Foreign,
}
enum PromptWork {
    Veyra { id: String, turn: TurnId },
    Provider { turn: TurnId },
}
fn tool_key(owner: ActiveTurn, prompt: &str, tool: &str) -> String {
    format!("{}/{prompt}/tool/{tool}", owner.target())
}
impl PromptWork {
    fn owner(&self) -> ActiveTurn {
        match self {
            Self::Veyra { turn, .. } => ActiveTurn::Veyra(*turn),
            Self::Provider { turn } => ActiveTurn::Provider(*turn),
        }
    }
}
#[derive(Default)]
struct ReplySegment {
    number: u32,
    open: bool,
    seen: bool,
}
impl ReplySegment {
    fn key(&self, prompt: &str) -> NativeItemKey {
        NativeItemKey(if self.number == 0 {
            format!("{prompt}/reply")
        } else {
            format!("{prompt}/reply/{}", self.number)
        })
    }
    fn next(&mut self, prompt: &str) -> NativeItemKey {
        if !self.open && self.seen {
            self.number += 1;
        }
        self.open = true;
        self.seen = true;
        self.key(prompt)
    }
    fn close(&mut self, prompt: &str) -> Option<NativeItemKey> {
        std::mem::take(&mut self.open).then(|| self.key(prompt))
    }
}
struct Prompt {
    origin: PromptOrigin,
    accepted: bool,
    first_frame: Option<Instant>,
    completion: Option<PromptCompletion>,
    tool_starts: BTreeMap<String, ToolCall>,
    reply_segment: ReplySegment,
}
struct PromptCompletion {
    deadline: Instant,
    outcome: TurnOutcome,
}
enum PermissionReply {
    Permission {
        allow: Option<String>,
        reject: Option<String>,
    },
    Question,
}
struct ToolState {
    title: String,
    output: String,
    ended: bool,
}
struct Request {
    id: Value,
    kind: RequestKind,
    reply: PermissionReply,
    owner: ActiveTurn,
}
#[derive(Default, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default, rename_all = "camelCase")]
struct Permission {
    session_id: String,
    tool_call: Tool,
    options: Vec<PermissionOption>,
    #[serde(rename = "_meta")]
    meta: Meta,
}
#[derive(Default, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default, rename_all = "camelCase")]
struct PermissionOption {
    option_id: String,
    kind: OptionKind,
}
#[derive(Default, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default, rename_all = "camelCase")]
struct Meta {
    prompt_index: Option<i64>,
    prompt_id: Option<String>,
    is_replay: bool,
}
#[derive(Default, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default, rename_all = "camelCase")]
struct Tool {
    tool_call_id: String,
    title: String,
    status: ToolStatus,
    raw_input: Value,
    raw_output: Value,
    content: Vec<Value>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct QuestionParams {
    session_id: String,
    tool_call_id: String,
    questions: Vec<WireQuestion>,
    #[serde(rename = "_meta")]
    meta: Meta,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PromptScope {
    session_id: String,
    prompt_id: Option<String>,
    #[serde(rename = "_meta")]
    meta: Meta,
}
impl PromptScope {
    fn prompt_id(&self) -> Option<&str> {
        self.meta.prompt_id.as_deref().or(self.prompt_id.as_deref())
    }
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct TaggedFrame {
    params: PromptScope,
    result: PromptScope,
    #[serde(flatten)]
    scope: PromptScope,
}
impl TaggedFrame {
    fn prompt_id(&self) -> Option<&str> {
        self.params
            .prompt_id()
            .or(self.scope.prompt_id())
            .or(self.result.prompt_id())
    }
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct OpenReply {
    session_id: Option<String>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct TaskUpdate {
    task_id: String,
    task_snapshot: Option<TaskSnapshot>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct TaskSnapshot {
    task_id: String,
}
#[derive(Deserialize)]
struct TurnCompleted {
    #[serde(rename = "sessionUpdate")]
    update: String,
    prompt_id: Option<String>,
    #[serde(flatten)]
    completion: Completion,
}
#[derive(Deserialize)]
struct UpdateHeader {
    #[serde(rename = "sessionUpdate")]
    kind: UpdateKind,
}
#[derive(Deserialize)]
struct PlanUpdate {
    entries: Vec<Value>,
}
#[derive(Deserialize)]
struct PlanEntry {
    content: String,
    #[serde(default, deserialize_with = "default_on_invalid")]
    status: ToolStatus,
}
enum Update {
    Message(Content),
    Thought(Content),
    Tool(Box<Tool>),
    Plan(PlanUpdate),
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct WireQuestion {
    question: String,
    options: Vec<QuestionOption>,
}
#[derive(Default, Deserialize)]
#[serde(default)]
struct QuestionOption {
    label: String,
}

#[derive(Deserialize, PartialEq, Eq)]
enum NotificationMethod {
    #[serde(rename = "session/update")]
    Update,
    #[serde(rename = "x.ai/task_backgrounded", alias = "_x.ai/task_backgrounded")]
    TaskBackgrounded,
    #[serde(rename = "x.ai/task_completed", alias = "_x.ai/task_completed")]
    TaskCompleted,
    #[serde(
        rename = "x.ai/session/prompt_complete",
        alias = "_x.ai/session/prompt_complete"
    )]
    PromptComplete,
    #[serde(
        rename = "_x.ai/session_notification",
        alias = "x.ai/session_notification"
    )]
    SessionNotification,
    #[serde(other)]
    Unknown,
}
#[derive(Deserialize)]
enum RequestMethod {
    #[serde(rename = "session/request_permission")]
    Permission,
    #[serde(rename = "x.ai/ask_user_question", alias = "_x.ai/ask_user_question")]
    Question,
    #[serde(other)]
    Unknown,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Content {
    Text {
        text: String,
    },
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(rename_all = "snake_case")]
enum OptionKind {
    AllowOnce,
    RejectOnce,
    AllowAlways,
    RejectAlways,
    #[default]
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UpdateKind {
    UserMessageChunk,
    TurnCompleted,
    AgentMessageChunk,
    AgentThoughtChunk,
    ToolCall,
    ToolCallUpdate,
    Plan,
    #[serde(other)]
    Unknown,
}
#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(rename_all = "snake_case")]
enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    #[default]
    #[serde(other)]
    Unknown,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CompletionReason {
    #[default]
    EndTurn,
    Cancelled,
    Error,
    Refusal,
    RateLimit,
    #[serde(other)]
    Unknown,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Completion {
    #[serde(alias = "stop_reason", deserialize_with = "default_on_invalid")]
    stop_reason: CompletionReason,
    #[serde(alias = "agent_result")]
    agent_result: Value,
    cancellation_category: Value,
}
#[derive(Clone)]
enum OpenTarget {
    New,
    Resume(NativeSessionId),
    Load(NativeSessionId),
}
#[derive(Clone, Copy)]
enum AuthenticationRetry {
    Available,
    Used,
}
enum SessionState {
    Initializing,
    Authenticating {
        target: OpenTarget,
        retry: AuthenticationRetry,
    },
    Opening {
        target: OpenTarget,
        retry: AuthenticationRetry,
    },
    Ready {
        native: NativeSessionId,
        active: Option<PromptWork>,
    },
}

pub(crate) struct Codec {
    peer: Peer<Call>,
    spec: SessionSpec,
    pid: u32,
    mcp: Option<Arc<McpConfig>>,
    state: SessionState,
    api_key_set: bool,
    version: Option<String>,
    auth: Option<String>,
    prompts: BTreeMap<String, Prompt>,
    ended: BTreeSet<String>,
    requests: BTreeMap<String, Request>,
    seen_requests: BTreeSet<String>,
    tasks: BTreeSet<String>,
    tools: BTreeMap<String, ToolState>,
    interrupted: Option<super::StopLadder>,
}
impl Codec {
    pub(crate) fn new(
        spec: SessionSpec,
        pid: u32,
        mcp: Option<Arc<McpConfig>>,
        api_key_set: bool,
    ) -> Self {
        Self {
            peer: Peer::new(true),
            spec,
            pid,
            mcp,
            state: SessionState::Initializing,
            api_key_set,
            version: None,
            auth: None,
            prompts: BTreeMap::new(),
            ended: BTreeSet::new(),
            requests: BTreeMap::new(),
            seen_requests: BTreeSet::new(),
            tasks: BTreeSet::new(),
            tools: BTreeMap::new(),
            interrupted: None,
        }
    }
    pub(crate) fn native(&self) -> Option<&NativeSessionId> {
        match &self.state {
            SessionState::Ready { native, .. } => Some(native),
            _ => None,
        }
    }
    pub(crate) fn initialize(&mut self) -> Value {
        self.peer.send(Call::Initialize, "initialize", initialize())
    }
    fn work(&self) -> Option<&PromptWork> {
        match &self.state {
            SessionState::Ready { active, .. } => active.as_ref(),
            _ => None,
        }
    }
    fn set_work(&mut self, work: Option<PromptWork>) {
        if let SessionState::Ready { active, .. } = &mut self.state {
            *active = work;
        }
    }
    pub(crate) fn active(&self) -> Option<ActiveTurn> {
        self.work().map(PromptWork::owner)
    }
    fn begin_provider(&mut self) -> TurnId {
        let turn = TurnId::new();
        self.set_work(Some(PromptWork::Provider { turn }));
        turn
    }
    fn open(
        &mut self,
        target: OpenTarget,
        retry: AuthenticationRetry,
    ) -> Result<Outbound, ProviderError> {
        let (method, native) = match &target {
            OpenTarget::New => ("session/new", None),
            OpenTarget::Resume(native) => ("session/resume", Some(native)),
            OpenTarget::Load(native) => ("session/load", Some(native)),
        };
        let mut params = json!({"cwd":self.spec.cwd,"mcpServers":[]});
        if self.spec.config.mode != PermissionMode::FullAccess {
            params["_meta"] = json!({"yoloMode":false,"autoMode":false});
        }
        if let Some(native) = native {
            params["sessionId"] = json!(native.0);
        }
        self.state = SessionState::Opening { target, retry };
        let line = self.peer.send(Call::Open, method, params);
        if let Some(mcp) = &self.mcp {
            mcp.frame(line)
        } else {
            Ok(Outbound::Message(line))
        }
    }
    fn authenticate(
        &mut self,
        target: OpenTarget,
        retry: AuthenticationRetry,
    ) -> Result<Value, ProviderError> {
        let method = self.auth.as_ref().ok_or_else(signed_out)?;
        self.state = SessionState::Authenticating { target, retry };
        Ok(self.peer.send(
            Call::Authenticate,
            "authenticate",
            json!({"methodId":method}),
        ))
    }
    pub(crate) fn start(&mut self, turn: TurnId, text: String) -> Option<Decoded> {
        if !self.prompts.is_empty() {
            return None;
        }
        let native = self.native()?.clone();
        let id = turn.to_string();
        self.prompts.insert(
            id.clone(),
            Prompt {
                origin: PromptOrigin::Veyra(turn),
                accepted: false,
                first_frame: Some(Instant::now() + Duration::from_secs(60)),
                completion: None,
                tool_starts: BTreeMap::new(),
                reply_segment: ReplySegment::default(),
            },
        );
        self.set_work(Some(PromptWork::Veyra {
            id: id.clone(),
            turn,
        }));
        self.interrupted = None;
        Some(Decoded {

            events: vec![ProviderEvent::Session(SessionEvent::Dispatched { turn })],
            controls: vec![Outbound::Prompt(self.peer.send(Call::Prompt(id.clone()),"session/prompt",
            json!({"sessionId":native.0,"prompt":[{"type":"text","text":text}],"_meta":{"promptId":id}})))],
            ..Default::default()
        })
    }
    fn event(decoded: &mut Decoded, owner: ActiveTurn, event: TurnEvent) {
        decoded.events.push(ProviderEvent::Turn {
            turn: owner.id(),
            event,
        });
    }
    fn prompt(&mut self, id: Option<&str>, decoded: &mut Decoded) -> Option<String> {
        let id = id.map(str::to_owned).or_else(|| match self.work() {
            Some(PromptWork::Veyra { id, .. }) => Some(id.clone()),
            _ => None,
        })?;
        if self.ended.contains(&id) {
            return None;
        }
        self.prompts.entry(id.clone()).or_insert(Prompt {
            origin: PromptOrigin::Foreign,
            accepted: true,
            first_frame: None,
            completion: None,
            tool_starts: BTreeMap::new(),
            reply_segment: ReplySegment::default(),
        });
        if self.work().is_none() {
            let turn = self.begin_provider();
            decoded.provider_turn = Some(turn);
            Self::event(decoded, ActiveTurn::Provider(turn), TurnEvent::Started);
        }
        Some(id)
    }
    fn finish(&mut self, id: &str, outcome: TurnOutcome, accepted: bool, decoded: &mut Decoded) {
        let Some(prompt) = self.prompts.remove(id) else {
            return;
        };
        self.ended.insert(id.into());
        let owner = match prompt.origin {
            PromptOrigin::Veyra(turn) => {
                if accepted && !prompt.accepted {
                    Self::event(decoded, ActiveTurn::Veyra(turn), TurnEvent::Accepted);
                }
                ActiveTurn::Veyra(turn)
            }
            PromptOrigin::Foreign => {
                if !self.prompts.is_empty()
                    || !matches!(self.work(), Some(PromptWork::Provider { .. }))
                {
                    return;
                }
                self.active().unwrap()
            }
        };
        let requests: Vec<_> = self
            .requests
            .iter()
            .filter(|(_, request)| request.owner.target() == owner.target())
            .map(|(key, _)| key.clone())
            .collect();
        for key in requests {
            let request = self.requests.remove(&key).unwrap();
            decoded
                .replies
                .push(self.peer.response(request.id, cancelled(&request.reply)));
        }
        Self::event(
            decoded,
            owner,
            TurnEvent::TurnEnded {
                outcome: self.end_outcome(outcome),
                park: false,
            },
        );
        self.set_work(None);
        if self.prompts.is_empty() {
            self.interrupted = None;
        } else {
            let turn = self.begin_provider();
            decoded
                .events
                .push(ProviderEvent::Session(SessionEvent::ProviderTurnStarted {
                    provider_turn: turn,
                }));
            Self::event(decoded, ActiveTurn::Provider(turn), TurnEvent::Started);
        }
    }
    pub(crate) fn value(&mut self, value: Value) -> Result<Decoded, ProviderError> {
        for prompt in self.prompts.values_mut() {
            prompt.first_frame = None;
        }
        let mut decoded = Decoded::default();
        let tagged: Option<TaggedFrame> = serde_json::from_value(value.clone()).ok();
        let frame = self.peer.receive(value);
        if matches!(frame, Frame::Notification { .. })
            && tagged.as_ref().is_some_and(|tagged| {
                tagged.params.meta.is_replay
                    || (!tagged.params.session_id.is_empty()
                        && self
                            .native()
                            .is_none_or(|native| native.0 != tagged.params.session_id))
            })
        {
            return Ok(decoded);
        }
        if let Some(prompt) = tagged
            .as_ref()
            .and_then(TaggedFrame::prompt_id)
            .and_then(|id| self.prompts.get_mut(id))
            && let PromptOrigin::Veyra(turn) = prompt.origin
            && !prompt.accepted
        {
            prompt.accepted = true;
            Self::event(&mut decoded, ActiveTurn::Veyra(turn), TurnEvent::Accepted);
        }
        match frame {
            Frame::Reply {
                tag: Call::Initialize,
                result,
            } => {
                let init: Initialization = decode(result.map_err(jsonrpc::failure)?)?;
                self.version = init.version();
                #[cfg(debug_assertions)]
                {
                    decoded.recording = Some(crate::record::Metadata {
                        native: None,
                        version: self.version.clone(),
                    });
                }
                let target = match &self.spec.native {
                    None => OpenTarget::New,
                    Some(native)
                        if init
                            .agent_capabilities
                            .session_capabilities
                            .resume
                            .is_some_and(|v| v != false && !v.is_null()) =>
                    {
                        OpenTarget::Resume(native.clone())
                    }
                    Some(native) => OpenTarget::Load(native.clone()),
                };
                self.auth = auth_method(&init.auth_methods, self.api_key_set);
                if self.auth.is_some() {
                    decoded
                        .replies
                        .push(self.authenticate(target, AuthenticationRetry::Available)?);
                } else {
                    decoded
                        .controls
                        .push(self.open(target, AuthenticationRetry::Available)?);
                }
            }
            Frame::Reply {
                tag: Call::Authenticate,
                result,
            } => {
                result.map_err(|_| signed_out())?;
                if let SessionState::Authenticating { target, retry } = &self.state {
                    decoded.controls.push(self.open(target.clone(), *retry)?);
                }
            }
            Frame::Reply {
                tag: Call::Open,
                result,
            } => {
                let SessionState::Opening { target, retry } = &self.state else {
                    return Err(ProviderError::Protocol(
                        "Unexpected Grok session reply".into(),
                    ));
                };
                let target = target.clone();
                let retry = *retry;
                match result {
                    Ok(result) => {
                        let native = decode::<OpenReply>(result)?
                            .session_id
                            .filter(|s| !s.is_empty())
                            .or_else(|| match &target {
                                OpenTarget::Resume(n) | OpenTarget::Load(n) => Some(n.0.clone()),
                                OpenTarget::New => None,
                            })
                            .ok_or_else(|| {
                                ProviderError::Protocol(
                                    "Grok session reply has no sessionId".into(),
                                )
                            })?;
                        self.state = SessionState::Ready {
                            native: NativeSessionId(native.clone()),
                            active: None,
                        };
                        if self.spec.config.mode != PermissionMode::FullAccess {
                            decoded.replies.push(self.peer.notification(
                                "_x.ai/yolo_mode_changed",
                                json!({"sessionId":native,"yolo_mode":false,"auto_mode":false}),
                            ));
                        }
                        #[cfg(debug_assertions)]
                        {
                            decoded.recording = Some(crate::record::Metadata {
                                native: Some(native.clone()),
                                version: self.version.clone(),
                            });
                        }
                        decoded
                            .events
                            .push(ProviderEvent::Session(SessionEvent::Ready {
                                native: NativeSessionId(native),
                                cli_version: self.version.clone(),
                                pid: self.pid,
                                pgid: self.pid,
                                capabilities: vec![],
                            }));
                    }
                    Err(error)
                        if auth_required(&error)
                            && matches!(retry, AuthenticationRetry::Available) =>
                    {
                        if let Some(mcp) = &self.mcp {
                            mcp.revoke();
                        }
                        decoded
                            .replies
                            .push(self.authenticate(target, AuthenticationRetry::Used)?);
                    }
                    Err(error) if !matches!(target, OpenTarget::New) && not_found(&error) => {
                        if let Some(mcp) = &self.mcp {
                            mcp.revoke();
                        }
                        decoded.events.push(ProviderEvent::Session(SessionEvent::Notice(
                            crate::domain::Notice {
                                text: "The earlier conversation could not be resumed; this is a new session".into(),
                                reset_at: None,
                            },
                        )));
                        decoded.controls.push(self.open(OpenTarget::New, retry)?);
                    }
                    Err(error) => {
                        return Err(if auth_required(&error) {
                            signed_out()
                        } else {
                            jsonrpc::failure(error)
                        });
                    }
                }
            }
            Frame::Reply {
                tag: Call::Prompt(id),
                result,
            } => {
                let (outcome, accepted) = match result {
                    Ok(result) => (outcome(&result), true),
                    Err(error) => (
                        TurnOutcome::Failed {
                            message: error.message,
                        },
                        false,
                    ),
                };
                self.finish(&id, outcome, accepted, &mut decoded);
            }
            Frame::Notification { method, params } => {
                if self.notification(&method, params, &mut decoded).is_err() {
                    tracing::debug!(method, "Skipped malformed Grok notification");
                }
            }
            Frame::Request { id, method, params } => {
                let reply_id = id.clone();
                if self.request(id, &method, params, &mut decoded).is_err() {
                    decoded
                        .replies
                        .push(self.peer.error(reply_id, -32602, "Invalid Grok request"));
                }
            }
            Frame::Unknown => {}
        }
        Ok(decoded)
    }
    fn notification(
        &mut self,
        method: &str,
        params: Value,
        decoded: &mut Decoded,
    ) -> Result<(), ProviderError> {
        let notification: NotificationMethod = decode(json!(method))?;
        if notification == NotificationMethod::Unknown {
            tracing::debug!(method, "Unknown Grok notification");
            return Ok(());
        }
        let scope: PromptScope = decode(params.clone())?;
        if self
            .native()
            .is_none_or(|native| native.0 != scope.session_id)
            || scope.meta.is_replay
        {
            return Ok(());
        }
        if matches!(
            notification,
            NotificationMethod::TaskBackgrounded | NotificationMethod::TaskCompleted
        ) {
            let task: TaskUpdate = decode(params["update"].clone())?;
            let task_id = task
                .task_snapshot
                .map(|snapshot| snapshot.task_id)
                .filter(|id| !id.is_empty())
                .unwrap_or(task.task_id);
            if !task_id.is_empty() {
                if notification == NotificationMethod::TaskBackgrounded {
                    self.tasks.insert(task_id);
                } else {
                    self.tasks.remove(&task_id);
                }
                decoded
                    .events
                    .push(ProviderEvent::Session(SessionEvent::BackgroundWork {
                        pending: !self.tasks.is_empty(),
                    }));
            }
            return Ok(());
        }
        if notification == NotificationMethod::SessionNotification {
            let completed: TurnCompleted = decode(params["update"].clone())?;
            if completed.update == "turn_completed"
                && let Some(id) = completed.prompt_id
            {
                match self.prompts.get_mut(&id) {
                    Some(prompt) if matches!(prompt.origin, PromptOrigin::Veyra(_)) => {
                        prompt.completion.get_or_insert_with(|| PromptCompletion {
                            deadline: Instant::now() + Duration::from_secs(2),
                            outcome: completed.completion.outcome(),
                        });
                    }
                    _ => {
                        self.ended.insert(id.clone());
                        self.finish(&id, completed.completion.outcome(), true, decoded);
                    }
                }
            }
            return Ok(());
        }
        if notification == NotificationMethod::PromptComplete {
            let completion: Completion = decode(params)?;
            let id = scope.prompt_id().map(str::to_owned);
            if let Some(id) = id {
                self.finish(&id, completion.outcome(), true, decoded);
            } else {
                tracing::info!("ignored untagged Grok prompt_complete");
            }
            return Ok(());
        }
        let update = params["update"].clone();
        let header: UpdateHeader = decode(update.clone())?;
        let update = match header.kind {
            UpdateKind::AgentMessageChunk => Update::Message(decode(update["content"].clone())?),
            UpdateKind::AgentThoughtChunk => Update::Thought(decode(update["content"].clone())?),
            UpdateKind::ToolCall | UpdateKind::ToolCallUpdate => Update::Tool(decode(update)?),
            UpdateKind::Plan => Update::Plan(decode(update)?),
            UpdateKind::UserMessageChunk | UpdateKind::TurnCompleted | UpdateKind::Unknown => {
                tracing::debug!(
                    kind = update["sessionUpdate"].as_str().unwrap_or_default(),
                    "Unknown Grok session update"
                );
                return Ok(());
            }
        };
        let Some(id) = self.prompt(scope.prompt_id(), decoded) else {
            return Ok(());
        };
        let owner = self.active().unwrap();
        if matches!(owner, ActiveTurn::Veyra(_))
            && matches!(self.prompts[&id].origin, PromptOrigin::Foreign)
        {
            if header.kind == UpdateKind::ToolCall
                && let Update::Tool(tool) = update
                && !tool.tool_call_id.is_empty()
            {
                self.prompts.get_mut(&id).unwrap().tool_starts.insert(
                    tool.tool_call_id,
                    ToolCall {
                        name: tool.title,
                        arguments: tool.raw_input,
                        veyra_label: false,
                        input_summary: None,
                    },
                );
            }
            tracing::debug!(method, "Skipped foreign Grok content during Veyra prompt");
            return Ok(());
        }
        let key = |suffix: &str| NativeItemKey(format!("{id}/{suffix}"));
        if !matches!(update, Update::Message(_))
            && let Some(key) = self.prompts.get_mut(&id).unwrap().reply_segment.close(&id)
        {
            Self::event(decoded, owner, TurnEvent::ItemEnded { key });
        }
        match update {
            Update::Message(Content::Text { text }) => Self::event(
                decoded,
                owner,
                TurnEvent::TextDelta {
                    key: self.prompts.get_mut(&id).unwrap().reply_segment.next(&id),
                    text,
                    streamed: true,
                },
            ),
            Update::Thought(Content::Text { text }) => Self::event(
                decoded,
                owner,
                TurnEvent::ReasoningDelta {
                    key: key("thought"),
                    text,
                },
            ),
            Update::Tool(tool) => {
                let mut tool = *tool;
                if tool.tool_call_id.is_empty() {
                    return Ok(());
                }
                let tool_key = tool_key(owner, &id, &tool.tool_call_id);
                let output = tool_output(&tool.raw_output, &tool.content);
                if let Some(start) = self
                    .prompts
                    .get_mut(&id)
                    .unwrap()
                    .tool_starts
                    .remove(&tool.tool_call_id)
                {
                    if tool.title.is_empty() {
                        tool.title = start.name;
                    }
                    if !start.arguments.is_null() {
                        tool.raw_input = start.arguments;
                    }
                }
                let state = self.tools.entry(tool_key.clone()).or_insert_with(|| {
                    Self::event(
                        decoded,
                        owner,
                        TurnEvent::ToolStarted {
                            key: NativeItemKey(tool_key.clone()),
                            call: ToolCall {
                                name: tool.title.clone(),
                                arguments: tool.raw_input,
                                veyra_label: false,
                                input_summary: None,
                            },
                        },
                    );
                    ToolState {
                        title: tool.title.clone(),
                        output: String::new(),
                        ended: false,
                    }
                });
                if state.ended {
                    return Ok(());
                }
                if !tool.title.is_empty() && tool.title != state.title {
                    state.title = tool.title.clone();
                    Self::event(
                        decoded,
                        owner,
                        TurnEvent::ToolTitleChanged {
                            key: NativeItemKey(tool_key.clone()),
                            title: tool.title,
                        },
                    );
                }
                let previous = &mut state.output;
                let delta = output
                    .map(|output| {
                        let delta = output
                            .strip_prefix(previous.as_str())
                            .unwrap_or(&output)
                            .to_owned();
                        *previous = output;
                        delta
                    })
                    .filter(|text| !text.is_empty());
                if matches!(tool.status, ToolStatus::Completed | ToolStatus::Failed) {
                    state.ended = true;
                    Self::event(
                        decoded,
                        owner,
                        TurnEvent::ToolFinished {
                            key: NativeItemKey(tool_key),
                            ok: tool.status == ToolStatus::Completed,
                            output_tail: delta,
                        },
                    );
                } else if let Some(text) = delta {
                    Self::event(
                        decoded,
                        owner,
                        TurnEvent::ToolOutput {
                            key: NativeItemKey(tool_key),
                            text,
                        },
                    );
                }
            }
            Update::Plan(plan) => Self::event(
                decoded,
                owner,
                TurnEvent::Plan(
                    plan.entries
                        .into_iter()
                        .filter_map(|entry| serde_json::from_value::<PlanEntry>(entry).ok())
                        .map(|entry| PlanStep {
                            text: entry.content,
                            done: entry.status == ToolStatus::Completed,
                        })
                        .collect(),
                ),
            ),
            Update::Message(Content::Unknown) | Update::Thought(Content::Unknown) => {}
        }
        Ok(())
    }
    fn approval_field(field: &str, value: &Value, shown: &[&str]) -> super::ApprovalField {
        use super::ApprovalField::*;
        match field {
            field if shown.contains(&field) => Shown,
            "description" | "reason" | "timeout" | "variant" => Passthrough {
                affects_execution: false,
            },
            "is_background" => Passthrough {
                affects_execution: value.as_bool() != Some(false),
            },
            _ => Passthrough {
                affects_execution: true,
            },
        }
    }
    fn request(
        &mut self,
        id: Value,
        method: &str,
        params: Value,
        decoded: &mut Decoded,
    ) -> Result<(), ProviderError> {
        let native = id.to_string();
        if !self.seen_requests.insert(native.clone()) {
            decoded
                .replies
                .push(self.peer.error(id, -32600, "Request id already used"));
            return Ok(());
        }
        let mut requested_tool = None;
        let parsed = match decode::<RequestMethod>(json!(method))? {
            RequestMethod::Permission => {
                let permission: Permission = decode(params.clone())?;
                if self.native().is_none_or(|n| n.0 != permission.session_id)
                    || permission.tool_call.tool_call_id.is_empty()
                {
                    None
                } else {
                    requested_tool = Some(permission.tool_call.tool_call_id.clone());
                    let allow = permission
                        .options
                        .iter()
                        .find(|o| o.kind == OptionKind::AllowOnce)
                        .map(|o| o.option_id.clone());
                    let reject = permission
                        .options
                        .iter()
                        .find(|o| o.kind == OptionKind::RejectOnce)
                        .map(|o| o.option_id.clone());
                    let command = permission.tool_call.raw_input["command"].as_str();
                    let reason =
                        command.and_then(|_| {
                            super::approval_explanation(["description", "reason"].map(|field| {
                                (field, permission.tool_call.raw_input[field].as_str())
                            }))
                        });
                    let mut shown = Vec::new();
                    if command.is_some() {
                        shown.push("command");
                    }
                    if let Some((field, _)) = &reason {
                        shown.push(field);
                    }
                    let bash = permission.tool_call.raw_input["variant"].as_str() == Some("Bash");
                    let fields =
                        super::ApprovalFields::new(&permission.tool_call.raw_input, |field| {
                            Self::approval_field(
                                field,
                                &permission.tool_call.raw_input[field],
                                &shown,
                            )
                        });
                    let exact_command = bash && fields.permits_grant();
                    let grant_key = command.filter(|_| exact_command).map(|command| GrantKey {
                        tool: "Bash".into(),
                        command: command.into(),
                    });
                    let preview = fields.preview(&permission.tool_call.raw_input);
                    let target = if let Some(command) = command {
                        ApprovalTarget::Command {
                            command: command.into(),
                            cwd: if exact_command {
                                CommandCwd::ProjectFolder
                            } else {
                                CommandCwd::NotVerified
                            },
                            reason: reason.map(|(_, preview)| preview),
                            preview,
                        }
                    } else {
                        ApprovalTarget::Tool { preview }
                    };
                    let mut action = ProposedAction::new(permission.tool_call.title, target)
                        .with_grant(grant_key);
                    if allow.is_none() {
                        action.consent = ApprovalConsent::DenyOnly {
                            reason: "Grok offered only a lasting grant".into(),
                        };
                    }
                    Some((
                        RequestKind::Approval { action },
                        PermissionReply::Permission { allow, reject },
                        permission.meta.prompt_id,
                    ))
                }
            }
            RequestMethod::Question => {
                let question: QuestionParams = decode(params)?;
                let valid = self.native().is_some_and(|n| n.0 == question.session_id)
                    && !question.tool_call_id.is_empty()
                    && !question.questions.is_empty()
                    && question.questions.iter().all(|q| {
                        !q.question.is_empty()
                            && !q.options.is_empty()
                            && q.options.iter().all(|o| !o.label.is_empty())
                    })
                    && question
                        .questions
                        .iter()
                        .map(|q| &q.question)
                        .collect::<BTreeSet<_>>()
                        .len()
                        == question.questions.len();
                valid.then(|| {
                    (
                        RequestKind::Question {
                            questions: question
                                .questions
                                .into_iter()
                                .map(|q| Question {
                                    text: q.question,
                                    options: q.options.into_iter().map(|o| o.label).collect(),
                                })
                                .collect(),
                        },
                        PermissionReply::Question,
                        question.meta.prompt_id,
                    )
                })
            }
            RequestMethod::Unknown => {
                decoded
                    .replies
                    .push(self.peer.error(id, -32601, "Unsupported server request"));
                return Ok(());
            }
        };
        let Some((kind, reply, prompt_id)) = parsed else {
            decoded
                .replies
                .push(self.peer.error(id, -32602, "Invalid Grok request"));
            return Ok(());
        };
        let prompt_id = prompt_id.or_else(|| match self.work() {
            Some(PromptWork::Provider { .. }) => self.prompts.keys().next().cloned(),
            _ => None,
        });
        let Some(prompt_id) = self.prompt(prompt_id.as_deref(), decoded) else {
            decoded
                .replies
                .push(self.peer.error(id, -32600, "Request has no active prompt"));
            return Ok(());
        };
        let owner = self.active().unwrap();
        if let Some(key) = self
            .prompts
            .get_mut(&prompt_id)
            .unwrap()
            .reply_segment
            .close(&prompt_id)
        {
            Self::event(decoded, owner, TurnEvent::ItemEnded { key });
        }
        Self::event(
            decoded,
            owner,
            TurnEvent::RequestOpened {
                tool: requested_tool.map(|id| NativeItemKey(tool_key(owner, &prompt_id, &id))),
                native: NativeRequestRef(native.clone()),
                kind: kind.clone(),
            },
        );
        self.requests.insert(
            native,
            Request {
                id,
                kind,
                reply,
                owner,
            },
        );
        Ok(())
    }
    pub(crate) fn answer(
        &mut self,
        native: NativeRequestRef,
        reply: super::Reply,
    ) -> Option<Value> {
        let request = self.requests.get(&native.0)?;
        let decision = request.kind.accepts(&reply)?;
        let result = match (&request.reply, decision) {
            (PermissionReply::Permission { allow, .. }, AcceptedDecision::Allow { .. }) => {
                json!({"outcome":{"outcome":"selected","optionId":allow.as_ref()?}})
            }
            (PermissionReply::Permission { reject, .. }, AcceptedDecision::Deny { .. }) => {
                match reject {
                    Some(id) => json!({"outcome":{"outcome":"selected","optionId":id}}),
                    None => json!({"outcome":{"outcome":"cancelled"}}),
                }
            }
            (PermissionReply::Question, AcceptedDecision::Answers { questions, answers }) => {
                let answers: serde_json::Map<String, Value> = questions
                    .iter()
                    .zip(answers)
                    .map(|(q, a)| (q.text.clone(), json!([a.text])))
                    .collect();
                json!({"outcome":"accepted","answers":answers})
            }
            _ => return None,
        };
        let request = self.requests.remove(&native.0)?;
        Some(self.peer.response(request.id, result))
    }
    pub(crate) fn interrupt_active(&mut self, reason: StopReason) -> Vec<Outbound> {
        self.active().map_or_else(Vec::new, |active| {
            self.interrupt_lines(active.target(), reason)
        })
    }
    pub(crate) fn interrupt_lines(&mut self, turn: TurnId, reason: StopReason) -> Vec<Outbound> {
        if self.active().is_none_or(|active| active.target() != turn) || self.interrupted.is_some()
        {
            return vec![];
        }
        let Some(native) = self.native().cloned() else {
            return vec![];
        };
        self.interrupted = Some(super::StopLadder::new(reason));
        let mut lines = Vec::new();
        for (_, request) in std::mem::take(&mut self.requests) {
            let result = cancelled(&request.reply);
            lines.push(Outbound::Message(self.peer.response(request.id, result)));
        }
        lines.push(Outbound::Interrupt {
            turn,
            line: self.peer.notification(
                "session/cancel",
                json!({"sessionId":native.0,"_meta":{"cancelTrigger":"ctrl_c"}}),
            ),
        });
        lines
    }
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.prompts
            .values()
            .filter_map(|p| p.first_frame)
            .chain(
                self.prompts
                    .values()
                    .filter_map(|prompt| prompt.completion.as_ref().map(|end| end.deadline)),
            )
            .chain(self.interrupted.as_ref().map(super::StopLadder::deadline))
            .min()
    }
    pub(crate) fn timer_lines(&mut self) -> Result<Vec<Outbound>, ProviderError> {
        if self
            .prompts
            .values()
            .any(|p| p.first_frame.is_some_and(|at| Instant::now() >= at))
        {
            return Err(ProviderError::Protocol(
                "Grok sent no frame within 60 seconds of session/prompt".into(),
            ));
        }
        Ok(vec![])
    }
    pub(crate) fn completed_prompts(&mut self) -> Decoded {
        let due: Vec<_> = self
            .prompts
            .iter()
            .filter_map(|(id, prompt)| {
                prompt
                    .completion
                    .as_ref()
                    .filter(|end| Instant::now() >= end.deadline)
                    .map(|end| (id.clone(), end.outcome.clone()))
            })
            .collect();
        let mut decoded = Decoded::default();
        for (id, outcome) in due {
            self.finish(&id, outcome, true, &mut decoded);
        }
        decoded
    }
    pub(crate) fn escalate_interrupt(&mut self) -> Option<Signal> {
        self.interrupted.as_mut()?.escalate()
    }
    pub(crate) fn end_outcome(&self, outcome: TurnOutcome) -> TurnOutcome {
        self.interrupted
            .as_ref()
            .map_or(outcome, super::StopLadder::outcome)
    }
    pub(crate) fn eof(&mut self) {
        self.peer.eof();
    }
}
fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ProviderError> {
    serde_json::from_value(value).map_err(|_| ProviderError::Protocol("Invalid Grok frame".into()))
}
pub(super) async fn probe_exchange(
    reader: &mut jsonrpc::Reader,
    writer: &tokio::sync::mpsc::UnboundedSender<Outbound>,
    binary: std::path::PathBuf,
    api_key: bool,
) -> Result<super::Probe, ProviderError> {
    let mut peer = Peer::new(true);
    writer
        .send(Outbound::Message(peer.send((), "initialize", initialize())))
        .map_err(|_| ProviderError::Protocol("Grok probe writer closed".into()))?;
    while let Some(line) = reader.next().await? {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Frame::Reply { result, .. } = peer.receive(value) {
            let init: Initialization = decode(result.map_err(jsonrpc::failure)?)?;
            let version = init.version();
            let readiness = if auth_method(&init.auth_methods, api_key).is_some() {
                super::Readiness::Ready
            } else {
                super::Readiness::SignedOut
            };
            let state = &init.meta["modelState"];
            let mut models = state["availableModels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|model| {
                    let id = model["modelId"].as_str().filter(|id| !id.is_empty())?;
                    let mut efforts = model["_meta"]["reasoningEfforts"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>();
                    efforts.sort_by_key(|effort| effort["default"].as_bool() != Some(true));
                    Some(super::Model {
                        value: id.into(),
                        display_name: model["name"].as_str().unwrap_or_default().into(),
                        effort_levels: efforts
                            .into_iter()
                            .filter_map(|effort| {
                                effort["value"]
                                    .as_str()
                                    .filter(|v| !v.is_empty())
                                    .or_else(|| effort["id"].as_str().filter(|v| !v.is_empty()))
                                    .map(str::to_owned)
                            })
                            .collect(),
                    })
                })
                .collect::<Vec<_>>();
            models.sort_by_key(|model| {
                Some(model.value.as_str()) != state["currentModelId"].as_str()
            });
            return Ok(super::Probe {
                provider: super::ProviderKind::Grok,
                path: Some(binary),
                version,
                readiness,
                models,
                account: None,
            });
        }
    }
    Err(ProviderError::Protocol(
        "Grok probe ended before initialize".into(),
    ))
}
fn default_on_invalid<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}
fn auth_method(methods: &[AuthMethod], api_key: bool) -> Option<String> {
    ["xai.api_key", "cached_token"]
        .into_iter()
        .find(|id| (*id != "xai.api_key" || api_key) && methods.iter().any(|m| m.id == *id))
        .map(str::to_owned)
}
fn not_found(error: &jsonrpc::Error) -> bool {
    let text = error.message.to_lowercase();
    !auth_required(error)
        && (text.contains("session not found") || text.contains("session does not exist"))
}
fn outcome(result: &Value) -> TurnOutcome {
    let completion: Completion = serde_json::from_value(result.clone()).unwrap_or_default();
    completion.outcome()
}
impl Completion {
    fn outcome(self) -> TurnOutcome {
        match self.stop_reason {
            CompletionReason::Cancelled => TurnOutcome::Interrupted {
                reason: if self.cancellation_category.as_str() == Some("PermissionRejected") {
                    "Stopped after you denied a request"
                } else {
                    "Grok cancelled the turn"
                }
                .into(),
            },
            CompletionReason::Error | CompletionReason::Refusal | CompletionReason::RateLimit => {
                TurnOutcome::Failed {
                    message: self
                        .agent_result
                        .as_str()
                        .unwrap_or("Grok could not complete the turn")
                        .into(),
                }
            }
            CompletionReason::EndTurn | CompletionReason::Unknown => TurnOutcome::Completed,
        }
    }
}
fn cancelled(reply: &PermissionReply) -> Value {
    match reply {
        PermissionReply::Permission { .. } => json!({"outcome":{"outcome":"cancelled"}}),
        PermissionReply::Question => json!({"outcome":"cancelled"}),
    }
}
fn auth_required(error: &jsonrpc::Error) -> bool {
    error.code == -32000
}
fn signed_out() -> ProviderError {
    ProviderError::Protocol("Sign in with `grok login`".into())
}
fn tool_output(raw: &Value, content: &[Value]) -> Option<String> {
    let text = if let Some(text) = raw.as_str() {
        text.into()
    } else if !raw.is_null() {
        raw.to_string()
    } else {
        content
            .iter()
            .filter_map(|c| c["content"]["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    fn boundary_codec() -> (Codec, TurnId) {
        let turn = TurnId::new();
        let mut codec = Codec::new(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: "/project".into(),
                native: None,
                root: false,
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
            },
            1,
            None,
            false,
        );
        codec.state = SessionState::Ready {
            native: NativeSessionId("veyra-grok-session".into()),
            active: None,
        };
        codec.start(turn, "reply".into()).unwrap();
        (codec, turn)
    }
    fn fixture_frame(source: &str, predicate: impl Fn(&Value) -> bool, turn: TurnId) -> Value {
        let mut frame = source
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|record| record["type"] == "emit_inbound")
            .map(|record| record["line"].clone())
            .find(predicate)
            .unwrap();
        frame["params"]["_meta"]["promptId"] = json!(turn.to_string());
        frame
    }
    #[test]
    fn grok_fixture_thought_plan_permission_and_question_each_close_text() {
        for boundary in ["thought", "plan", "permission", "question"] {
            let (mut codec, turn) = boundary_codec();
            let message = fixture_frame(
                include_str!("../../tests/fixtures/grok/tool-title.jsonl"),
                |frame| frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk",
                turn,
            );
            let before = codec.value(message.clone()).unwrap();
            let frame = match boundary {
                "thought" => fixture_frame(
                    include_str!("../../tests/fixtures/grok/foreign-tool-span.jsonl"),
                    |frame| frame["params"]["update"]["sessionUpdate"] == "agent_thought_chunk",
                    turn,
                ),
                "plan" => fixture_frame(
                    include_str!("../../tests/fixtures/grok/plan-entry-defaults.jsonl"),
                    |frame| frame["params"]["update"]["sessionUpdate"] == "plan",
                    turn,
                ),
                "permission" => fixture_frame(
                    include_str!("../../tests/fixtures/grok/approval-deny.jsonl"),
                    |frame| frame["method"] == "session/request_permission",
                    turn,
                ),
                _ => fixture_frame(
                    include_str!("../../tests/fixtures/grok/question.jsonl"),
                    |frame| frame["method"] == "_x.ai/ask_user_question",
                    turn,
                ),
            };
            let boundary_events = codec.value(frame).unwrap();
            let after = codec.value(message).unwrap();
            let text_key = |decoded: &Decoded| {
                decoded
                    .events
                    .iter()
                    .find_map(|event| match event {
                        ProviderEvent::Turn {
                            event: TurnEvent::TextDelta { key, .. },
                            ..
                        } => Some(key.clone()),
                        _ => None,
                    })
                    .unwrap()
            };
            let before = text_key(&before);
            assert!(
                boundary_events.events.iter().any(|event| matches!(event,
                ProviderEvent::Turn { event: TurnEvent::ItemEnded { key }, .. } if key == &before)),
                "{boundary} did not close the preceding reply"
            );
            assert_ne!(
                before,
                text_key(&after),
                "{boundary} joined separate reply items"
            );
        }
    }
    #[test]
    fn grok_fixture_permission_carries_only_its_native_tool_key() {
        let (mut codec, turn) = boundary_codec();
        let started = codec
            .value(
                json!({"jsonrpc":"2.0", "method":"session/update", "params":{
            "sessionId":"veyra-grok-session", "_meta":{"promptId":turn.to_string()}, "update":{
            "sessionUpdate":"tool_call", "toolCallId":"create-check", "title":"Execute",
            "status":"pending", "rawInput":{"command":"printf check","variant":"Bash"}}}}),
            )
            .unwrap();
        let frame = fixture_frame(
            include_str!("../../tests/fixtures/grok/approval-deny.jsonl"),
            |frame| frame["method"] == "session/request_permission",
            turn,
        );
        let expected = NativeItemKey(tool_key(
            ActiveTurn::Veyra(turn),
            &turn.to_string(),
            "create-check",
        ));
        assert!(started.events.iter().any(|event| matches!(event,
            ProviderEvent::Turn { event: TurnEvent::ToolStarted { key, .. }, .. } if key == &expected)));
        let decoded = codec.value(frame).unwrap();
        assert!(decoded.events.iter().any(|event| matches!(event,
            ProviderEvent::Turn { event: TurnEvent::RequestOpened { tool: Some(key), .. }, .. } if key == &expected)));
        let frame = fixture_frame(
            include_str!("../../tests/fixtures/grok/question.jsonl"),
            |frame| frame["method"] == "_x.ai/ask_user_question",
            turn,
        );
        let decoded = codec.value(frame).unwrap();
        assert!(decoded.events.iter().any(|event| matches!(
            event,
            ProviderEvent::Turn {
                event: TurnEvent::RequestOpened { tool: None, .. },
                ..
            }
        )));
    }
    #[test]
    fn grok_explanation_source_and_residual_are_codec_owned() {
        for (description, reason, selected, source) in [
            (
                json!("Description"),
                json!("Reason"),
                "Description",
                "description",
            ),
            (
                json!({"nested":"keep"}),
                json!("Reason"),
                "Reason",
                "reason",
            ),
            (Value::Null, json!(""), "", "reason"),
        ] {
            let (mut codec, turn) = boundary_codec();
            let mut frame = fixture_frame(
                include_str!("../../tests/fixtures/grok/approval-deny.jsonl"),
                |frame| frame["method"] == "session/request_permission",
                turn,
            );
            frame["params"]["toolCall"]["rawInput"] = json!({"command":"printf check",
                "variant":"Bash", "description":description, "reason":reason});
            let decoded = codec.value(frame).unwrap();
            let action = decoded
                .events
                .iter()
                .find_map(|event| match event {
                    ProviderEvent::Turn {
                        event:
                            TurnEvent::RequestOpened {
                                kind: RequestKind::Approval { action },
                                ..
                            },
                        ..
                    } => Some(action),
                    _ => None,
                })
                .unwrap();
            let encoded = serde_json::to_value(action).unwrap();
            assert_eq!(encoded["target"]["reason"]["text"], selected);
            assert_eq!(encoded["target"]["reason"]["explanation"], source);
            let residual: Value =
                serde_json::from_str(encoded["target"]["preview"]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(
                residual,
                if source == "description" {
                    json!({"variant":"Bash", "reason":reason})
                } else {
                    json!({"variant":"Bash", "description":description})
                }
            );
        }
    }
    #[test]
    fn grok_recorded_live_foreground_grant_does_not_cover_background() {
        let mut frame = serde_json::from_str::<Value>(
            r###"
{
    "jsonrpc": "2.0",
    "id": "recorded-approval",
    "method": "session/request_permission",
    "params": {
        "sessionId": "veyra-grok-session",
        "toolCall": {
            "toolCallId": "recorded-approval",
            "kind": "execute",
            "title": "Execute `printf 'veyra\\n' > veyra-check.txt`",
            "rawInput": {
                "variant": "Bash",
                "command": "printf 'veyra\\n' > veyra-check.txt",
                "description": "Write veyra newline to check file",
                "is_background": false
            },
            "_meta": {
                "x.ai/tool": {
                    "version": 1,
                    "name": "run_terminal_command",
                    "kind": "execute",
                    "namespace": "grok_build",
                    "label": "Run Command",
                    "read_only": false,
                    "input": {
                        "command": "printf 'veyra\\n' > veyra-check.txt",
                        "description": "Write veyra newline to check file"
                    }
                },
                "x.ai/requestingSessionId": "<id:session>"
            }
        },
        "options": [
            {
                "optionId": "always-allow",
                "name": "Yes, and don't ask again for bash commands",
                "kind": "allow_always"
            },
            {
                "optionId": "allow-once",
                "name": "Yes, proceed",
                "kind": "allow_once"
            },
            {
                "optionId": "reject-once",
                "name": "No, and tell Grok what to do differently",
                "kind": "reject_once"
            },
            {
                "optionId": "reject-always",
                "name": "No, and don't ask again for this command",
                "kind": "reject_always"
            }
        ]
    }
}
"###,
        )
        .unwrap();
        let command = frame["params"]["toolCall"]["rawInput"]["command"]
            .as_str()
            .unwrap()
            .to_owned();
        let (mut codec, _) = boundary_codec();
        let action = super::super::approval_test_action(codec.value(frame.clone()).unwrap());
        assert_eq!(
            action.consent.grant_key(),
            Some(&GrantKey {
                tool: "Bash".into(),
                command: command.clone()
            }),
            "explicit foreground must retain the thread grant"
        );
        let ApprovalTarget::Command {
            command: shown,
            cwd,
            reason,
            preview,
        } = &action.target
        else {
            panic!("recorded command target")
        };
        assert_eq!(shown, &command);
        assert_eq!(cwd, &CommandCwd::ProjectFolder);
        assert_eq!(
            reason.as_ref().unwrap().preview.text,
            "Write veyra newline to check file"
        );
        assert_eq!(
            reason.as_ref().unwrap().explanation,
            Some(ApprovalExplanation::Description)
        );
        assert_eq!(
            preview.text,
            serde_json::to_string_pretty(&json!({"variant":"Bash","is_background":false})).unwrap()
        );
        frame["id"] = json!("recorded-background");
        frame["params"]["toolCall"]["rawInput"]["is_background"] = json!(true);
        let background = super::super::approval_test_action(codec.value(frame).unwrap());
        assert!(
            background.consent.grant_key().is_none(),
            "foreground grant must not cover background execution"
        );
        let ApprovalTarget::Command { preview, .. } = &background.target else {
            panic!("background command")
        };
        assert_eq!(
            preview.text,
            serde_json::to_string_pretty(&json!({"variant":"Bash","is_background":true})).unwrap()
        );
    }
    #[test]
    fn grok_recorded_search_replace_preserves_every_edit_argument() {
        let frame = serde_json::from_str::<Value>(
            r###"{
  "jsonrpc": "2.0",
  "id": "recorded-edit",
  "method": "session/request_permission",
  "params": {
    "sessionId": "veyra-grok-session",
    "toolCall": {
      "toolCallId": "recorded-edit",
      "kind": "edit",
      "title": "Edit `<cwd>/permission-project.txt`",
      "rawInput": {
        "variant": "SearchReplace",
        "file_path": "<cwd>/permission-project.txt",
        "old_string": "",
        "new_string": "veyra permission probe\n",
        "replace_all": false
      },
      "_meta": {
        "x.ai/tool": {
          "version": 1,
          "name": "search_replace",
          "kind": "edit",
          "namespace": "grok_build",
          "label": "Edit",
          "read_only": false,
          "input": {
            "path": "<cwd>/permission-project.txt"
          }
        },
        "x.ai/requestingSessionId": "veyra-grok-session"
      }
    },
    "options": [
      {
        "optionId": "allow-edits-session",
        "name": "Yes, allow all edits during this session",
        "kind": "allow_always"
      },
      {
        "optionId": "allow-once",
        "name": "Yes",
        "kind": "allow_once"
      },
      {
        "optionId": "reject-once",
        "name": "No, and tell Grok what to do differently",
        "kind": "reject_once"
      }
    ]
  }
}"###,
        )
        .unwrap();
        let (mut codec, _) = boundary_codec();
        let action = super::super::approval_test_action(codec.value(frame).unwrap());
        assert_eq!(action.tool, "Edit `<cwd>/permission-project.txt`");
        assert!(action.consent.grant_key().is_none());
        assert!(matches!(action.target, ApprovalTarget::Tool { .. }));
        super::super::assert_approval_preview(
            &action,
            json!({
                "variant":"SearchReplace", "file_path":"<cwd>/permission-project.txt",
                "old_string":"", "new_string":"veyra permission probe\n", "replace_all":false
            }),
            "recorded SearchReplace",
        );
        let reply = codec
            .answer(
                NativeRequestRef("\"recorded-edit\"".into()),
                Decision::AllowOnce,
            )
            .unwrap();
        assert_eq!(
            reply["result"]["outcome"],
            json!({"outcome":"selected","optionId":"allow-once"})
        );
    }
    #[test]
    fn grok_invalid_background_stays_visible_and_withholds_grant() {
        for background in [json!("false"), json!(0), Value::Null, json!([]), json!({})] {
            let (mut codec, turn) = boundary_codec();
            let mut frame = fixture_frame(
                include_str!("../../tests/fixtures/grok/approval-deny.jsonl"),
                |frame| frame["method"] == "session/request_permission",
                turn,
            );
            frame["params"]["toolCall"]["rawInput"] = json!({
                "variant":"Bash", "command":"printf check", "is_background":background
            });
            let action = super::super::approval_test_action(codec.value(frame).unwrap());
            assert!(action.consent.grant_key().is_none(), "{background}");
            let ApprovalTarget::Command { cwd, .. } = &action.target else {
                panic!("invalid background command")
            };
            assert_eq!(cwd, &CommandCwd::NotVerified);
            super::super::assert_approval_preview(
                &action,
                json!({"variant":"Bash", "is_background":background}),
                "is_background",
            );
        }
    }
    #[test]
    fn grok_all_approval_kinds_preserve_complete_fields() {
        for (index, (variant, command)) in [
            ("Bash", json!("printf check")),
            ("Edit", json!(42)),
            ("SearchReplace", Value::Null),
            ("mcp__local__change", json!({"nested":"command"})),
            ("FutureTool", Value::Null),
        ]
        .into_iter()
        .enumerate()
        {
            let (mut codec, turn) = boundary_codec();
            let mut frame = fixture_frame(
                include_str!("../../tests/fixtures/grok/approval-deny.jsonl"),
                |frame| frame["method"] == "session/request_permission",
                turn,
            );
            let input = json!({"variant":variant, "command":command, "cwd":"relative/folder",
                "description":{"nested":"not a string"}, "reason":"Actual reason", "timeout":1500,
                "is_background":true, "file_path":"relative/file", "path":"relative/file",
                "notebook_path":"relative/file", "content":"whole file", "old_string":"old",
                "new_string":"new", "replace_all":false, "env":{"CHECK":"material"}, "networkAccess":false,
                "dangerouslyDisableSandbox":true, "itemId":"argument identity", "unknownFlag":false,
                "unknownObject":{"command":"nested material"}, "unknownArray":[1,"two",null]});
            frame["id"] = json!(format!("complete-{index}"));
            frame["params"]["toolCall"]["rawInput"] = input.clone();
            frame["params"]["toolCall"]["title"] = json!(variant);
            frame["params"]["toolCall"]["kind"] = json!("execute");
            frame["params"]["toolCall"]["locations"] = json!([{"path":"/project/file"}]);
            frame["params"]["toolCall"]["_meta"] = json!({"x.ai/requestingSessionId":"internal-session",
                "x.ai/tool":{"input":{"command":"printf check","description":"Tool documentation"},
                    "kind":"execute",
                        "label":"Run Command",
                        "name":"run_terminal_command",
                        "namespace":"grok_build",
                        "read_only":false,
                        "version":1}});
            for option in frame["params"]["options"].as_array_mut().unwrap() {
                option["name"] = json!("Consent option");
            }
            frame["params"]["toolCall"]["status"] = json!("pending");
            frame["params"]["toolCall"]["rawOutput"] =
                json!({"internalOutput":"not approval input"});
            frame["params"]["toolCall"]["content"] =
                json!([{"type":"text", "text":"internal content"}]);
            frame["params"]["_meta"]["promptIndex"] = json!(1);
            frame["params"]["_meta"]["isReplay"] = json!(false);
            frame["params"] = super::super::approval_wire_sample(
                "grok",
                variant,
                serde_json::to_value(Permission {
                    options: vec![PermissionOption::default()],
                    ..Default::default()
                })
                .unwrap(),
                frame["params"].clone(),
            );
            if index == 0 {
                for (field, value) in input.as_object().unwrap() {
                    let mut solo = json!({"variant":"Bash",
                        "command":"printf check",
                        "description":"Primary",
                        "reason":"Secondary"});
                    solo[field] = value.clone();
                    let mut expected = solo.clone();
                    let (mut codec, turn) = boundary_codec();
                    let mut frame = fixture_frame(
                        include_str!("../../tests/fixtures/grok/approval-deny.jsonl"),
                        |frame| frame["method"] == "session/request_permission",
                        turn,
                    );
                    frame["params"]["toolCall"]["rawInput"] = solo;
                    let action = super::super::approval_test_action(codec.value(frame).unwrap());
                    assert_eq!(
                        action.consent.grant_key().is_some(),
                        matches!(
                            field.as_str(),
                            "variant" | "command" | "description" | "reason" | "timeout"
                        ),
                        "grant carrying {field}"
                    );
                    let selected = if expected["description"].is_string() {
                        "description"
                    } else {
                        "reason"
                    };
                    expected
                        .as_object_mut()
                        .unwrap()
                        .retain(|key, _| key != "command" && key != selected);
                    super::super::assert_approval_preview(&action, expected, field);
                }
            }
            let decoded = codec.value(frame).unwrap();
            let action = decoded
                .events
                .iter()
                .find_map(|event| match event {
                    ProviderEvent::Turn {
                        event:
                            TurnEvent::RequestOpened {
                                kind: RequestKind::Approval { action },
                                ..
                            },
                        ..
                    } => Some(action),
                    _ => None,
                })
                .unwrap();
            assert_eq!(action.tool, variant);
            assert!(action.consent.grant_key().is_none());
            let mut expected = input;
            let preview = match &action.target {
                ApprovalTarget::Command {
                    command,
                    cwd,
                    reason,
                    preview,
                } => {
                    assert_eq!(variant, "Bash");
                    assert_eq!(command, "printf check");
                    assert_eq!(cwd, &CommandCwd::NotVerified);
                    assert_eq!(reason.as_ref().unwrap().preview.text, "Actual reason");
                    expected
                        .as_object_mut()
                        .unwrap()
                        .retain(|field, _| !matches!(field.as_str(), "command" | "reason"));
                    preview
                }
                ApprovalTarget::Tool { preview } => {
                    assert_ne!(variant, "Bash");
                    preview
                }
                other => panic!("unexpected target: {other:?}"),
            };
            assert!(!preview.truncated);
            assert_eq!(
                preview.text,
                serde_json::to_string_pretty(&expected).unwrap(),
                "{variant}"
            );
        }
    }
    #[test]
    fn grok_fixture_approval_preview_keeps_every_unmapped_field() {
        let (mut codec, turn) = boundary_codec();
        let mut frame = fixture_frame(
            include_str!("../../tests/fixtures/grok/approval-deny.jsonl"),
            |frame| frame["method"] == "session/request_permission",
            turn,
        );
        let input = json!({"variant":"Bash", "command":"printf check", "cwd":"/elsewhere/other-project",
            "description":"Description shown separately", "reason":"Additional reason",
            "env":{"CHECK":"material"}, "networkAccess":true, "dangerouslyDisableSandbox":true,
            "itemId":"an argument, not a Grok id"});
        frame["params"]["toolCall"]["rawInput"] = input.clone();
        frame["params"]["toolCall"]["toolCallId"] = json!("grok-internal-sentinel");
        let decoded = codec.value(frame).unwrap();
        let action = decoded
            .events
            .iter()
            .find_map(|event| match event {
                ProviderEvent::Turn {
                    event:
                        TurnEvent::RequestOpened {
                            kind: RequestKind::Approval { action },
                            ..
                        },
                    ..
                } => Some(action),
                _ => None,
            })
            .unwrap();
        let ApprovalTarget::Command {
            command,
            cwd,
            reason,
            preview,
        } = &action.target
        else {
            panic!("command approval")
        };
        assert_eq!(command, "printf check");
        assert_eq!(cwd, &CommandCwd::NotVerified);
        assert_eq!(
            reason.as_ref().unwrap().preview.text,
            "Description shown separately"
        );
        let mut expected = input;
        expected.as_object_mut().unwrap().remove("command");
        expected.as_object_mut().unwrap().remove("description");
        assert_eq!(
            serde_json::from_str::<Value>(&preview.text).unwrap(),
            expected
        );
        assert!(!preview.text.contains("grok-internal-sentinel"));
    }
    #[test]
    fn grok_fixture_text_chunks_keep_tool_boundaries() {
        let turn = TurnId::new();
        let prompt = turn.to_string();
        let mut codec = Codec::new(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: "/project".into(),
                native: None,
                root: false,
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
            },
            1,
            None,
            false,
        );
        codec.state = SessionState::Ready {
            native: NativeSessionId("veyra-grok-session".into()),
            active: None,
        };
        codec.start(turn, "reply".into()).unwrap();
        let source = include_str!("../../tests/fixtures/grok/tool-title.jsonl");
        let frames: Vec<Value> = source
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|record| {
                record["type"] == "emit_inbound" && record["line"]["method"] == "session/update"
            })
            .map(|record| {
                serde_json::from_str(&record["line"].to_string().replace("<id:prompt1>", &prompt))
                    .unwrap()
            })
            .collect();
        let message = frames
            .iter()
            .find(|frame| frame["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
            .unwrap();
        let first = codec.value(message.clone()).unwrap();
        let chunk = codec.value(message.clone()).unwrap();
        let boundary = codec.value(frames[0].clone()).unwrap();
        let last = codec.value(message.clone()).unwrap();
        let text_key = |decoded: &Decoded| {
            decoded
                .events
                .iter()
                .find_map(|event| match event {
                    ProviderEvent::Turn {
                        event: TurnEvent::TextDelta { key, .. },
                        ..
                    } => Some(key.clone()),
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(text_key(&first), text_key(&chunk));
        assert_ne!(text_key(&first), text_key(&last));
        assert!(boundary.events.iter().any(|event| matches!(event,
            ProviderEvent::Turn { event: TurnEvent::ItemEnded { key }, .. } if *key == text_key(&first))));
    }
    #[test]
    fn grok_wire_unknown_kinds_have_fallbacks() {
        let value = json!("future_kind");
        assert!(matches!(
            decode::<OptionKind>(value.clone()).unwrap(),
            OptionKind::Unknown
        ));
        assert!(matches!(
            decode::<UpdateKind>(value.clone()).unwrap(),
            UpdateKind::Unknown
        ));
        assert!(matches!(
            decode::<ToolStatus>(value.clone()).unwrap(),
            ToolStatus::Unknown
        ));
        assert!(matches!(
            decode::<CompletionReason>(value).unwrap(),
            CompletionReason::Unknown
        ));
    }
}
