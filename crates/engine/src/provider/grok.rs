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
        PermissionMode::AskFirst => {
            args.extend(["--permission-mode".into(), "default".into(), "agent".into()])
        }
        PermissionMode::AcceptEdits => args.extend([
            "--permission-mode".into(),
            "acceptEdits".into(),
            "agent".into(),
        ]),
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
struct Prompt {
    owner: ActiveTurn,
    accepted: bool,
    first_frame: Option<Instant>,
}
enum PermissionReply {
    Permission {
        allow: Option<String>,
        reject: Option<String>,
    },
    Question,
}
enum ToolState {
    Running(String),
    Ended,
}
struct Request {
    id: Value,
    kind: RequestKind,
    reply: PermissionReply,
    prompt: String,
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
    kind: String,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Meta {
    prompt_id: Option<String>,
    is_replay: bool,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Tool {
    tool_call_id: String,
    title: String,
    status: String,
    raw_input: Value,
    raw_output: Value,
    content: Vec<Value>,
}
#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct QuestionParams {
    session_id: String,
    questions: Vec<WireQuestion>,
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

pub(crate) struct Codec {
    peer: Peer<Call>,
    spec: SessionSpec,
    pid: u32,
    mcp: Option<Arc<McpConfig>>,
    native: Option<NativeSessionId>,
    version: Option<String>,
    resume: bool,
    auth: Option<String>,
    auth_retried: bool,
    prompts: BTreeMap<String, Prompt>,
    ended: BTreeSet<String>,
    active: Option<String>,
    requests: BTreeMap<String, Request>,
    seen_requests: BTreeSet<String>,
    tasks: BTreeSet<String>,
    tools: BTreeMap<String, ToolState>,
    interrupted: Option<super::StopLadder>,
}
impl Codec {
    pub(crate) fn new(spec: SessionSpec, pid: u32, mcp: Option<Arc<McpConfig>>) -> Self {
        Self {
            peer: Peer::new(true),
            spec,
            pid,
            mcp,
            native: None,
            version: None,
            resume: false,
            auth: None,
            auth_retried: false,
            prompts: BTreeMap::new(),
            ended: BTreeSet::new(),
            active: None,
            requests: BTreeMap::new(),
            seen_requests: BTreeSet::new(),
            tasks: BTreeSet::new(),
            tools: BTreeMap::new(),
            interrupted: None,
        }
    }
    #[cfg(debug_assertions)]
    pub(crate) fn native(&self) -> Option<&NativeSessionId> {
        self.native.as_ref()
    }
    pub(crate) fn initialize(&mut self) -> Value {
        self.peer.send(Call::Initialize, "initialize", initialize())
    }
    pub(crate) fn active(&self) -> Option<ActiveTurn> {
        self.active
            .as_ref()
            .and_then(|id| self.prompts.get(id))
            .map(|p| p.owner)
    }
    fn open(&mut self) -> Result<Outbound, ProviderError> {
        let method = if self.spec.native.is_none() {
            "session/new"
        } else if self.resume && !self.spec.import {
            "session/resume"
        } else {
            "session/load"
        };
        let mut params = json!({"cwd":self.spec.cwd,"mcpServers":[]});
        if let Some(native) = &self.spec.native {
            params["sessionId"] = json!(native.0);
        }
        let line = self.peer.send(Call::Open, method, params);
        if let Some(mcp) = &self.mcp {
            mcp.frame(line)
        } else {
            Ok(Outbound::Message(line))
        }
    }
    fn authenticate(&mut self) -> Result<Value, ProviderError> {
        let method = self
            .auth
            .as_ref()
            .ok_or_else(|| ProviderError::Protocol("Sign in with `grok login`".into()))?;
        Ok(self.peer.send(
            Call::Authenticate,
            "authenticate",
            json!({"methodId":method}),
        ))
    }
    pub(crate) fn start(&mut self, turn: TurnId, text: String) -> Option<Outbound> {
        if !self.prompts.is_empty() {
            return None;
        }
        let native = self.native.as_ref()?;
        let id = turn.to_string();
        self.prompts.insert(
            id.clone(),
            Prompt {
                owner: ActiveTurn::Veyra(turn),
                accepted: false,
                first_frame: Some(Instant::now() + Duration::from_secs(60)),
            },
        );
        self.active = Some(id.clone());
        self.interrupted = None;
        Some(Outbound::Prompt(self.peer.send(Call::Prompt(id.clone()),"session/prompt",
            json!({"sessionId":native.0,"prompt":[{"type":"text","text":text}],"_meta":{"promptId":id}}))))
    }
    fn event(decoded: &mut Decoded, owner: ActiveTurn, event: TurnEvent) {
        decoded.events.push(ProviderEvent::Turn {
            turn: owner.id(),
            event,
        });
    }
    fn prompt(&mut self, id: Option<&str>, decoded: &mut Decoded) -> Option<String> {
        let id = id.map(str::to_owned).or_else(|| self.active.clone())?;
        if self.ended.contains(&id) {
            return None;
        }
        if !self.prompts.contains_key(&id) {
            let owner = ActiveTurn::Provider(TurnId::new());
            self.prompts.insert(
                id.clone(),
                Prompt {
                    owner,
                    accepted: true,
                    first_frame: None,
                },
            );
            decoded.provider_turn = Some(owner.target());
            Self::event(decoded, owner, TurnEvent::Started);
        }
        self.active = Some(id.clone());
        Some(id)
    }
    fn finish(&mut self, id: &str, outcome: TurnOutcome, accepted: bool, decoded: &mut Decoded) {
        let Some(prompt) = self.prompts.remove(id) else {
            return;
        };
        if accepted && !prompt.accepted {
            Self::event(decoded, prompt.owner, TurnEvent::Accepted);
        }
        let outcome = if self.active.as_deref() == Some(id) {
            self.end_outcome(outcome)
        } else {
            outcome
        };
        Self::event(
            decoded,
            prompt.owner,
            TurnEvent::TurnEnded {
                outcome,
                park: false,
            },
        );
        self.ended.insert(id.into());
        self.requests.retain(|_, r| r.prompt != id);
        if self.active.as_deref() == Some(id) {
            self.active = self.prompts.keys().next().cloned();
            self.interrupted = None;
        }
    }
    pub(crate) fn value(&mut self, value: Value) -> Result<Decoded, ProviderError> {
        for prompt in self.prompts.values_mut() {
            prompt.first_frame = None;
        }
        let mut decoded = Decoded::default();
        let prompt_id = [
            "/params/_meta/promptId",
            "/params/promptId",
            "/_meta/promptId",
            "/result/_meta/promptId",
        ]
        .into_iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_str));
        if let Some(prompt) = prompt_id.and_then(|id| self.prompts.get_mut(id))
            && matches!(prompt.owner, ActiveTurn::Veyra(_))
            && !prompt.accepted
        {
            prompt.accepted = true;
            Self::event(&mut decoded, prompt.owner, TurnEvent::Accepted);
        }
        match self.peer.receive(value) {
            Frame::Reply {
                tag: Call::Initialize,
                result,
            } => {
                let init: Initialization = decode(result.map_err(jsonrpc::failure)?)?;
                self.version = init.agent_info.version.filter(|s| !s.is_empty());
                self.resume = init
                    .agent_capabilities
                    .session_capabilities
                    .resume
                    .is_some_and(|v| v != false && !v.is_null());
                self.auth = auth_method(
                    &init.auth_methods,
                    std::env::var_os("XAI_API_KEY").is_some(),
                );
                if self.auth.is_some() {
                    decoded.replies.push(self.authenticate()?);
                } else {
                    decoded.controls.push(self.open()?);
                }
            }
            Frame::Reply {
                tag: Call::Authenticate,
                result,
            } => {
                result.map_err(|_| ProviderError::Protocol("Sign in with `grok login`".into()))?;
                decoded.controls.push(self.open()?);
            }
            Frame::Reply {
                tag: Call::Open,
                result,
            } => {
                match result {
                    Ok(result) => {
                        let native = result["sessionId"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                            .or_else(|| self.spec.native.as_ref().map(|n| n.0.clone()))
                            .ok_or_else(|| {
                                ProviderError::Protocol(
                                    "Grok session reply has no sessionId".into(),
                                )
                            })?;
                        self.native = Some(NativeSessionId(native.clone()));
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
                    Err(error) if error.code == -32000 && !self.auth_retried => {
                        if let Some(mcp) = &self.mcp {
                            mcp.revoke();
                        }
                        self.auth_retried = true;
                        decoded.replies.push(self.authenticate()?);
                    }
                    Err(error) if self.spec.native.is_some() && not_found(&error) => {
                        if let Some(mcp) = &self.mcp {
                            mcp.revoke();
                        }
                        self.spec.native = None;
                        decoded.events.push(ProviderEvent::Session(SessionEvent::Notice {
                        text:"The earlier conversation could not be resumed; this is a new session".into() }));
                        decoded.controls.push(self.open()?);
                    }
                    Err(error) => {
                        return Err(if error.code == -32000 {
                            ProviderError::Protocol("Sign in with `grok login`".into())
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
                self.notification(&method, params, &mut decoded)?
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
    fn own_session(&self, params: &Value) -> bool {
        self.native
            .as_ref()
            .is_some_and(|n| params["sessionId"].as_str() == Some(n.0.as_str()))
    }
    fn notification(
        &mut self,
        method: &str,
        params: Value,
        decoded: &mut Decoded,
    ) -> Result<(), ProviderError> {
        if !self.own_session(&params) {
            return Ok(());
        }
        if matches!(
            method,
            "x.ai/task_backgrounded"
                | "_x.ai/task_backgrounded"
                | "x.ai/task_completed"
                | "_x.ai/task_completed"
        ) {
            if let Some(id) = params["update"]["task_id"]
                .as_str()
                .filter(|id| !id.is_empty())
            {
                if method.ends_with("task_backgrounded") {
                    self.tasks.insert(id.into());
                } else {
                    self.tasks.remove(id);
                }
                decoded
                    .events
                    .push(ProviderEvent::Session(SessionEvent::BackgroundWork {
                        pending: !self.tasks.is_empty(),
                    }));
            }
            return Ok(());
        }
        let meta: Meta = decode(params.get("_meta").cloned().unwrap_or(json!({})))?;
        if meta.is_replay {
            return Ok(());
        }
        if matches!(
            method,
            "_x.ai/session/prompt_complete" | "x.ai/session/prompt_complete"
        ) {
            let id = params["promptId"]
                .as_str()
                .or(meta.prompt_id.as_deref())
                .map(str::to_owned)
                .or_else(|| self.active.clone());
            if let Some(id) = id {
                self.finish(&id, outcome(&params), true, decoded);
            }
            return Ok(());
        }
        if method != "session/update" {
            return Ok(());
        }
        let update = &params["update"];
        let kind = update["sessionUpdate"].as_str().unwrap_or_default();
        if !matches!(
            kind,
            "agent_message_chunk"
                | "agent_thought_chunk"
                | "tool_call"
                | "tool_call_update"
                | "plan"
        ) {
            return Ok(());
        }
        let Some(id) = self.prompt(meta.prompt_id.as_deref(), decoded) else {
            return Ok(());
        };
        let owner = self.prompts[&id].owner;
        let key = |suffix: &str| NativeItemKey(format!("{id}/{suffix}"));
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                if update["content"]["type"] == "text"
                    && let Some(text) = update["content"]["text"].as_str()
                {
                    Self::event(
                        decoded,
                        owner,
                        if kind == "agent_message_chunk" {
                            TurnEvent::TextDelta {
                                key: key("reply"),
                                text: text.into(),
                                streamed: true,
                            }
                        } else {
                            TurnEvent::ReasoningDelta {
                                key: key("thought"),
                                text: text.into(),
                            }
                        },
                    );
                }
            }
            "tool_call" | "tool_call_update" => {
                let tool: Tool = decode(update.clone())?;
                if tool.tool_call_id.is_empty() {
                    return Ok(());
                }
                let tool_key = format!("{id}/tool/{}", tool.tool_call_id);
                let output = tool_output(&tool.raw_output, &tool.content);
                let state = self.tools.entry(tool_key.clone()).or_insert_with(|| {
                    Self::event(
                        decoded,
                        owner,
                        TurnEvent::ToolStarted {
                            key: NativeItemKey(tool_key.clone()),
                            call: ToolCall {
                                name: tool.title,
                                arguments: tool.raw_input,
                                veyra_label: false,
                                input_summary: None,
                            },
                        },
                    );
                    ToolState::Running(String::new())
                });
                let ToolState::Running(previous) = state else {
                    return Ok(());
                };
                let delta = output
                    .map(|output| {
                        let delta = output
                            .strip_prefix(previous.as_str())
                            .unwrap_or(&output)
                            .to_owned();
                        *previous = output;
                        delta
                            .chars()
                            .rev()
                            .take(TOOL_OUTPUT_CHARS)
                            .collect::<String>()
                            .chars()
                            .rev()
                            .collect::<String>()
                    })
                    .filter(|text| !text.is_empty());
                if matches!(tool.status.as_str(), "completed" | "failed") {
                    *state = ToolState::Ended;
                    Self::event(
                        decoded,
                        owner,
                        TurnEvent::ToolFinished {
                            key: NativeItemKey(tool_key),
                            ok: tool.status == "completed",
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
            "plan" => {
                if let Some(entries) = update["entries"].as_array() {
                    Self::event(
                        decoded,
                        owner,
                        TurnEvent::Plan(
                            entries
                                .iter()
                                .filter_map(|entry| {
                                    Some(PlanStep {
                                        text: entry["content"].as_str()?.into(),
                                        done: entry["status"] == "completed",
                                    })
                                })
                                .collect(),
                        ),
                    );
                }
            }
            _ => {}
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
        let parsed = match method {
            "session/request_permission" => {
                let permission: Permission = decode(params.clone())?;
                if self
                    .native
                    .as_ref()
                    .is_none_or(|n| n.0 != permission.session_id)
                    || permission.tool_call.tool_call_id.is_empty()
                {
                    None
                } else {
                    let allow = permission
                        .options
                        .iter()
                        .find(|o| o.kind == "allow_once")
                        .map(|o| o.option_id.clone());
                    let reject = permission
                        .options
                        .iter()
                        .find(|o| o.kind == "reject_once")
                        .map(|o| o.option_id.clone());
                    let command = permission.tool_call.raw_input["command"].as_str();
                    let exact_command = permission.tool_call.title == "Bash"
                        && permission
                            .tool_call
                            .raw_input
                            .as_object()
                            .is_some_and(|args| {
                                args.keys().all(|key| {
                                    matches!(key.as_str(), "command" | "description" | "timeout")
                                })
                            });
                    let grant_key =
                        command
                            .filter(|_| exact_command && allow.is_some())
                            .map(|command| GrantKey {
                                tool: permission.tool_call.title.clone(),
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
                    let mut action = ProposedAction::new(permission.tool_call.title, target);
                    if allow.is_none() {
                        action.consent = ApprovalConsent::DenyOnly {
                            reason: "Grok offered only a lasting grant".into(),
                        };
                    }
                    Some((
                        RequestKind::Approval { action, grant_key },
                        PermissionReply::Permission { allow, reject },
                        permission.meta.prompt_id,
                    ))
                }
            }
            "_x.ai/ask_user_question" | "x.ai/ask_user_question" => {
                let question: QuestionParams = decode(params["params"].clone())?;
                let valid = self
                    .native
                    .as_ref()
                    .is_some_and(|n| n.0 == question.session_id)
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
                        params["_meta"]["promptId"].as_str().map(str::to_owned),
                    )
                })
            }
            _ => {
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
        let Some(prompt) = self.prompt(prompt_id.as_deref(), decoded) else {
            decoded
                .replies
                .push(self.peer.error(id, -32600, "Request has no active prompt"));
            return Ok(());
        };
        let owner = self.prompts[&prompt].owner;
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
                prompt,
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
            (PermissionReply::Permission { reject, .. }, AcceptedDecision::Deny) => match reject {
                Some(id) => json!({"outcome":{"outcome":"selected","optionId":id}}),
                None => json!({"outcome":{"outcome":"cancelled"}}),
            },
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
        let Some(native) = &self.native else {
            return vec![];
        };
        self.interrupted = Some(super::StopLadder::new(reason));
        let mut lines = Vec::new();
        for (_, request) in std::mem::take(&mut self.requests) {
            let result = match request.reply {
                PermissionReply::Permission { .. } => json!({"outcome":{"outcome":"cancelled"}}),
                PermissionReply::Question => json!({"outcome":"cancelled"}),
            };
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
fn auth_method(methods: &[AuthMethod], api_key: bool) -> Option<String> {
    ["xai.api_key", "cached_token"]
        .into_iter()
        .find(|id| (*id != "xai.api_key" || api_key) && methods.iter().any(|m| m.id == *id))
        .map(str::to_owned)
}
fn not_found(error: &jsonrpc::Error) -> bool {
    let text = error.message.to_lowercase();
    error.code != -32000
        && (text.contains("session not found") || text.contains("session does not exist"))
}
fn outcome(result: &Value) -> TurnOutcome {
    match result["stopReason"].as_str().unwrap_or("end_turn") {
        "cancelled" => TurnOutcome::Interrupted {
            reason: "Grok cancelled the turn".into(),
        },
        "error" | "refusal" | "rate_limit" => TurnOutcome::Failed {
            message: result["agentResult"]
                .as_str()
                .unwrap_or("Grok could not complete the turn")
                .into(),
        },
        _ => TurnOutcome::Completed,
    }
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
