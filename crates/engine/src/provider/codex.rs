use super::{
    ActiveTurn, Decoded, Outbound, ProviderError, ProviderEvent, SessionEvent, SessionSpec,
    StopReason, TurnEvent,
    jsonrpc::{self, Frame, Peer},
};
use crate::{domain::*, platform::Signal};
use serde_json::{Value, json};
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
pub fn thread_params(spec: &SessionSpec, mcp: Option<&Value>) -> Value {
    let (approval, sandbox, _) = mode(spec.config.mode);
    let mut params = json!({"cwd":spec.cwd,"model":spec.config.model,"approvalPolicy":approval,"sandbox":sandbox,"approvalsReviewer":"user"});
    if let Some(native) = &spec.native {
        params["threadId"] = json!(native.0);
        params["excludeTurns"] = json!(true);
    }
    if let Some(mcp) = mcp {
        params["config"] = json!({"mcp_servers.veyra.command":mcp["command"],"mcp_servers.veyra.args":mcp["args"],"mcp_servers.veyra.env":mcp["env"]});
    }
    params
}
pub fn turn_params(native: &str, config: &ThreadConfig, text: String) -> Value {
    let (approval, _, sandbox) = mode(config.mode);
    json!({"threadId":native,"input":[{"type":"text","text":text,"text_elements":[]}],"model":config.model,"effort":config.effort,"summary":"auto","approvalPolicy":approval,"approvalsReviewer":"user","sandboxPolicy":sandbox})
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
    turn: String,
}
#[derive(Default)]
struct Item {
    finished: bool,
    streamed: bool,
    started: bool,
    output: String,
}
enum InterruptPhase {
    Protocol(Instant),
    Terminate(Instant),
}
struct Interruption {
    target: TurnId,
    reason: String,
    phase: InterruptPhase,
    retry_after_started: bool,
    retried: bool,
    sent: bool,
}
enum Early {
    Notification {
        method: String,
        params: Value,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
}
pub(crate) struct Codec {
    peer: Peer<Call>,
    spec: SessionSpec,
    mcp: Option<Value>,
    pid: u32,
    native: Option<String>,
    pub active: Option<ActiveTurn>,
    starting: Option<TurnId>,
    native_turn: Option<String>,
    early: Vec<Early>,
    turns: BTreeMap<String, Turn>,
    ended: BTreeSet<String>,
    items: BTreeMap<String, Item>,
    requests: BTreeMap<String, Request>,
    seen_requests: BTreeSet<String>,
    interrupted: Option<Interruption>,
    resume_retry: Option<Instant>,
    resume_until: Option<Instant>,
}
impl Codec {
    pub(crate) fn new(spec: SessionSpec, pid: u32, mcp: Option<Value>) -> Self {
        Self {
            native: spec.native.as_ref().map(|n| n.0.clone()),
            spec,
            pid,
            mcp,
            peer: Peer::new(false),
            active: None,
            starting: None,
            native_turn: None,
            early: vec![],
            turns: BTreeMap::new(),
            ended: BTreeSet::new(),
            items: BTreeMap::new(),
            requests: BTreeMap::new(),
            seen_requests: BTreeSet::new(),
            interrupted: None,
            resume_retry: None,
            resume_until: None,
        }
    }
    pub(crate) fn initialize(&mut self) -> Value {
        self.peer.send(Call::Initialize, "initialize", initialize())
    }
    fn open_thread(&mut self) -> Value {
        self.peer.send(
            Call::Thread,
            if self.spec.native.is_some() {
                "thread/resume"
            } else {
                "thread/start"
            },
            thread_params(&self.spec, self.mcp.as_ref()),
        )
    }
    pub(crate) fn start(&mut self, turn: TurnId, text: String) -> Option<Outbound> {
        if self.active.is_some() {
            return None;
        }
        let native = self.native.as_deref()?;
        self.active = Some(ActiveTurn::Veyra(turn));
        self.starting = Some(turn);
        self.native_turn = None;
        self.interrupted = None;
        Some(Outbound::Prompt(self.peer.send(
            Call::Start(turn),
            "turn/start",
            turn_params(native, &self.spec.config, text),
        )))
    }
    pub(crate) fn value(&mut self, wire: Value) -> Result<Decoded, ProviderError> {
        let mut decoded = Decoded::default();
        match self.peer.receive(wire) {
            Frame::Reply {
                tag: Call::Initialize,
                result,
            } => {
                result.map_err(jsonrpc::failure)?;
                decoded
                    .replies
                    .push(self.peer.notification("initialized", Value::Null));
                decoded.replies.push(self.open_thread());
            }
            Frame::Reply {
                tag: Call::Thread,
                result,
            } => match result {
                Ok(result) => {
                    let native = result["thread"]["id"]
                        .as_str()
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| {
                            ProviderError::Protocol("Codex thread reply has no thread id".into())
                        })?
                        .to_owned();
                    self.native = Some(native.clone());
                    self.mcp = None;
                    self.resume_retry = None;
                    self.resume_until = None;
                    decoded
                        .events
                        .push(ProviderEvent::Session(SessionEvent::Ready {
                            native: NativeSessionId(native),
                            cli_version: result["thread"]["cliVersion"]
                                .as_str()
                                .filter(|v| !v.is_empty())
                                .map(str::to_owned),
                            pid: self.pid,
                            pgid: self.pid,
                            capabilities: vec![],
                        }));
                }
                Err(error) if self.spec.native.is_some() && not_found(&error.message) => {
                    self.spec.native = None;
                    self.native = None;
                    decoded.events.push(Self::notice(
                        "The earlier conversation could not be resumed; this is a new session",
                    ));
                    decoded.replies.push(self.open_thread());
                }
                Err(error)
                    if self.spec.native.is_some()
                        && error.message.contains("already has an active writer") =>
                {
                    let until = *self
                        .resume_until
                        .get_or_insert_with(|| Instant::now() + Duration::from_secs(3));
                    if Instant::now() >= until {
                        return Err(jsonrpc::failure(error));
                    }
                    self.resume_retry =
                        Some((Instant::now() + Duration::from_millis(250)).min(until));
                }
                Err(error) => return Err(jsonrpc::failure(error)),
            },
            Frame::Reply {
                tag: Call::Start(turn),
                result,
            } => {
                let result = result.map_err(jsonrpc::failure)?;
                let id = result["turn"]["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        ProviderError::Protocol("Codex turn reply has no turn id".into())
                    })?
                    .to_owned();
                self.starting = None;
                self.native_turn = Some(id.clone());
                self.turns.insert(
                    id.clone(),
                    Turn {
                        owner: ActiveTurn::Veyra(turn),
                        started: false,
                    },
                );
                decoded.events.push(ProviderEvent::Turn {
                    turn: Some(turn),
                    event: TurnEvent::Accepted,
                });
                for frame in std::mem::take(&mut self.early) {
                    match frame {
                        Early::Notification { method, params } => {
                            self.notification(&method, params, &mut decoded)?
                        }
                        Early::Request { id, method, params } => {
                            self.request(id, &method, params, &mut decoded)
                        }
                    }
                }
                if self.active.is_some() && self.interrupted.is_some() && !self.peer_has_interrupt()
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
                        .native_turn
                        .as_ref()
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
                            && !interrupt.retried
                        {
                            interrupt.retry_after_started = true;
                        }
                    } else if self.active.is_some_and(|active| active.target() == target) {
                        return Err(jsonrpc::failure(error));
                    }
                }
            }
            Frame::Request { id, method, params } => {
                if self.starting.is_some() && self.own_thread(&params) && supported_request(&method)
                {
                    self.early.push(Early::Request { id, method, params });
                } else {
                    self.request(id, &method, params, &mut decoded);
                }
            }
            Frame::Notification { method, params } => {
                self.notification(&method, params, &mut decoded)?
            }
            Frame::Unknown => tracing::info!("ignored unrecognized Codex frame"),
        }
        Ok(decoded)
    }
    fn peer_has_interrupt(&self) -> bool {
        self.interrupted.as_ref().is_some_and(|i| i.sent)
    }
    fn own_thread(&self, params: &Value) -> bool {
        params["threadId"]
            .as_str()
            .is_some_and(|id| Some(id) == self.native.as_deref())
    }
    fn route(&mut self, id: &str, decoded: &mut Decoded) -> Option<ActiveTurn> {
        if self.ended.contains(id) {
            return None;
        }
        if let Some(turn) = self.turns.get(id) {
            return Some(turn.owner);
        }
        if self.active.is_some() {
            return None;
        }
        let owner = ActiveTurn::Provider(TurnId::new());
        self.active = Some(owner);
        self.native_turn = Some(id.into());
        self.turns.insert(
            id.into(),
            Turn {
                owner,
                started: false,
            },
        );
        decoded.provider_turn = Some(owner.target());
        Some(owner)
    }
    fn notification(
        &mut self,
        method: &str,
        params: Value,
        decoded: &mut Decoded,
    ) -> Result<(), ProviderError> {
        if method == "account/rateLimits/updated" {
            decoded.events.push(Self::notice(
                "Codex updated its usage limits. Check the CLI for the remaining allowance.",
            ));
            return Ok(());
        }
        if method == "error" && params["willRetry"] == true {
            if params.get("threadId").is_none() || self.own_thread(&params) {
                decoded
                    .events
                    .push(Self::notice("Codex is retrying after a provider error."));
            }
            return Ok(());
        }
        if !matches!(
            method,
            "turn/started"
                | "turn/completed"
                | "item/agentMessage/delta"
                | "item/reasoning/summaryTextDelta"
                | "item/reasoning/textDelta"
                | "item/commandExecution/outputDelta"
                | "item/started"
                | "item/completed"
                | "turn/plan/updated"
        ) {
            tracing::info!(method, "ignored Codex notification");
            return Ok(());
        }
        if !self.own_thread(&params) {
            return Ok(());
        }
        let id = params["turnId"]
            .as_str()
            .or_else(|| params["turn"]["id"].as_str())
            .unwrap_or_default()
            .to_owned();
        if id.is_empty() {
            return Ok(());
        }
        if self.starting.is_some() {
            self.early.push(Early::Notification {
                method: method.into(),
                params,
            });
            return Ok(());
        }
        let owner = if matches!(
            method,
            "turn/completed" | "item/commandExecution/outputDelta"
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
        match method {
            "turn/started" => {
                if let Some(turn) = self.turns.get_mut(&id)
                    && !turn.started
                {
                    turn.started = true;
                    events.push(TurnEvent::Started);
                }
                let retry = self.interrupted.as_mut().is_some_and(|i| {
                    if i.target == owner.target() && i.retry_after_started && !i.retried {
                        i.retried = true;
                        true
                    } else {
                        false
                    }
                });
                if retry {
                    decoded.controls.push(self.interrupt_frame(owner.target()));
                }
            }
            "turn/completed" => {
                let outcome = match params["turn"]["status"].as_str().unwrap_or("completed") {
                    "interrupted" => TurnOutcome::Interrupted {
                        reason: "Stopped by Codex".into(),
                    },
                    "failed" => TurnOutcome::Failed {
                        message: crate::record::redact(
                            params["turn"]["error"]["message"]
                                .as_str()
                                .unwrap_or("Codex could not finish this turn"),
                            None,
                        ),
                    },
                    _ => TurnOutcome::Completed,
                };
                let outcome = self.end_outcome(outcome);
                self.ended.insert(id.clone());
                self.requests.retain(|_, request| request.turn != id);
                if self
                    .active
                    .is_some_and(|active| active.target() == owner.target())
                {
                    self.active = None;
                    self.native_turn = None;
                    self.interrupted = None;
                }
                events.push(TurnEvent::TurnEnded {
                    outcome,
                    park: false,
                });
            }
            "item/agentMessage/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/textDelta" => {
                let item = params["itemId"].as_str().unwrap_or_default();
                if !item.is_empty() {
                    let key = Self::key(&id, item);
                    let state = self.items.entry(key.0.clone()).or_default();
                    state.streamed = true;
                    let text = params["delta"].as_str().unwrap_or_default().to_owned();
                    events.push(if method == "item/agentMessage/delta" {
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
            "item/commandExecution/outputDelta" => {
                let item = params["itemId"].as_str().unwrap_or_default();
                if !item.is_empty() {
                    let key = Self::key(&id, item);
                    let text = params["delta"].as_str().unwrap_or_default().to_owned();
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
            "item/started" | "item/completed" => self.item(
                &id,
                &params["item"],
                method == "item/completed",
                &mut events,
            ),
            "turn/plan/updated" => {
                let plan = params["plan"]
                    .as_array()
                    .map(|steps| {
                        steps
                            .iter()
                            .filter_map(|step| {
                                Some(PlanStep {
                                    text: step["step"].as_str()?.into(),
                                    done: step["status"] == "completed",
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                events.push(TurnEvent::Plan(plan));
            }
            _ => tracing::info!(method, "ignored Codex notification"),
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
    fn item(&mut self, turn: &str, wire: &Value, completed: bool, events: &mut Vec<TurnEvent>) {
        let Some(id) = wire["id"].as_str().filter(|id| !id.is_empty()) else {
            return;
        };
        let key = Self::key(turn, id);
        let state = self.items.entry(key.0.clone()).or_default();
        if state.finished {
            return;
        }
        state.finished = completed;
        match wire["type"].as_str().unwrap_or_default() {
            "agentMessage" if completed => {
                if !state.streamed {
                    events.push(TurnEvent::TextDelta {
                        key: key.clone(),
                        text: wire["text"].as_str().unwrap_or_default().into(),
                        streamed: false,
                    });
                }
                events.push(TurnEvent::ItemEnded { key });
            }
            "reasoning" if completed => {
                if !state.streamed {
                    let text = wire["summary"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("\n")
                        })
                        .unwrap_or_default();
                    if !text.is_empty() {
                        events.push(TurnEvent::ReasoningDelta {
                            key: key.clone(),
                            text,
                        });
                    }
                }
                events.push(TurnEvent::ItemEnded { key });
            }
            kind @ ("commandExecution"
            | "fileChange"
            | "mcpToolCall"
            | "collabAgentToolCall"
            | "webSearch"
            | "imageView") => {
                if !state.started {
                    state.started = true;
                    let (name, arguments, summary) = match kind {
                        "commandExecution" => (
                            "Command".into(),
                            json!({"command":wire["command"],"cwd":wire["cwd"]}),
                            wire["command"].as_str().map(str::to_owned),
                        ),
                        "fileChange" => (
                            "File change".into(),
                            json!({"changes":wire["changes"]}),
                            None,
                        ),
                        "mcpToolCall" => (
                            format!(
                                "{} / {}",
                                wire["server"].as_str().unwrap_or("MCP"),
                                wire["tool"].as_str().unwrap_or("tool")
                            ),
                            wire["arguments"].clone(),
                            None,
                        ),
                        _ => (kind.into(), wire.clone(), None),
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
                    let output = wire["aggregatedOutput"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| (!state.output.is_empty()).then(|| state.output.clone()))
                        .or_else(|| {
                            wire.get("result")
                                .filter(|v| !v.is_null())
                                .map(Value::to_string)
                        });
                    let ok = wire["status"] != "failed"
                        && wire["status"] != "declined"
                        && wire["exitCode"].as_i64().is_none_or(|c| c == 0);
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
    fn request(&mut self, id: Value, method: &str, params: Value, decoded: &mut Decoded) {
        let key = id.to_string();
        if !self.seen_requests.insert(key.clone()) {
            decoded
                .replies
                .push(self.peer.error(id, -32600, "Request id was already used"));
            return;
        }
        let supported = supported_request(method);
        if !supported {
            decoded
                .replies
                .push(self.peer.error(id, -32601, "Unsupported server request"));
            return;
        }
        let native_turn = params["turnId"]
            .as_str()
            .or(self.native_turn.as_deref())
            .unwrap_or_default();
        let owner = self.turns.get(native_turn).map(|t| t.owner);
        if !self.own_thread(&params) || owner.is_none() || self.ended.contains(native_turn) {
            decoded.replies.push(self.peer.error(
                id,
                -32600,
                "Request is not for this thread's active turn",
            ));
            return;
        }
        let turn = native_turn.to_owned();
        let preview =
            || ApprovalPreview::new(serde_json::to_string_pretty(&params).unwrap_or_default());
        let approval = |tool: String, target| RequestKind::Approval {
            action: ProposedAction { tool, target },
            grant_key: None,
        };
        let (kind, reply) = match method {
            "item/commandExecution/requestApproval" => {
                let Some(command) = params["command"].as_str().filter(|s| !s.is_empty()) else {
                    decoded.replies.push(self.peer.error(
                        id,
                        -32602,
                        "Command approval has no command",
                    ));
                    return;
                };
                (
                    RequestKind::Approval {
                        action: ProposedAction {
                            tool: "Command".into(),
                            target: ApprovalTarget::Command {
                                command: command.into(),
                                preview: preview(),
                            },
                        },
                        grant_key: Some(GrantKey {
                            tool: "Command".into(),
                            command: command.into(),
                        }),
                    },
                    RequestReply::Decision,
                )
            }
            "item/fileChange/requestApproval" => (
                approval(
                    "File change".into(),
                    ApprovalTarget::Tool { preview: preview() },
                ),
                RequestReply::Decision,
            ),
            "item/permissions/requestApproval" if params["permissions"].is_object() => (
                approval(
                    "Permissions".into(),
                    ApprovalTarget::Tool { preview: preview() },
                ),
                RequestReply::Permissions(params["permissions"].clone()),
            ),
            "mcpServer/elicitation/request" => {
                let Some(tool) = elicitation_tool(&params) else {
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
                            params["serverName"].as_str().unwrap_or("MCP")
                        ),
                        ApprovalTarget::Tool {
                            preview: ApprovalPreview::new(
                                params["message"].as_str().unwrap_or_default().into(),
                            ),
                        },
                    ),
                    RequestReply::Elicitation,
                )
            }
            "item/tool/requestUserInput" => {
                let Some((questions, ids)) = questions(&params) else {
                    decoded.replies.push(self.peer.error(
                        id,
                        -32602,
                        "Question request has no valid questions",
                    ));
                    return;
                };
                (
                    RequestKind::Question { questions },
                    RequestReply::Question(ids),
                )
            }
            _ => {
                decoded
                    .replies
                    .push(self.peer.error(id, -32602, "Invalid approval parameters"));
                return;
            }
        };
        if self.interrupted.is_some() {
            decoded.replies.push(self.peer.response(id, cancel(&reply)));
            return;
        }
        self.requests.insert(
            key.clone(),
            Request {
                id,
                kind: kind.clone(),
                reply,
                turn,
            },
        );
        decoded.events.push(ProviderEvent::Turn {
            turn: owner.unwrap().id(),
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
                json!({"answers":ids.iter().zip(answers).map(|(id,a)|(id.clone(),json!({"answers":[a.text]}))).collect::<serde_json::Map<_,_>>()})
            }
            _ => return None,
        };
        let request = self.requests.remove(&native.0)?;
        Some(self.peer.response(request.id, response))
    }
    fn interrupt_frame(&mut self, target: TurnId) -> Outbound {
        if let Some(interrupt) = &mut self.interrupted {
            interrupt.sent = true;
        }
        Outbound::RpcInterrupt {
            turn: target,
            line: self.peer.send(
                Call::Interrupt(target),
                "turn/interrupt",
                json!({"threadId":self.native,"turnId":self.native_turn}),
            ),
        }
    }
    pub(crate) fn interrupt_lines(&mut self, target: TurnId, reason: StopReason) -> Vec<Outbound> {
        if !self.active.is_some_and(|active| active.target() == target)
            || self.interrupted.is_some()
        {
            return vec![];
        }
        self.interrupted = Some(Interruption {
            target,
            reason: reason.text().into(),
            phase: InterruptPhase::Protocol(Instant::now() + Duration::from_secs(3)),
            retry_after_started: false,
            retried: false,
            sent: false,
        });
        let mut lines: Vec<_> = std::mem::take(&mut self.requests)
            .into_values()
            .map(|r| Outbound::Message(self.peer.response(r.id, cancel(&r.reply))))
            .collect();
        if self.native_turn.is_some() {
            lines.push(self.interrupt_frame(target));
        }
        lines
    }
    pub(crate) fn interrupt_active(&mut self, reason: StopReason) -> Vec<Outbound> {
        self.active
            .map(|turn| self.interrupt_lines(turn.target(), reason))
            .unwrap_or_default()
    }
    pub(crate) fn interrupt_deadline(&self) -> Option<Instant> {
        let ladder = self.interrupted.as_ref().map(|i| match i.phase {
            InterruptPhase::Protocol(at) | InterruptPhase::Terminate(at) => at,
        });
        ladder.into_iter().chain(self.resume_retry).min()
    }
    pub(crate) fn timer_lines(&mut self) -> Vec<Outbound> {
        if self.resume_retry.is_some_and(|at| Instant::now() >= at) {
            self.resume_retry = None;
            return vec![Outbound::Message(self.open_thread())];
        }
        vec![]
    }
    pub(crate) fn escalate_interrupt(&mut self) -> Option<Signal> {
        let interrupt = self.interrupted.as_mut()?;
        match interrupt.phase {
            InterruptPhase::Protocol(at) if Instant::now() >= at => {
                interrupt.phase =
                    InterruptPhase::Terminate(Instant::now() + Duration::from_secs(1));
                Some(Signal::Terminate)
            }
            InterruptPhase::Terminate(at) if Instant::now() >= at => Some(Signal::Kill),
            _ => None,
        }
    }
    pub(crate) fn end_outcome(&self, fallback: TurnOutcome) -> TurnOutcome {
        self.interrupted
            .as_ref()
            .map_or(fallback, |i| TurnOutcome::Interrupted {
                reason: i.reason.clone(),
            })
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
fn elicitation_tool(params: &Value) -> Option<String> {
    let server = params["serverName"].as_str().filter(|s| !s.is_empty())?;
    let keyed = params["_meta"]["codex_approval_kind"] == "mcp_tool_call";
    let parsed = params["message"].as_str().and_then(|message| {
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
fn questions(params: &Value) -> Option<(Vec<Question>, Vec<String>)> {
    let mut ids = BTreeSet::new();
    let mut texts = BTreeSet::new();
    let mut questions = Vec::new();
    let mut ordered_ids = Vec::new();
    for question in params["questions"].as_array()? {
        let id = question["id"].as_str().filter(|s| !s.is_empty())?;
        let text = question["question"].as_str().filter(|s| !s.is_empty())?;
        if !ids.insert(id) || !texts.insert(text) {
            return None;
        }
        let options = match question["options"].as_array() {
            Some(options) => options
                .iter()
                .map(|o| {
                    o["label"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                })
                .collect::<Option<Vec<_>>>()?,
            None => vec![],
        };
        questions.push(Question {
            text: text.into(),
            options,
        });
        ordered_ids.push(id.into());
    }
    (!questions.is_empty()).then_some((questions, ordered_ids))
}

fn supported_request(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
            | "item/tool/requestUserInput"
            | "mcpServer/elicitation/request"
    )
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
            codec.spec.native,
            Some(NativeSessionId("old-thread".into()))
        );
        let mut codec = resumed();
        let d=codec.value(json!({"id":2,"error":{"code":-32600,"message":"no rollout found for thread id old-thread"}})).unwrap();
        assert_eq!(d.replies[0]["method"], "thread/start");
        assert_eq!(codec.spec.native, None);
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
            codec.spec.native,
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
                        &json!({"serverName":"veyra","message":message,"mode":"form","_meta":meta})
                    ),
                    Some("list_providers".into())
                );
            }
        }
        assert_eq!(
            elicitation_tool(
                &json!({"serverName":"veyra","message":"Select a color","mode":"form"})
            ),
            None
        );
    }
}
