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
impl PromptWork {
    fn owner(&self) -> ActiveTurn {
        match self {
            Self::Veyra { turn, .. } => ActiveTurn::Veyra(*turn),
            Self::Provider { turn } => ActiveTurn::Provider(*turn),
        }
    }
}
struct Prompt {
    origin: PromptOrigin,
    accepted: bool,
    first_frame: Option<Instant>,
    tool_starts: BTreeMap<String, ToolCall>,
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
#[serde(default, rename_all = "camelCase")]
struct Permission {
    session_id: String,
    tool_call: Tool,
    options: Vec<PermissionOption>,
    #[serde(rename = "_meta")]
    meta: Meta,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PermissionOption {
    option_id: String,
    kind: OptionKind,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Meta {
    prompt_index: Option<i64>,
    prompt_id: Option<String>,
    is_replay: bool,
}
#[derive(Default, Deserialize)]
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReplayFrame {
    session_id: String,
    update: ReplayUpdate,
    #[serde(rename = "_meta", default)]
    meta: Meta,
}
#[derive(Deserialize)]
struct ReplayUpdate {
    #[serde(rename = "_meta", default)]
    meta: Meta,
    #[serde(rename = "sessionUpdate")]
    kind: UpdateKind,
    #[serde(default)]
    content: Value,
    #[serde(default)]
    prompt_id: Option<String>,
}
fn replay_import(frames: &[ReplayFrame], request: &super::HistoryRequest) -> super::History {
    let mut turns = Vec::new();
    let mut user = String::new();
    let mut agent = String::new();
    let mut id = None;
    let mut failed = false;
    let mut user_index = None;
    for frame in frames {
        let update = &frame.update;
        if update.kind == UpdateKind::UserMessageChunk
            && !user.is_empty()
            && (!agent.is_empty() || user_index.is_some() && update.meta.prompt_index != user_index)
        {
            failed = true;
            user.clear();
            agent.clear();
            id = None;
        }
        if update.kind == UpdateKind::UserMessageChunk {
            user_index = update.meta.prompt_index.or(user_index);
        }
        if let Some(prompt) = update
            .prompt_id
            .as_ref()
            .or(frame.meta.prompt_id.as_ref())
            .or(update.meta.prompt_id.as_ref())
        {
            id = Some(prompt.clone());
        }
        if matches!(
            update.kind,
            UpdateKind::UserMessageChunk | UpdateKind::AgentMessageChunk
        ) {
            match decode::<Content>(update.content.clone()) {
                Ok(Content::Text { text }) if update.kind == UpdateKind::UserMessageChunk => {
                    user.push_str(&text)
                }
                Ok(Content::Text { text }) => agent.push_str(&text),
                Ok(Content::Unknown) => {}
                Err(_) => failed = true,
            }
        }
        if update.kind == UpdateKind::TurnCompleted {
            user_index = None;
            if let Some(prompt) = id.take() {
                turns.push((
                    prompt,
                    std::mem::take(&mut user),
                    std::mem::take(&mut agent),
                ));
            } else {
                failed = true;
            }
        }
    }
    let incomplete = id.is_some() || !user.is_empty() || !agent.is_empty();
    let cursor = id
        .or_else(|| turns.last().map(|turn| turn.0.clone()))
        .map(Into::into)
        .or_else(|| request.cursor.clone());
    let anchor = request
        .cursor
        .as_ref()
        .and_then(|cursor| turns.iter().position(|turn| turn.0 == **cursor));
    failed |= incomplete || request.cursor.is_some() && anchor.is_none();
    let mut text = Vec::new();
    if !failed && let Some(anchor) = anchor {
        for (id, user, agent) in turns.into_iter().skip(anchor + 1) {
            if request.sent.contains(&id) {
                continue;
            }
            if !user.is_empty() {
                text.push(user);
            }
            if !agent.is_empty() {
                text.push(agent);
            }
        }
    }
    super::History {
        cursor,
        text: text.join("\n"),
        failed,
    }
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
    replay: Vec<ReplayFrame>,
    replay_failed: bool,
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
            replay: vec![],
            replay_failed: false,
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
                tool_starts: BTreeMap::new(),
            },
        );
        self.set_work(Some(PromptWork::Veyra {
            id: id.clone(),
            turn,
        }));
        self.interrupted = None;
        Some(Decoded {
            cursor: Some(turn.to_string().into()),
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
            tool_starts: BTreeMap::new(),
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
                if matches!(self.work(), Some(PromptWork::Provider { .. })) {
                    decoded.cursor = Some(id.into());
                }
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
        if self.spec.import.is_some()
            && matches!(
                self.state,
                SessionState::Opening {
                    target: OpenTarget::Load(_),
                    ..
                }
            )
            && value.get("id").is_none()
            && tagged
                .as_ref()
                .is_some_and(|frame| frame.params.meta.is_replay)
        {
            match decode::<ReplayFrame>(value["params"].clone()) {
                Ok(frame)
                    if self
                        .spec
                        .native
                        .as_ref()
                        .is_some_and(|native| frame.session_id == native.0) =>
                {
                    self.replay.push(frame)
                }
                Ok(_) => {}
                Err(_) => self.replay_failed = true,
            }
            return Ok(decoded);
        }
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
                        if self.spec.import.is_none()
                            && init
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
                        if let Some(request) = &self.spec.import {
                            let mut history = replay_import(&self.replay, request);
                            history.failed |= self.replay_failed;
                            if history.failed {
                                history.text.clear();
                            }
                            decoded.history = Some(history);
                            self.replay.clear();
                        }
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
                        decoded.events.push(ProviderEvent::Session(SessionEvent::Notice {
                        text:"The earlier conversation could not be resumed; this is a new session".into() }));
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
                self.ended.insert(id.clone());
                if self
                    .prompts
                    .get(&id)
                    .is_some_and(|prompt| matches!(prompt.origin, PromptOrigin::Foreign))
                {
                    self.finish(&id, completed.completion.outcome(), true, decoded);
                }
            }
            return Ok(());
        }
        if notification == NotificationMethod::PromptComplete {
            let completion: Completion = decode(params)?;
            let id = scope
                .prompt_id()
                .map(str::to_owned)
                .or_else(|| match self.work() {
                    Some(PromptWork::Veyra { id, .. }) => Some(id.clone()),
                    _ => None,
                });
            if let Some(id) = id {
                self.finish(&id, completion.outcome(), true, decoded);
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
        match update {
            Update::Message(Content::Text { text }) => Self::event(
                decoded,
                owner,
                TurnEvent::TextDelta {
                    key: key("reply"),
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
                let tool_key = format!("{}/{id}/tool/{}", owner.target(), tool.tool_call_id);
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
        let parsed = match decode::<RequestMethod>(json!(method))? {
            RequestMethod::Permission => {
                let permission: Permission = decode(params.clone())?;
                if self.native().is_none_or(|n| n.0 != permission.session_id)
                    || permission.tool_call.tool_call_id.is_empty()
                {
                    None
                } else {
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
                    let exact_command = permission.tool_call.raw_input["variant"].as_str()
                        == Some("Bash")
                        && permission
                            .tool_call
                            .raw_input
                            .as_object()
                            .is_some_and(|args| {
                                args.keys().all(|key| {
                                    matches!(
                                        key.as_str(),
                                        "variant"
                                            | "command"
                                            | "description"
                                            | "is_background"
                                            | "timeout"
                                    )
                                })
                            });
                    let grant_key = command.filter(|_| exact_command).map(|command| GrantKey {
                        tool: "Bash".into(),
                        command: command.into(),
                    });
                    let target = if let Some(command) = command {
                        ApprovalTarget::Command {
                            command: command.into(),
                            cwd: if exact_command {
                                CommandCwd::ProjectFolder
                            } else {
                                CommandCwd::NotVerified
                            },
                            reason: None,
                            preview: ApprovalPreview::new(serde_json::to_string_pretty(
                                &permission.tool_call.raw_input,
                            )?),
                        }
                    } else {
                        ApprovalTarget::Tool {
                            preview: ApprovalPreview::new(serde_json::to_string_pretty(
                                &permission.tool_call.raw_input,
                            )?),
                        }
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
        let Some(_) = self.prompt(prompt_id.as_deref(), decoded) else {
            decoded
                .replies
                .push(self.peer.error(id, -32600, "Request has no active prompt"));
            return Ok(());
        };
        let owner = self.active().unwrap();
        Self::event(
            decoded,
            owner,
            TurnEvent::RequestOpened {
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

pub fn tui_command(
    native: &crate::domain::NativeSessionId,
    mode: crate::domain::PermissionMode,
) -> Vec<String> {
    let mut args = vec!["--no-auto-update".into()];
    if mode == PermissionMode::FullAccess {
        args.push("--always-approve".into());
    } else {
        args.extend(["--permission-mode".into(), "default".into()]);
    }
    args.extend(["--resume".into(), native.0.clone()]);
    args
}

#[cfg(test)]
#[test]
fn import_grok_rejects_incomplete_turn() {
    let request = super::HistoryRequest {
        cursor: Some("anchor".into()),
        ..Default::default()
    };
    let frame = |update| {
        serde_json::from_value::<ReplayFrame>(json!({"sessionId":"native", "update":update}))
            .unwrap()
    };
    let user = frame(
        json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"keep me"}}),
    );
    assert!(replay_import(std::slice::from_ref(&user), &request).failed);
    let anchor = frame(json!({"sessionUpdate":"turn_completed","prompt_id":"anchor"}));
    let complete = frame(json!({"sessionUpdate":"turn_completed","prompt_id":"terminal"}));
    assert_eq!(
        replay_import(&[anchor, user, complete], &request),
        super::History {
            failed: false,
            cursor: Some("terminal".into()),
            text: "keep me".into()
        }
    );
}

#[cfg(test)]
mod wire_tests {
    use super::*;
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

#[cfg(test)]
#[test]
fn grok_replay_ignores_arrays_outside_message_chunks() {
    let raw = |update| json!({"sessionId":"native-array", "update":update});
    let mut frames = vec![
        serde_json::from_value::<ReplayFrame>(raw(
            json!({"sessionUpdate":"turn_completed","prompt_id":"anchor"}),
        ))
        .unwrap(),
    ];
    for kind in ["tool_call", "tool_call_update", "plan", "future_update"] {
        frames.push(serde_json::from_value::<ReplayFrame>(raw(json!({"sessionUpdate":kind,"toolCallId":"native-tool","content":[{"type":"content","content":{"type":"text","text":"tool-only output"}}]}))).unwrap());
    }
    for update in [
        json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"window prompt"}}),
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"window reply"}}),
        json!({"sessionUpdate":"turn_completed","prompt_id":"terminal-turn"}),
    ] {
        frames.push(serde_json::from_value(raw(update)).unwrap());
    }
    assert_eq!(
        replay_import(
            &frames,
            &super::HistoryRequest {
                cursor: Some("anchor".into()),
                ..Default::default()
            }
        ),
        super::History {
            cursor: Some("terminal-turn".into()),
            text: "window prompt\nwindow reply".into(),
            failed: false
        }
    );
}

#[cfg(test)]
#[test]
fn grok_overlap_cursor_does_not_reimport_displayed_tail() {
    let native = NativeSessionId("overlap-native".into());
    let mut codec = Codec::new(
        SessionSpec {
            thread: ThreadId::new(),
            cwd: "/tmp".into(),
            config: ThreadConfig {
                model: None,
                effort: None,
                mode: PermissionMode::AskFirst,
            },
            native: Some(native.clone()),
            import: None,
            root: false,
        },
        1,
        None,
        false,
    );
    codec.state = SessionState::Ready {
        native: native.clone(),
        active: None,
    };
    let own = TurnId::new();
    let Outbound::Prompt(prompt) = codec
        .start(own, "own prompt".into())
        .unwrap()
        .take_outbound()
        .remove(0)
    else {
        panic!("prompt outbound");
    };
    let message = |id: &str, text: &str| json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":native.0,"_meta":{"promptId":id},"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}});
    let complete = |id: &str| json!({"jsonrpc":"2.0","method":"_x.ai/session/prompt_complete","params":{"sessionId":native.0,"promptId":id,"stopReason":"end_turn"}});
    codec
        .value(message("foreign-first", "skip before promotion"))
        .unwrap();
    codec
        .value(message("foreign-newest", "skip before promotion"))
        .unwrap();
    let promoted = codec
        .value(json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}}))
        .unwrap();
    let mut cursor = promoted
        .cursor
        .and_then(super::CursorUpdate::value)
        .or(Some(own.to_string().into()));
    let wires = [
        message("foreign-first", "first"),
        message("foreign-newest", "newest"),
        complete("foreign-newest"),
        message("foreign-first", " tail"),
        complete("foreign-first"),
    ];
    let mut replay = Vec::new();
    for wire in wires {
        let decoded = codec.value(wire.clone()).unwrap();
        if decoded.cursor.is_some() {
            cursor = decoded.cursor.and_then(super::CursorUpdate::value);
        }
        let params = if wire["method"] == "_x.ai/session/prompt_complete" {
            json!({"sessionId":native.0,"update":{"sessionUpdate":"turn_completed","prompt_id":wire["params"]["promptId"]}})
        } else {
            wire["params"].clone()
        };
        replay.push(serde_json::from_value::<ReplayFrame>(params).unwrap());
    }
    let history = replay_import(
        &replay,
        &super::HistoryRequest {
            cursor: cursor.clone(),
            ..Default::default()
        },
    );
    assert_eq!(
        history.text, "",
        "the chat already displayed first tail and newest"
    );
    assert!(!history.failed);
    assert_eq!(cursor.as_deref(), Some("foreign-first"));
}
