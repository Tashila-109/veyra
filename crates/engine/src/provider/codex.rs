use super::{
    ActiveTurn, Decoded, Outbound, ProviderError, ProviderEvent, SessionEvent, SessionSpec,
    StopReason, TurnEvent,
    jsonrpc::{self, Frame, Peer},
};
use crate::{domain::*, platform::Signal};
use serde_json::{Value, json};
mod wire;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio::time::Instant;

pub fn mode(mode: PermissionMode) -> (&'static str, &'static str, Value) {
    match mode {
        PermissionMode::AskFirst => (
            "untrusted",
            "read-only",
            json!({"type":"readOnly","networkAccess":true}),
        ),
        PermissionMode::AcceptEdits => (
            "on-request",
            "workspace-write",
            json!({"type":"workspaceWrite","networkAccess":true}),
        ),
        PermissionMode::FullAccess => (
            "never",
            "danger-full-access",
            json!({"type":"dangerFullAccess"}),
        ),
    }
}
pub(crate) struct McpConfig {
    pub command: std::path::PathBuf,
    pub socket: std::path::PathBuf,
    pub token: crate::mcp::McpToken,
}
impl McpConfig {
    pub fn value(&self, token: &str) -> Value {
        json!({
            "mcp_servers.veyra.command":self.command,
            "mcp_servers.veyra.args":["mcp"],
            "mcp_servers.veyra.env":{
                "VEYRA_SOCKET":self.socket,
                "VEYRA_MCP_TOKEN":token
            }
        })
    }
}
pub fn thread_params(spec: &SessionSpec) -> Value {
    let (approval, sandbox, _) = mode(spec.config.mode);
    let mut params = json!({
        "cwd":spec.cwd,
        "model":spec.config.model,
        "approvalPolicy":approval,
        "sandbox":sandbox,
        "approvalsReviewer":"user"
    });
    if let Some(native) = &spec.native {
        params["threadId"] = json!(native.0);
        params["excludeTurns"] = json!(true);
    }
    params
}
pub fn turn_params(native: &str, config: &ThreadConfig, text: String) -> Value {
    let (approval, _, sandbox) = mode(config.mode);
    json!({
        "threadId":native,
        "input":[{"type":"text","text":text,"text_elements":[]}],
        "model":config.model,
        "effort":config.effort,
        "summary":"auto",
        "approvalPolicy":approval,
        "approvalsReviewer":"user",
        "sandboxPolicy":sandbox
    })
}
pub fn initialize() -> Value {
    json!({"clientInfo":{"name":"veyra","title":"Veyra","version":env!("CARGO_PKG_VERSION")}})
}
#[derive(Clone, Copy)]
enum Call {
    Initialize,
    Thread,
    Start(TurnId),
    Interrupt(TurnId),
}
struct Turn {
    owner: ActiveTurn,
    started: bool,
}
enum RequestReply {
    Decision,
    Permissions(Value),
    Elicitation,
    Question(Vec<String>),
}
struct Request {
    id: Value,
    kind: RequestKind,
    reply: RequestReply,
    owner: ActiveTurn,
}
#[derive(Default)]
struct Item {
    finished: bool,
    streamed: bool,
    started: bool,
    output: String,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum InterruptRequest {
    Pending,
    Sent,
    RetryWhenStarted,
    Retried,
}
struct Interruption {
    target: TurnId,
    ladder: super::StopLadder,
    request: InterruptRequest,
}
enum Conversation {
    Opening {
        native: Option<NativeSessionId>,
        retry: Option<ResumeRetry>,
    },
    Ready(NativeSessionId),
}
struct ResumeRetry {
    next: Option<Instant>,
    until: Instant,
}
impl Conversation {
    fn native(&self) -> Option<&NativeSessionId> {
        match self {
            Self::Opening { native, .. } => native.as_ref(),
            Self::Ready(native) => Some(native),
        }
    }
}
enum TurnState {
    Idle,
    Starting {
        owner: TurnId,
        native: Option<String>,
        early: Vec<wire::Notification>,
    },
    Running {
        owner: ActiveTurn,
        native: String,
    },
}
impl TurnState {
    fn active(&self) -> Option<ActiveTurn> {
        match self {
            Self::Idle => None,
            Self::Starting { owner, .. } => Some(ActiveTurn::Veyra(*owner)),
            Self::Running { owner, .. } => Some(*owner),
        }
    }
    fn native(&self) -> Option<&str> {
        match self {
            Self::Idle => None,
            Self::Starting { native, .. } => native.as_deref(),
            Self::Running { native, .. } => Some(native),
        }
    }
}
pub(crate) struct Codec {
    peer: Peer<Call>,
    spec: SessionSpec,
    mcp: Option<std::sync::Arc<McpConfig>>,
    pid: u32,
    conversation: Conversation,
    turn: TurnState,
    turns: BTreeMap<String, Turn>,
    ended: BTreeSet<String>,
    items: BTreeMap<String, Item>,
    requests: BTreeMap<String, Request>,
    seen_requests: BTreeSet<String>,
    interrupted: Option<Interruption>,
}
impl Codec {
    pub(crate) fn new(
        mut spec: SessionSpec,
        pid: u32,
        mcp: Option<std::sync::Arc<McpConfig>>,
    ) -> Self {
        Self {
            conversation: Conversation::Opening {
                native: spec.native.take(),
                retry: None,
            },
            spec,
            pid,
            mcp,
            peer: Peer::new(false),
            turn: TurnState::Idle,
            turns: BTreeMap::new(),
            ended: BTreeSet::new(),
            items: BTreeMap::new(),
            requests: BTreeMap::new(),
            seen_requests: BTreeSet::new(),
            interrupted: None,
        }
    }
    pub(crate) fn initialize(&mut self) -> Value {
        self.peer.send(Call::Initialize, "initialize", initialize())
    }
    pub(crate) fn native(&self) -> Option<&NativeSessionId> {
        self.conversation.native()
    }
    pub(crate) fn active(&self) -> Option<ActiveTurn> {
        self.turn.active()
    }
    fn open_thread(&mut self) -> Outbound {
        let mut spec = self.spec.clone();
        spec.native = self.conversation.native().cloned();
        let line = self.peer.send(
            Call::Thread,
            if spec.native.is_some() {
                "thread/resume"
            } else {
                "thread/start"
            },
            thread_params(&spec),
        );
        match &self.mcp {
            Some(mcp) => Outbound::CodexThread {
                line,
                mcp: mcp.clone(),
            },
            None => Outbound::Message(line),
        }
    }
    fn queue_thread(&mut self, decoded: &mut Decoded) {
        match self.open_thread() {
            Outbound::Message(line) => decoded.replies.push(line),
            frame => decoded.controls.push(frame),
        }
    }
    pub(crate) fn start(&mut self, turn: TurnId, text: String) -> Option<Outbound> {
        if self.active().is_some() {
            return None;
        }
        let native = self.native()?.0.clone();
        self.turn = TurnState::Starting {
            owner: turn,
            native: None,
            early: vec![],
        };
        self.interrupted = None;
        Some(Outbound::Prompt(self.peer.send(
            Call::Start(turn),
            "turn/start",
            turn_params(&native, &self.spec.config, text),
        )))
    }
    pub(crate) fn value(&mut self, value: Value) -> Result<Decoded, ProviderError> {
        let mut decoded = Decoded::default();
        match self.peer.receive(value) {
            Frame::Reply {
                tag: Call::Initialize,
                result,
            } => {
                let _: wire::InitializeReply =
                    decode(result.map_err(jsonrpc::failure)?, "initialize reply")?;
                decoded
                    .replies
                    .push(self.peer.notification("initialized", Value::Null));
                self.queue_thread(&mut decoded);
            }
            Frame::Reply {
                tag: Call::Thread,
                result,
            } => match result {
                Ok(result) => {
                    let reply: wire::ThreadReply = decode(result, "thread reply")?;
                    let native = reply.thread.id.filter(|id| !id.is_empty()).ok_or_else(|| {
                        ProviderError::Protocol("Codex thread reply has no thread id".into())
                    })?;
                    self.conversation = Conversation::Ready(NativeSessionId(native.clone()));
                    self.mcp = None;
                    decoded
                        .events
                        .push(ProviderEvent::Session(SessionEvent::Ready {
                            native: NativeSessionId(native),
                            cli_version: reply.thread.cli_version.filter(|v| !v.is_empty()),
                            pid: self.pid,
                            pgid: self.pid,
                            capabilities: vec![],
                        }));
                }
                Err(error) if self.native().is_some() && not_found(&error.message) => {
                    self.conversation = Conversation::Opening {
                        native: None,
                        retry: None,
                    };
                    decoded.events.push(Self::notice(
                        "The earlier conversation could not be resumed; this is a new session",
                    ));
                    self.queue_thread(&mut decoded);
                }
                Err(error)
                    if self.native().is_some()
                        && error.message.contains("already has an active writer") =>
                {
                    let Conversation::Opening { retry, .. } = &mut self.conversation else {
                        return Err(jsonrpc::failure(error));
                    };
                    let retry = retry.get_or_insert_with(|| ResumeRetry {
                        next: None,
                        until: Instant::now() + Duration::from_secs(3),
                    });
                    if Instant::now() >= retry.until {
                        return Err(jsonrpc::failure(error));
                    }
                    retry.next =
                        Some((Instant::now() + Duration::from_millis(250)).min(retry.until));
                }
                Err(error) => return Err(jsonrpc::failure(error)),
            },
            Frame::Reply {
                tag: Call::Start(turn),
                result,
            } => {
                let reply: wire::TurnReply =
                    decode(result.map_err(jsonrpc::failure)?, "turn reply")?;
                let id = reply.turn.id.filter(|id| !id.is_empty()).ok_or_else(|| {
                    ProviderError::Protocol("Codex turn reply has no turn id".into())
                })?;
                let prior = std::mem::replace(
                    &mut self.turn,
                    TurnState::Running {
                        owner: ActiveTurn::Veyra(turn),
                        native: id.clone(),
                    },
                );
                let early = match prior {
                    TurnState::Starting { native, early, .. } => {
                        if native.as_ref().is_some_and(|native| native != &id) {
                            return Err(ProviderError::Protocol(
                                "Codex turn reply changed the pending turn id".into(),
                            ));
                        }
                        early
                    }
                    _ => vec![],
                };
                self.turns.entry(id).or_insert(Turn {
                    owner: ActiveTurn::Veyra(turn),
                    started: false,
                });
                decoded.events.push(ProviderEvent::Turn {
                    turn: Some(turn),
                    event: TurnEvent::Accepted,
                });
                for notification in early {
                    self.notification(notification, &mut decoded)?;
                }
                if self.active().is_some()
                    && self.interrupted.is_some()
                    && !self.peer_has_interrupt()
                {
                    decoded.controls.push(self.interrupt_frame(turn));
                }
            }
            Frame::Reply {
                tag: Call::Interrupt(target),
                result,
            } => {
                if let Err(error) = result {
                    let before_started = self
                        .turn
                        .native()
                        .and_then(|id| self.turns.get(id))
                        .is_some_and(|turn| !turn.started);
                    if error.code == -32600
                        && error
                            .message
                            .to_lowercase()
                            .contains("no active turn to interrupt")
                        && before_started
                    {
                        if let Some(interrupt) = &mut self.interrupted
                            && interrupt.target == target
                            && interrupt.request != InterruptRequest::Retried
                        {
                            interrupt.request = InterruptRequest::RetryWhenStarted;
                        }
                    } else if self
                        .active()
                        .is_some_and(|active| active.target() == target)
                    {
                        return Err(jsonrpc::failure(error));
                    }
                }
            }
            Frame::Request { id, method, params } => {
                let request = serde_json::from_value(json!({"method":method,"params":params}));
                match request {
                    Ok(request) => self.request(id, request, params, &mut decoded),
                    Err(_) => decoded.replies.push(self.peer.error(
                        id,
                        -32602,
                        "Malformed server request",
                    )),
                }
            }
            Frame::Notification { method, params } => {
                let notification =
                    decode(json!({"method":method,"params":params}), "notification")?;
                self.notification(notification, &mut decoded)?;
            }
            Frame::Unknown => tracing::info!("ignored unrecognized Codex frame"),
        }
        Ok(decoded)
    }
    fn peer_has_interrupt(&self) -> bool {
        self.interrupted
            .as_ref()
            .is_some_and(|i| i.request != InterruptRequest::Pending)
    }
    fn own_thread(&self, scope: &wire::Scope) -> bool {
        scope
            .thread_id
            .as_deref()
            .is_some_and(|id| self.native().is_some_and(|native| native.0 == id))
    }
    fn route(&mut self, id: &str, decoded: &mut Decoded) -> Option<ActiveTurn> {
        if self.ended.contains(id) {
            return None;
        }
        if let Some(turn) = self.turns.get(id) {
            return Some(turn.owner);
        }
        let owner = match &mut self.turn {
            TurnState::Idle => {
                let owner = ActiveTurn::Provider(TurnId::new());
                decoded.provider_turn = Some(owner.target());
                self.turn = TurnState::Running {
                    owner,
                    native: id.into(),
                };
                owner
            }
            TurnState::Starting { owner, native, .. }
                if native.as_ref().is_none_or(|native| native == id) =>
            {
                *native = Some(id.into());
                ActiveTurn::Veyra(*owner)
            }
            _ => return None,
        };
        self.turns.insert(
            id.into(),
            Turn {
                owner,
                started: false,
            },
        );
        Some(owner)
    }
    fn notification(
        &mut self,
        notification: wire::Notification,
        decoded: &mut Decoded,
    ) -> Result<(), ProviderError> {
        use wire::Notification as N;
        match &notification {
            N::RateLimits { params: details } => {
                let _ = details;
                decoded.events.push(Self::notice(
                    "Codex updated its usage limits. Check the CLI for the remaining allowance.",
                ));
                return Ok(());
            }
            N::Error { params: error } => {
                if error.will_retry
                    && (error.scope.thread_id.is_none() || self.own_thread(&error.scope))
                {
                    decoded
                        .events
                        .push(Self::notice("Codex is retrying after a provider error."));
                }
                return Ok(());
            }
            N::Unknown => return Ok(()),
            _ => {}
        }
        if notification
            .scope()
            .is_none_or(|scope| !self.own_thread(scope))
        {
            return Ok(());
        }
        let Some(id) = notification
            .turn_id()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            return Ok(());
        };
        if let TurnState::Starting { early, .. } = &mut self.turn {
            early.push(notification);
            return Ok(());
        }
        let owner = if matches!(
            &notification,
            N::TurnCompleted { params: _ } | N::CommandOutput { params: _ }
        ) {
            self.turns.get(&id).map(|turn| turn.owner)
        } else {
            self.route(&id, decoded)
        };
        let Some(owner) = owner else {
            return Ok(());
        };
        if self.ended.contains(&id) {
            return Ok(());
        }
        let mut events = Vec::new();
        let agent = matches!(&notification, N::AgentDelta { params: _ });
        match notification {
            N::TurnStarted { params: _ } => {
                if let Some(turn) = self.turns.get_mut(&id)
                    && !turn.started
                {
                    turn.started = true;
                    events.push(TurnEvent::Started);
                }
                let retry = self.interrupted.as_mut().is_some_and(|i| {
                    if i.target == owner.target() && i.request == InterruptRequest::RetryWhenStarted
                    {
                        i.request = InterruptRequest::Retried;
                        true
                    } else {
                        false
                    }
                });
                if retry {
                    decoded.controls.push(self.interrupt_frame(owner.target()));
                }
            }
            N::TurnCompleted { params } => {
                let outcome = match params.turn.status {
                    wire::TurnStatus::Interrupted => TurnOutcome::Interrupted {
                        reason: "Stopped by Codex".into(),
                    },
                    wire::TurnStatus::Failed => TurnOutcome::Failed {
                        message: crate::record::redact(
                            params
                                .turn
                                .error
                                .as_ref()
                                .and_then(|error| error.message.as_deref())
                                .unwrap_or("Codex could not finish this turn"),
                            None,
                        ),
                    },
                    _ => TurnOutcome::Completed,
                };
                let outcome = self.end_outcome(outcome);
                self.ended.insert(id.clone());
                self.requests
                    .retain(|_, request| request.owner.target() != owner.target());
                if self
                    .active()
                    .is_some_and(|active| active.target() == owner.target())
                {
                    self.turn = TurnState::Idle;
                    self.interrupted = None;
                }
                events.push(TurnEvent::TurnEnded {
                    outcome,
                    park: false,
                });
            }
            N::AgentDelta { params }
            | N::ReasoningSummaryDelta { params }
            | N::ReasoningDelta { params } => {
                if let Some(item) = params.item_id.filter(|id| !id.is_empty()) {
                    let key = Self::key(&id, &item);
                    self.items.entry(key.0.clone()).or_default().streamed = true;
                    let text = params.delta.unwrap_or_default();
                    events.push(if agent {
                        TurnEvent::TextDelta {
                            key,
                            text,
                            streamed: true,
                        }
                    } else {
                        TurnEvent::ReasoningDelta { key, text }
                    });
                }
            }
            N::CommandOutput { params } => {
                if let Some(item) = params.item_id.filter(|id| !id.is_empty()) {
                    let key = Self::key(&id, &item);
                    let text = params.delta.unwrap_or_default();
                    let state = self.items.entry(key.0.clone()).or_default();
                    state.output.push_str(&text);
                    let chars = state.output.chars().count();
                    if chars > TOOL_OUTPUT_CHARS {
                        state.output = state
                            .output
                            .chars()
                            .skip(chars - TOOL_OUTPUT_CHARS)
                            .collect();
                    }
                    events.push(TurnEvent::ToolOutput { key, text });
                }
            }
            N::ItemStarted { params } => self.item(&id, &params.item, false, &mut events),
            N::ItemCompleted { params } => self.item(&id, &params.item, true, &mut events),
            N::PlanUpdated { params } => {
                let plan = params
                    .plan
                    .into_iter()
                    .filter_map(|step| {
                        Some(PlanStep {
                            text: step.step?,
                            done: matches!(step.status, wire::PlanStatus::Completed),
                        })
                    })
                    .collect();
                events.push(TurnEvent::Plan(plan));
            }
            N::RateLimits { params: _ } | N::Error { params: _ } | N::Unknown => {}
        }
        decoded
            .events
            .extend(events.into_iter().map(|event| ProviderEvent::Turn {
                turn: owner.id(),
                event,
            }));
        Ok(())
    }
    fn key(turn: &str, item: &str) -> NativeItemKey {
        NativeItemKey(format!("{turn}:{item}"))
    }
    fn item(
        &mut self,
        turn: &str,
        wire: &wire::Item,
        completed: bool,
        events: &mut Vec<TurnEvent>,
    ) {
        let Some(id) = wire.id().filter(|id| !id.is_empty()) else {
            return;
        };
        let key = Self::key(turn, id);
        let state = self.items.entry(key.0.clone()).or_default();
        if state.finished {
            return;
        }
        state.finished = completed;
        match wire {
            wire::Item::AgentMessage(message) if completed => {
                if !state.streamed {
                    events.push(TurnEvent::TextDelta {
                        key: key.clone(),
                        text: message.text.clone().unwrap_or_default(),
                        streamed: false,
                    });
                }
                events.push(TurnEvent::ItemEnded { key });
            }
            wire::Item::Reasoning(reasoning) if completed => {
                if !state.streamed {
                    let text = reasoning.summary.join("\n");
                    if !text.is_empty() {
                        events.push(TurnEvent::ReasoningDelta {
                            key: key.clone(),
                            text,
                        });
                    }
                }
                events.push(TurnEvent::ItemEnded { key });
            }
            item if item.tool().is_some() => {
                let tool = item.tool().unwrap();
                if !state.started {
                    state.started = true;
                    let (name, arguments, summary) = match item {
                        wire::Item::CommandExecution(command) => (
                            "Command".into(),
                            json!({"command":command.command,"cwd":command.cwd}),
                            command.command.clone(),
                        ),
                        wire::Item::FileChange(file) => {
                            ("File change".into(), json!({"changes":file.changes}), None)
                        }
                        wire::Item::McpToolCall(call) => (
                            format!(
                                "{} / {}",
                                call.server.as_deref().unwrap_or("MCP"),
                                call.tool_name.as_deref().unwrap_or("tool")
                            ),
                            call.arguments.clone().unwrap_or(Value::Null),
                            None,
                        ),
                        wire::Item::CollabAgentToolCall(_) => (
                            "collabAgentToolCall".into(),
                            serde_json::to_value(item).unwrap(),
                            None,
                        ),
                        wire::Item::WebSearch(_) => (
                            "webSearch".into(),
                            serde_json::to_value(item).unwrap(),
                            None,
                        ),
                        wire::Item::ImageView(_) => (
                            "imageView".into(),
                            serde_json::to_value(item).unwrap(),
                            None,
                        ),
                        _ => return,
                    };
                    events.push(TurnEvent::ToolStarted {
                        key: key.clone(),
                        call: ToolCall {
                            name,
                            arguments,
                            input_summary: summary,
                        },
                    });
                }
                if completed {
                    let output = tool
                        .aggregated_output
                        .clone()
                        .or_else(|| (!state.output.is_empty()).then(|| state.output.clone()))
                        .or_else(|| {
                            tool.result
                                .as_ref()
                                .filter(|value| !value.is_null())
                                .map(Value::to_string)
                        });
                    let ok = !matches!(
                        tool.status,
                        wire::ToolStatus::Failed | wire::ToolStatus::Declined
                    ) && tool.exit_code.is_none_or(|code| code == 0);
                    events.push(TurnEvent::ToolFinished {
                        key,
                        ok,
                        output_tail: output,
                    });
                }
            }
            _ => {}
        }
    }
    fn request(&mut self, id: Value, request: wire::Request, params: Value, decoded: &mut Decoded) {
        let key = id.to_string();
        if !self.seen_requests.insert(key.clone()) {
            decoded
                .replies
                .push(self.peer.error(id, -32600, "Request id was already used"));
            return;
        }
        let Some(scope) = request.scope() else {
            decoded
                .replies
                .push(self.peer.error(id, -32601, "Unsupported server request"));
            return;
        };
        let native_turn = scope
            .turn_id
            .as_deref()
            .or(self.turn.native())
            .map(str::to_owned);
        if !self.own_thread(scope)
            || native_turn
                .as_ref()
                .is_some_and(|turn| turn.is_empty() || self.ended.contains(turn))
        {
            decoded.replies.push(self.peer.error(
                id,
                -32600,
                "Request is not for this thread's active turn",
            ));
            return;
        }
        let preview =
            || ApprovalPreview::new(serde_json::to_string_pretty(&params).unwrap_or_default());
        let approval = |tool: String, target| RequestKind::Approval {
            action: ProposedAction { tool, target },
            grant_key: None,
        };
        let (kind, reply) = match request {
            wire::Request::Command { params: command } => {
                let Some(text) = command.command.as_deref().filter(|text| !text.is_empty()) else {
                    decoded.replies.push(self.peer.error(
                        id,
                        -32602,
                        "Command approval has no command",
                    ));
                    return;
                };
                let in_project = command
                    .cwd
                    .as_deref()
                    .is_some_and(|cwd| Some(cwd) == self.spec.cwd.to_str());
                let location = if in_project {
                    CommandCwd::ProjectFolder
                } else if let Some(path) = command.cwd.as_deref().filter(|path| !path.is_empty()) {
                    CommandCwd::Directory(path.into())
                } else {
                    CommandCwd::NotVerified
                };
                (
                    RequestKind::Approval {
                        action: ProposedAction {
                            tool: "Command".into(),
                            target: ApprovalTarget::Command {
                                command: text.into(),
                                cwd: location,
                                preview: preview(),
                            },
                        },
                        grant_key: in_project.then(|| GrantKey {
                            tool: "Command".into(),
                            command: text.into(),
                        }),
                    },
                    RequestReply::Decision,
                )
            }
            wire::Request::File { params: _ } => (
                approval(
                    "File change".into(),
                    ApprovalTarget::Tool { preview: preview() },
                ),
                RequestReply::Decision,
            ),
            wire::Request::Permissions { params: profile } => {
                let Some(profile) = profile.permissions else {
                    decoded.replies.push(self.peer.error(
                        id,
                        -32602,
                        "Permissions approval has no profile",
                    ));
                    return;
                };
                (
                    approval(
                        "Permissions".into(),
                        ApprovalTarget::Tool { preview: preview() },
                    ),
                    RequestReply::Permissions(serde_json::to_value(profile).unwrap()),
                )
            }
            wire::Request::UserInput { params: input } => {
                let Some((questions, ids)) = questions(&input) else {
                    decoded
                        .replies
                        .push(self.peer.error(id, -32602, "Invalid questions"));
                    return;
                };
                (
                    RequestKind::Question { questions },
                    RequestReply::Question(ids),
                )
            }
            wire::Request::Elicitation {
                params: elicitation,
            } => {
                let Some(tool) = elicitation_tool(&elicitation) else {
                    decoded
                        .replies
                        .push(self.peer.response(id, json!({"action":"decline"})));
                    decoded.events.push(Self::notice(
                        "Codex requested an MCP form that Veyra cannot display; it was declined.",
                    ));
                    return;
                };
                (
                    approval(
                        format!(
                            "{} / {tool}",
                            elicitation.server_name.as_deref().unwrap_or("MCP")
                        ),
                        ApprovalTarget::Tool { preview: preview() },
                    ),
                    RequestReply::Elicitation,
                )
            }
            wire::Request::Unknown => return,
        };
        let owner = match native_turn {
            Some(native) => self.route(&native, decoded),
            None => self.active(),
        };
        let Some(owner) = owner else {
            decoded.replies.push(self.peer.error(
                id,
                -32600,
                "Request is not for this thread's active turn",
            ));
            return;
        };
        if self
            .interrupted
            .as_ref()
            .is_some_and(|i| i.target == owner.target())
        {
            decoded.replies.push(self.peer.response(id, cancel(&reply)));
            if self.turn.native().is_some() && !self.peer_has_interrupt() {
                decoded.controls.push(self.interrupt_frame(owner.target()));
            }
            return;
        }
        self.requests.insert(
            key.clone(),
            Request {
                id,
                kind: kind.clone(),
                reply,
                owner,
            },
        );
        decoded.events.push(ProviderEvent::Turn {
            turn: owner.id(),
            event: TurnEvent::RequestOpened {
                native: NativeRequestRef(key),
                kind,
            },
        });
    }
    pub(crate) fn answer(&mut self, native: NativeRequestRef, decision: Decision) -> Option<Value> {
        let request = self.requests.get(&native.0)?;
        let accepted = request.kind.accepts(&decision)?;
        let response = match (&request.reply, accepted) {
            (RequestReply::Decision, AcceptedDecision::Allow { .. }) => {
                json!({"decision":"accept"})
            }
            (RequestReply::Decision, AcceptedDecision::Deny) => json!({"decision":"decline"}),
            (RequestReply::Permissions(profile), AcceptedDecision::Allow { .. }) => {
                json!({"permissions":profile,"scope":"turn"})
            }
            (RequestReply::Permissions(_), AcceptedDecision::Deny) => json!({"permissions":{}}),
            (RequestReply::Elicitation, AcceptedDecision::Allow { .. }) => {
                json!({"action":"accept"})
            }
            (RequestReply::Elicitation, AcceptedDecision::Deny) => json!({"action":"decline"}),
            (RequestReply::Question(ids), AcceptedDecision::Answers { answers, .. }) => {
                let answers: serde_json::Map<_, _> = ids
                    .iter()
                    .zip(answers)
                    .map(|(id, answer)| (id.clone(), json!({"answers":[answer.text]})))
                    .collect();
                json!({"answers":answers})
            }
            _ => return None,
        };
        let request = self.requests.remove(&native.0)?;
        Some(self.peer.response(request.id, response))
    }
    fn interrupt_frame(&mut self, target: TurnId) -> Outbound {
        if let Some(interrupt) = &mut self.interrupted
            && interrupt.request != InterruptRequest::Retried
        {
            interrupt.request = InterruptRequest::Sent;
        }
        Outbound::Interrupt {
            turn: target,
            line: self.peer.send(
                Call::Interrupt(target),
                "turn/interrupt",
                json!({"threadId":self.native().map(|n|&n.0),"turnId":self.turn.native()}),
            ),
        }
    }
    pub(crate) fn interrupt_lines(&mut self, target: TurnId, reason: StopReason) -> Vec<Outbound> {
        if !self
            .active()
            .is_some_and(|active| active.target() == target)
            || self.interrupted.is_some()
        {
            return vec![];
        }
        self.interrupted = Some(Interruption {
            target,
            ladder: super::StopLadder::new(reason),
            request: InterruptRequest::Pending,
        });
        let mut lines: Vec<_> = std::mem::take(&mut self.requests)
            .into_values()
            .map(|r| Outbound::Message(self.peer.response(r.id, cancel(&r.reply))))
            .collect();
        if self.turn.native().is_some() {
            lines.push(self.interrupt_frame(target));
        }
        lines
    }
    pub(crate) fn interrupt_active(&mut self, reason: StopReason) -> Vec<Outbound> {
        self.active()
            .map(|turn| self.interrupt_lines(turn.target(), reason))
            .unwrap_or_default()
    }
    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        let ladder = self.interrupted.as_ref().map(|i| i.ladder.deadline());
        let retry = match &self.conversation {
            Conversation::Opening { retry, .. } => retry.as_ref().and_then(|retry| retry.next),
            Conversation::Ready(_) => None,
        };
        ladder.into_iter().chain(retry).min()
    }
    pub(crate) fn timer_lines(&mut self) -> Vec<Outbound> {
        if let Conversation::Opening {
            retry: Some(retry), ..
        } = &mut self.conversation
            && retry.next.is_some_and(|at| Instant::now() >= at)
        {
            retry.next = None;
            return vec![self.open_thread()];
        }
        vec![]
    }
    pub(crate) fn escalate_interrupt(&mut self) -> Option<Signal> {
        self.interrupted.as_mut()?.ladder.escalate()
    }
    pub(crate) fn end_outcome(&self, fallback: TurnOutcome) -> TurnOutcome {
        self.interrupted
            .as_ref()
            .map_or(fallback, |i| i.ladder.outcome())
    }
    pub(crate) fn eof(&mut self) {
        self.peer.eof("Codex JSON-RPC stdout closed");
    }
    fn notice(text: &str) -> ProviderEvent {
        ProviderEvent::Session(SessionEvent::Notice { text: text.into() })
    }
}
fn cancel(reply: &RequestReply) -> Value {
    match reply {
        RequestReply::Decision => json!({"decision":"cancel"}),
        RequestReply::Permissions(_) => json!({"permissions":{}}),
        RequestReply::Elicitation => json!({"action":"cancel"}),
        RequestReply::Question(_) => json!({"answers":{}}),
    }
}
fn not_found(message: &str) -> bool {
    message
        .to_lowercase()
        .contains("no rollout found for thread id")
}
fn decode<T: serde::de::DeserializeOwned>(value: Value, kind: &str) -> Result<T, ProviderError> {
    serde_json::from_value(value)
        .map_err(|_| ProviderError::Protocol(format!("Codex {kind} is malformed")))
}
fn elicitation_tool(params: &wire::Elicitation) -> Option<String> {
    let server = params.server_name.as_deref().filter(|s| !s.is_empty())?;
    let keyed = params.meta.as_ref().is_some_and(|meta| {
        matches!(
            meta.codex_approval_kind,
            Some(wire::ApprovalKind::McpToolCall)
        )
    });
    let parsed = params.message.as_deref().and_then(|message| {
        let prefix = format!("Allow the {server} MCP server to run tool ");
        let rest = message.strip_prefix(&prefix)?;
        let tool = rest
            .split_whitespace()
            .next()?
            .trim_matches(['`', '\'', '"', '.', '?']);
        (!tool.is_empty()).then(|| tool.to_owned())
    });
    if keyed {
        Some(parsed.unwrap_or_else(|| "tool call".into()))
    } else {
        parsed
    }
}
fn questions(params: &wire::UserInput) -> Option<(Vec<Question>, Vec<String>)> {
    let mut ids = BTreeSet::new();
    let mut texts = BTreeSet::new();
    let mut result = Vec::new();
    let mut ordered_ids = Vec::new();
    for question in params.questions.as_ref()? {
        let id = question.id.as_deref().filter(|id| !id.is_empty())?;
        let text = question
            .question
            .as_deref()
            .filter(|text| !text.is_empty())?;
        if !ids.insert(id) || !texts.insert(text) {
            return None;
        }
        let options = question
            .options
            .as_ref()
            .map(|options| {
                options
                    .iter()
                    .map(|o| o.label.as_ref().filter(|text| !text.is_empty()).cloned())
                    .collect::<Option<Vec<_>>>()
            })
            .unwrap_or_else(|| Some(vec![]))?;
        result.push(Question {
            text: text.into(),
            options,
        });
        ordered_ids.push(id.into());
    }
    (!result.is_empty()).then_some((result, ordered_ids))
}

#[derive(Clone, Copy)]
enum ProbeCall {
    Initialize,
    Account,
    Models,
}
pub(super) async fn probe_exchange(
    reader: &mut jsonrpc::Reader,
    writer: &tokio::sync::mpsc::UnboundedSender<Outbound>,
    binary: std::path::PathBuf,
) -> Result<super::Probe, ProviderError> {
    let send = |frame| {
        writer
            .send(Outbound::Message(frame))
            .map_err(|_| ProviderError::Protocol("Codex probe writer closed".into()))
    };
    let mut peer = Peer::new(false);
    send(peer.send(ProbeCall::Initialize, "initialize", initialize()))?;
    let mut version = None;
    let mut account = None;
    let mut readiness = super::Readiness::Ready;
    let mut models = vec![];
    let mut waiting = 2;
    while let Some(line) = reader.next().await? {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match peer.receive(value) {
            Frame::Reply { tag, result } => {
                let value = result.map_err(jsonrpc::failure)?;
                match tag {
                    ProbeCall::Initialize => {
                        let reply: wire::InitializeReply = decode(value, "probe initialize reply")?;
                        version = reply
                            .user_agent
                            .as_deref()
                            .and_then(|s| s.split_whitespace().next())
                            .and_then(|s| s.rsplit_once('/'))
                            .map(|(_, version)| version.to_owned());
                        send(peer.notification("initialized", Value::Null))?;
                        send(peer.send(
                            ProbeCall::Account,
                            "account/read",
                            json!({"refreshToken":false}),
                        ))?;
                        send(peer.send(ProbeCall::Models, "model/list", json!({})))?;
                    }
                    ProbeCall::Account => {
                        let reply: wire::AccountReply = decode(value, "account reply")?;
                        match reply.account {
                            wire::Account::Null if reply.requires_openai_auth => {
                                readiness = super::Readiness::SignedOut
                            }
                            wire::Account::Known(value) => account = Some(value),
                            _ => {}
                        }
                        waiting -= 1;
                    }
                    ProbeCall::Models => {
                        let reply: wire::ModelsReply = decode(value, "model list reply")?;
                        models.extend(reply.data.into_iter().filter_map(|model| {
                            Some(super::Model {
                                value: model.model.or(model.id)?,
                                display_name: model.display_name.unwrap_or_default(),
                                effort_levels: model
                                    .supported_reasoning_efforts
                                    .into_iter()
                                    .filter_map(|e| e.reasoning_effort)
                                    .collect(),
                            })
                        }));
                        if let Some(cursor) = reply.next_cursor {
                            send(peer.send(
                                ProbeCall::Models,
                                "model/list",
                                json!({"cursor":cursor}),
                            ))?;
                        } else {
                            waiting -= 1;
                        }
                    }
                }
                if waiting == 0 {
                    return Ok(super::Probe {
                        provider: super::ProviderKind::Codex,
                        path: Some(binary),
                        version,
                        readiness,
                        models,
                        account,
                    });
                }
            }
            Frame::Request { id, .. } => {
                send(peer.error(id, -32601, "Unsupported server request"))?
            }
            Frame::Notification { .. } | Frame::Unknown => {}
        }
    }
    peer.eof("Codex probe stdout closed");
    Err(ProviderError::Protocol(
        "Codex probe ended without a reply".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resumed() -> Codec {
        let mut codec = Codec::new(
            SessionSpec {
                thread: ThreadId::new(),
                cwd: "/project".into(),
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: Some(NativeSessionId("old-thread".into())),
                import: false,
                root: false,
            },
            1,
            None,
        );
        codec.initialize();
        codec.value(json!({"id":1,"result":{}})).unwrap();
        codec
    }
    #[test]
    fn codex_other_resume_errors_preserve_native() {
        let mut codec = resumed();
        assert_eq!(
            codec
                .value(json!({"id":2,"error":{"code":-32600,"message":"invalid session id"}}))
                .err()
                .unwrap()
                .to_string(),
            "invalid session id"
        );
        assert_eq!(
            codec.native().cloned(),
            Some(NativeSessionId("old-thread".into()))
        );
        let mut codec = resumed();
        let d=codec.value(json!({"id":2,"error":{"code":-32600,"message":"no rollout found for thread id old-thread"}})).unwrap();
        assert_eq!(d.replies[0]["method"], "thread/start");
        assert_eq!(codec.native().cloned(), None);
    }
    #[tokio::test(start_paused = true)]
    async fn codex_active_writer_retry_is_bounded() {
        let mut codec = resumed();
        let start = Instant::now();
        let mut id = 2;
        loop {
            let result=codec.value(json!({"id":id,"error":{"code":-32600,"message":"thread old-thread already has an active writer"}}));
            if let Err(error) = result {
                assert_eq!(
                    error.to_string(),
                    "thread old-thread already has an active writer"
                );
                break;
            }
            tokio::time::advance(Duration::from_millis(250)).await;
            let frames = codec.timer_lines();
            assert_eq!(frames.len(), 1);
            assert_eq!(frames[0].value()["method"], "thread/resume");
            id += 1;
        }
        assert_eq!(Instant::now() - start, Duration::from_secs(3));
        assert_eq!(id, 14);
        assert_eq!(
            codec.native().cloned(),
            Some(NativeSessionId("old-thread".into()))
        );
    }
    #[test]
    fn codex_ignored_notifications_do_not_open_turns() {
        let mut codec = resumed();
        codec
            .value(json!({"id":2,"result":{"thread":{"id":"old-thread"}}}))
            .unwrap();
        for method in [
            "remoteControl/status/changed",
            "account/updated",
            "hook/started",
            "hook/completed",
            "mcpServer/startupStatus/updated",
            "thread/tokenUsage/updated",
            "serverRequest/resolved",
            "thread/status/changed",
            "veyra/future",
        ] {
            let d=codec.value(json!({"method":method,"params":{"threadId":"old-thread","turnId":"stray-turn","status":{"type":"active"}},"emittedAtMs":1})).unwrap();
            assert!(d.events.is_empty());
            assert!(d.provider_turn.is_none());
        }
        let d=codec.value(json!({"method":"turn/started","params":{"threadId":"old-thread","turn":{"id":"cli-turn"}}})).unwrap();
        assert!(d.provider_turn.is_some());
        assert!(matches!(
            d.events.as_slice(),
            [ProviderEvent::Turn {
                event: TurnEvent::Started,
                ..
            }]
        ));
    }
    #[test]
    fn codex_elicitation_matches_key_and_quoted_or_unquoted_message() {
        for message in [
            "Allow the veyra MCP server to run tool list_providers",
            "Allow the veyra MCP server to run tool \"list_providers\"?",
        ] {
            for meta in [
                Value::Null,
                json!({"codex_approval_kind":"mcp_tool_call"}),
                json!({"codex_approval_kind":"future"}),
            ] {
                assert_eq!(
                    elicitation_tool(
                        &serde_json::from_value(json!({"serverName":"veyra","message":message,"mode":"form","_meta":meta})).unwrap()
                    ),
                    Some("list_providers".into())
                );
            }
        }
        assert_eq!(
            elicitation_tool(
                &serde_json::from_value(
                    json!({"serverName":"veyra","message":"Select a color","mode":"form"})
                )
                .unwrap()
            ),
            None
        );
    }
    fn early_request_codec() -> Codec {
        let mut codec = resumed();
        codec
            .value(json!({"id":2,"result":{"thread":{"id":"old-thread"}}}))
            .unwrap();
        codec
    }
    fn early_approval(turn: &str) -> Value {
        json!({"id":0,"method":"item/commandExecution/requestApproval",
            "params":{"threadId":"old-thread","turnId":turn,"itemId":"command","command":"touch test-file","cwd":"/project"}})
    }
    fn request_count(decoded: &Decoded) -> usize {
        decoded
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ProviderEvent::Turn {
                        event: TurnEvent::RequestOpened { .. },
                        ..
                    }
                )
            })
            .count()
    }
    #[test]
    fn codex_request_waiting_for_start_reply_can_be_answered() {
        let mut codec = early_request_codec();
        let turn = TurnId::new();
        codec.start(turn, "work".into()).unwrap();
        let opened = codec.value(early_approval("native-turn")).unwrap();
        assert_eq!(
            request_count(&opened),
            1,
            "a provider waiting for consent before its start reply must progress"
        );
        assert!(opened.events.iter().any(|e| matches!(e,ProviderEvent::Turn {turn:Some(id),event:TurnEvent::RequestOpened{..}} if *id==turn)));
        assert_eq!(
            codec
                .answer(NativeRequestRef("0".into()), Decision::Deny)
                .unwrap(),
            json!({"id":0,"result":{"decision":"decline"}})
        );
    }
    #[test]
    fn codex_cli_request_without_started_opens_provider() {
        let mut codec = early_request_codec();
        let opened = codec.value(early_approval("cli-turn")).unwrap();
        assert_eq!(request_count(&opened), 1);
        assert!(opened.provider_turn.is_some());
        assert_eq!(
            codec
                .answer(NativeRequestRef("0".into()), Decision::AllowOnce)
                .unwrap(),
            json!({"id":0,"result":{"decision":"accept"}})
        );
        let mut foreign = early_approval("foreign-turn");
        foreign["id"] = json!(1);
        foreign["params"]["threadId"] = json!("another-thread");
        assert_eq!(request_count(&codec.value(foreign).unwrap()), 0);
    }
    #[tokio::test]
    async fn codex_stop_orders_cancel_before_interrupt() {
        let mut codec = early_request_codec();
        let turn = TurnId::new();
        codec.start(turn, "work".into()).unwrap();
        codec.value(early_approval("native-turn")).unwrap();
        let mut frames: Vec<Value> = codec
            .interrupt_lines(turn, StopReason::User)
            .iter()
            .map(Outbound::value)
            .collect();
        let decoded = codec
            .value(json!({"id":3,"result":{"turn":{"id":"native-turn"}}}))
            .unwrap();
        frames.extend(decoded.controls.iter().map(Outbound::value));
        frames.extend(decoded.replies);
        assert_eq!(frames[0], json!({"id":0,"result":{"decision":"cancel"}}));
        assert_eq!(frames[1]["method"], "turn/interrupt");
        assert_eq!(frames.len(), 2);
    }
    #[test]
    fn codex_thread_credential_stays_typed_until_serialization() {
        let token = crate::mcp::McpToken::new();
        let config = std::sync::Arc::new(McpConfig {
            command: "/owned/veyra".into(),
            socket: "/owned/veyra.sock".into(),
            token: token.clone(),
        });
        let outbound = Outbound::CodexThread {
            line: json!({"id":1,"method":"thread/start","params":{}}),
            mcp: config,
        };
        let recorded = outbound.value();
        assert_eq!(
            recorded["params"]["config"]["mcp_servers.veyra.env"]["VEYRA_MCP_TOKEN"],
            "<id:token>"
        );
        assert!(!recorded.to_string().contains(token.expose()));
        let bytes = outbound.into_bytes().unwrap();
        let written: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            written["params"]["config"]["mcp_servers.veyra.env"]["VEYRA_MCP_TOKEN"].as_str()
                == Some(token.expose())
        );
    }
    #[test]
    fn codex_early_mcp_without_turn_id_can_be_answered() {
        let mut codec = early_request_codec();
        let turn = TurnId::new();
        codec.start(turn, "work".into()).unwrap();
        let opened=codec.value(json!({"id":0,"method":"mcpServer/elicitation/request",
            "params":{"threadId":"old-thread","serverName":"veyra","message":"Allow the veyra MCP server to run tool list_providers"}})).unwrap();
        assert_eq!(request_count(&opened), 1);
        assert!(opened.events.iter().any(|e|matches!(e,ProviderEvent::Turn {turn:Some(id),event:TurnEvent::RequestOpened{..}} if *id==turn)));
        assert_eq!(
            codec
                .answer(NativeRequestRef("0".into()), Decision::AllowOnce)
                .unwrap(),
            json!({"id":0,"result":{"action":"accept"}})
        );
    }
}
