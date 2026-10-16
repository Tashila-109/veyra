use super::{ActiveTurn, Decoded, Outbound};
use super::{Model, ProviderEvent, SessionEvent, SessionSpec, StopReason, TurnEvent};
use crate::domain::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const INITIALIZE_ID: &str = "veyra-initialize";
pub fn initialize() -> Value {
    json!({"type":"control_request","request_id":INITIALIZE_ID,"request":{"subtype":"initialize"}})
}
pub fn args(spec: Option<&SessionSpec>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--replay-user-messages",
        "--permission-prompt-tool",
        "stdio",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if let Some(spec) = spec {
        args.extend([
            "--permission-mode".into(),
            match spec.config.mode {
                PermissionMode::AskFirst => "default",
                PermissionMode::AcceptEdits => "acceptEdits",
                PermissionMode::FullAccess => "bypassPermissions",
            }
            .into(),
        ]);
        if spec.config.mode == PermissionMode::FullAccess {
            args.push("--allow-dangerously-skip-permissions".into());
        }
        if let Some(native) = &spec.native {
            args.extend(["--resume".into(), native.0.clone()]);
        } else {
            args.extend(["--session-id".into(), spec.thread.to_string()]);
        }
        if let Some(model) = &spec.config.model {
            args.extend(["--model".into(), model.clone()]);
        }
        if let Some(effort) = &spec.config.effort {
            args.extend(["--effort".into(), effort.clone()]);
        }
    }
    args
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Frame {
    System(System),
    StreamEvent(Stream),
    User(User),
    Assistant(Assistant),
    Result(ResultFrame),
    CommandLifecycle(Lifecycle),
    ControlResponse(ControlResponse),
    ControlRequest(ControlRequest),
    ConversationReset,
    RateLimitEvent {
        rate_limit_info: RateLimit,
    },
    #[serde(other)]
    Unknown,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct System {
    subtype: String,
    session_id: String,
    claude_code_version: String,
    capabilities: Vec<String>,
    tasks: Vec<Value>,
    attempt: u32,
    retry_delay_ms: u64,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Stream {
    event: Event,
    parent_tool_use_id: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    MessageStart {
        #[serde(default)]
        message: Value,
    },
    ContentBlockStart {
        #[serde(default)]
        index: u32,
        #[serde(default)]
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        #[serde(default)]
        index: u32,
        #[serde(default)]
        delta: Delta,
    },
    ContentBlockStop {
        #[serde(default)]
        index: u32,
    },
    #[serde(other)]
    #[default]
    Unknown,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct User {
    uuid: String,
    #[serde(rename = "isReplay")]
    is_replay: bool,
    message: Value,
    parent_tool_use_id: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Assistant {
    message: Value,
    error: Option<String>,
    parent_tool_use_id: Option<String>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct ResultFrame {
    is_error: bool,
    subtype: String,
    result: String,
    errors: Vec<String>,
    stop_reason: Option<String>,
    terminal_reason: Option<String>,
    user_message_uuid: Option<String>,
    user_message_uuids: Vec<String>,
    origin: Option<Value>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Lifecycle {
    command_uuid: String,
    state: String,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct ControlResponse {
    response: Response,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Response {
    request_id: String,
    subtype: String,
    response: InitializeReply,
    error: String,
}
#[derive(Deserialize, Default)]
#[serde(default)]
pub(crate) struct InitializeReply {
    pub models: Vec<Model>,
    pub account: Option<Value>,
    pub capabilities: Vec<String>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct ControlRequest {
    request_id: String,
    request: ToolRequest,
}
#[derive(Deserialize, Default)]
struct ToolRequest {
    #[serde(default)]
    subtype: String,
    #[serde(default)]
    tool_name: String,
    input: serde_json::Map<String, Value>,
}
struct PendingRequest {
    input: serde_json::Map<String, Value>,
    kind: RequestKind,
}

pub(crate) fn probe_value(wire: Value) -> Result<Option<InitializeReply>, super::ProviderError> {
    let frame: Frame = serde_json::from_value(wire)?;
    if let Frame::ControlResponse(reply) = frame
        && reply.response.request_id == INITIALIZE_ID
    {
        if reply.response.subtype == "error" {
            return Err(super::ProviderError::Protocol(reply.response.error));
        }
        return Ok(Some(reply.response.response));
    }
    Ok(None)
}

fn target_location(project: &std::path::Path, target: &std::path::Path) -> TargetLocation {
    use std::{io::ErrorKind, path::Component};
    if !target.is_absolute() {
        return TargetLocation::NotVerified;
    }
    let Ok(project) = project.canonicalize() else {
        return TargetLocation::NotVerified;
    };
    let mut ancestor = target.to_owned();
    let mut missing = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut resolved) => {
                for part in missing.into_iter().rev() {
                    resolved.push(part);
                }
                return if resolved.starts_with(&project) {
                    TargetLocation::InsideProject
                } else {
                    TargetLocation::OutsideProject
                };
            }
            Err(error) => {
                if error.kind() != ErrorKind::NotFound || ancestor.symlink_metadata().is_ok() {
                    return TargetLocation::NotVerified;
                }
                let Some(Component::Normal(part)) = ancestor.components().next_back() else {
                    return TargetLocation::NotVerified;
                };
                missing.push(part.to_owned());
                if !ancestor.pop() {
                    return TargetLocation::NotVerified;
                }
            }
        }
    }
}

#[derive(Default)]
enum EchoMode {
    #[default]
    Unknown,
    Known,
}
#[derive(Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text {
        #[serde(default)]
        text: String,
    },
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    ToolUse {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: Value,
    },
    #[serde(other)]
    #[default]
    Unknown,
}
impl ContentBlock {
    fn kind(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Thinking { .. } => "thinking",
            Self::ToolUse { .. } => "tool_use",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Deserialize, Default)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Delta {
    #[serde(rename = "text_delta")]
    Text {
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "thinking_delta")]
    Thinking {
        #[serde(default)]
        thinking: String,
    },
    #[serde(rename = "input_json_delta")]
    InputJson {
        #[serde(default)]
        partial_json: String,
    },
    #[serde(other)]
    #[default]
    Unknown,
}
struct Block {
    key: NativeItemKey,
    content: ContentBlock,
    input: String,
    streamed: bool,
    text_filter: TextFilter,
}
enum TextFilter {
    Plain,
    Prefix(String),
    Drop,
}
impl TextFilter {
    fn text(&mut self, text: String) -> Option<String> {
        let Self::Prefix(prefix) = self else {
            return if matches!(self, Self::Plain) {
                Some(text)
            } else {
                None
            };
        };
        prefix.push_str(&text);
        if prefix.starts_with("[ede_diagnostic]") {
            *self = Self::Drop;
            None
        } else if "[ede_diagnostic]".starts_with(prefix.as_str()) {
            None
        } else {
            let text = std::mem::take(prefix);
            *self = Self::Plain;
            Some(text)
        }
    }
}
fn log_ignored(kind: &str, value: &Value) {
    tracing::info!(
        kind,
        frame_type = value["type"].as_str().unwrap_or("unknown"),
        "ignored Claude wire type"
    );
}
impl ResultFrame {
    fn outcome(self) -> TurnOutcome {
        let reason = self.terminal_reason.as_deref();
        let aborted = reason.is_some_and(|reason| reason.starts_with("aborted_"));
        let incomplete = reason.is_some_and(|reason| {
            ![
                "completed",
                "background_requested",
                "tool_deferred",
                "hook_stopped",
                "stop_hook_prevented",
            ]
            .contains(&reason)
        });
        if aborted {
            return TurnOutcome::Interrupted {
                reason: format!("Claude ended with {}", reason.unwrap()),
            };
        }
        if self.is_error
            || self.subtype.starts_with("error")
            || (!aborted && incomplete)
            || matches!(self.stop_reason.as_deref(), Some("max_tokens" | "refusal"))
        {
            let message = if !self.errors.is_empty() {
                self.errors.join("\n")
            } else if !self.result.is_empty() {
                self.result
            } else {
                format!("Claude ended with {}", reason.unwrap_or(&self.subtype))
            };
            TurnOutcome::Failed {
                message: crate::record::redact(&message, None),
            }
        } else {
            TurnOutcome::Completed
        }
    }
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct RateLimit {
    status: String,
    #[serde(rename = "resetsAt")]
    resets_at: Option<u64>,
}
struct AssistantError {
    message: String,
    park: bool,
}
impl AssistantError {
    fn new(code: &str) -> Self {
        let (words, park) = match code {
            "authentication_failed" => (
                "Claude is signed out. Sign in from a terminal with `claude`, then send again.",
                true,
            ),
            "oauth_org_not_allowed" => (
                "Claude cannot use this organization. Choose an allowed organization in a terminal, then send again.",
                true,
            ),
            "account_on_hold" => (
                "Your Claude account is on hold. Resolve the account hold, then send again.",
                true,
            ),
            "verification_required" => (
                "Claude requires account verification. Verify your account, then send again.",
                true,
            ),
            "billing_error" => (
                "Claude could not bill this account. Check billing, then send again.",
                true,
            ),
            "cloud_credential_error" => (
                "Claude could not use your cloud credentials. Repair those credentials, then send again.",
                true,
            ),
            "rate_limit" => (
                "Claude reached a usage limit. Wait for the limit to reset, then send again.",
                false,
            ),
            "overloaded" => (
                "Claude is overloaded. Wait briefly, then send again.",
                false,
            ),
            "invalid_request" => (
                "Claude rejected this request. Check the prompt and configuration, then send again.",
                false,
            ),
            "model_not_found" => (
                "Claude could not find this model. Choose another model, then send again.",
                false,
            ),
            "server_error" => ("Claude had a server error. Send again to retry.", false),
            "max_output_tokens" => (
                "Claude reached its output limit. Send a shorter request or ask it to continue.",
                false,
            ),
            _ => (
                "Claude could not finish this request. Check the CLI in a terminal, then send again.",
                false,
            ),
        };
        Self {
            message: words.into(),
            park,
        }
    }
}
#[derive(Default)]
struct MessageStream {
    message: String,
    blocks: BTreeMap<u32, Block>,
    final_blocks: BTreeMap<(String, String), usize>,
}
struct Tool {
    key: NativeItemKey,
    subagent: Option<Box<Codec>>,
}
pub(crate) struct Codec {
    pub active: Option<ActiveTurn>,
    echo: EchoMode,
    native: NativeSessionId,
    pid: u32,
    cwd: std::path::PathBuf,
    stream: MessageStream,
    tools: BTreeMap<String, Tool>,
    error: Option<AssistantError>,
    interrupted: Option<super::StopLadder>,
    requests: BTreeMap<String, PendingRequest>,
    seen_requests: BTreeSet<String>,
    seen_commands: BTreeSet<String>,
}
impl Codec {
    #[cfg(debug_assertions)]
    pub fn native(&self) -> &NativeSessionId {
        &self.native
    }
    pub fn new(native: NativeSessionId, pid: u32, cwd: &std::path::Path) -> Self {
        Self {
            active: None,
            echo: EchoMode::Unknown,
            native,
            pid,
            cwd: cwd.to_owned(),
            stream: MessageStream::default(),
            error: None,
            interrupted: None,
            tools: BTreeMap::new(),
            requests: BTreeMap::new(),
            seen_requests: BTreeSet::new(),
            seen_commands: BTreeSet::new(),
        }
    }
    pub fn start(&mut self, turn: TurnId, text: String) -> Option<Outbound> {
        if self.active.is_some() {
            return None;
        }
        self.seen_commands.insert(turn.to_string());
        self.active = Some(ActiveTurn::Veyra(turn));
        self.stream = MessageStream::default();
        self.error = None;
        self.interrupted = None;
        Some(Outbound::Prompt(json!({
            "type":"user", "uuid":turn.to_string(), "session_id":self.native.0,
            "parent_tool_use_id":null, "message":{"role":"user","content":text}
        })))
    }
    fn prompt(&self) -> Option<TurnId> {
        self.active.and_then(ActiveTurn::id)
    }
    fn ready(&self, version: Option<String>, capabilities: Vec<String>) -> ProviderEvent {
        ProviderEvent::Session(SessionEvent::Ready {
            native: self.native.clone(),
            cli_version: version,
            pid: self.pid,
            pgid: self.pid,
            capabilities,
        })
    }
    #[cfg(test)]
    pub fn line(&mut self, line: &str) -> Result<Decoded, super::ProviderError> {
        self.value(serde_json::from_str(line)?)
    }
    pub fn value(&mut self, wire: Value) -> Result<Decoded, super::ProviderError> {
        let metadata = json!({
            "type":wire["type"].as_str().unwrap_or("unknown"),
            "subtype":wire["subtype"].as_str().unwrap_or(""),
            "event":{
                "type":wire["event"]["type"].as_str().unwrap_or("unknown"),
                "content_block":{"type":wire["event"]["content_block"]["type"].as_str().unwrap_or("unknown")},
                "delta":{"type":wire["event"]["delta"]["type"].as_str().unwrap_or("unknown")}
            }
        });
        let frame_type = wire["type"].as_str().unwrap_or("unknown");
        let request_id = wire["request_id"].as_str().map(str::to_owned);
        let control_request = frame_type == "control_request";
        let control_response = frame_type == "control_response";
        let frame = match serde_json::from_value(wire) {
            Ok(frame) => frame,
            Err(_) if control_request => {
                let id = request_id.filter(|id| !id.is_empty()).ok_or_else(|| {
                    super::ProviderError::Protocol("Claude control request has no valid id".into())
                })?;
                self.seen_requests.insert(id.clone());
                return Ok(Decoded {
                    events: Vec::new(),
                    controls: Vec::new(),
                    replies: vec![Self::unsupported_request(id)],
                    provider_turn: None,
                    #[cfg(debug_assertions)]
                    recording: None,
                });
            }
            Err(_) if control_response => {
                return Err(super::ProviderError::Protocol(
                    "Claude control response could not be decoded".into(),
                ));
            }
            Err(_) => {
                tracing::info!(
                    frame_type = metadata["type"].as_str().unwrap_or("unknown"),
                    "skipped undecodable Claude frame"
                );
                return Ok(Decoded::default());
            }
        };
        self.frame(metadata, frame)
    }
    fn frame(&mut self, wire: Value, mut frame: Frame) -> Result<Decoded, super::ProviderError> {
        let mut decoded = Decoded::default();
        let parent = match &frame {
            Frame::Assistant(frame) => frame.parent_tool_use_id.as_deref(),
            Frame::User(frame) => frame.parent_tool_use_id.as_deref(),
            Frame::StreamEvent(frame) => frame.parent_tool_use_id.as_deref(),
            _ => None,
        };
        if let Some(parent) = parent {
            if let Some(tool) = self.tools.get_mut(parent)
                && let Some(codec) = &mut tool.subagent
            {
                if let Frame::User(user) = &frame {
                    if let Some(content) = user.message["content"].as_array() {
                        for block in content {
                            let text = block["text"].as_str().or_else(|| block["content"].as_str());
                            if let Some(text) = text {
                                decoded.events.push(ProviderEvent::Turn {
                                    turn: self.active.and_then(ActiveTurn::id),
                                    event: TurnEvent::ToolOutput {
                                        key: tool.key.clone(),
                                        text: text.into(),
                                    },
                                });
                            }
                        }
                    }
                } else {
                    match &mut frame {
                        Frame::Assistant(frame) => frame.parent_tool_use_id = None,
                        Frame::User(frame) => frame.parent_tool_use_id = None,
                        Frame::StreamEvent(frame) => frame.parent_tool_use_id = None,
                        _ => {}
                    }
                    for event in codec.frame(wire, frame)?.events {
                        if let ProviderEvent::Turn {
                            event: TurnEvent::TextDelta { text, .. },
                            ..
                        } = event
                        {
                            decoded.events.push(ProviderEvent::Turn {
                                turn: self.active.and_then(ActiveTurn::id),
                                event: TurnEvent::ToolOutput {
                                    key: tool.key.clone(),
                                    text,
                                },
                            });
                        }
                    }
                }
            } else {
                log_ignored("subagent", &wire);
            }
            return Ok(decoded);
        }
        let mut events = Vec::new();
        match frame {
            Frame::ControlResponse(reply) if reply.response.request_id == INITIALIZE_ID => {
                if reply.response.subtype == "error" {
                    return Err(super::ProviderError::Protocol(reply.response.error));
                }
                decoded
                    .events
                    .push(self.ready(None, reply.response.response.capabilities));
            }
            Frame::System(system) => self.system(system, &mut decoded),
            Frame::CommandLifecycle(lifecycle) => {
                if self
                    .prompt()
                    .is_some_and(|turn| lifecycle.command_uuid == turn.to_string())
                {
                    self.echo = EchoMode::Known;
                    if ["queued", "started"].contains(&lifecycle.state.as_str()) {
                        events.push(TurnEvent::Accepted);
                    }
                } else if self.active.is_none()
                    && ["queued", "started"].contains(&lifecycle.state.as_str())
                    && self.seen_commands.insert(lifecycle.command_uuid)
                {
                    events.push(TurnEvent::Started);
                }
            }
            Frame::User(user) => self.user(user, &mut events),
            Frame::StreamEvent(stream) => {
                self.stream(stream.event, &wire, &mut events)?;
            }
            Frame::Assistant(assistant) => self.assistant(assistant, &mut events)?,
            Frame::Result(result) => {
                if self.owns_result(&result) {
                    events.push(TurnEvent::TurnEnded {
                        outcome: self.end_outcome(result.outcome()),
                        park: self.error.as_ref().is_some_and(|error| error.park),
                    });
                }
            }
            Frame::ConversationReset => decoded.events.push(Self::notice(
                "Claude cleared this conversation. The CLI no longer sees the messages above this notice.".into())),
            Frame::RateLimitEvent { rate_limit_info } => {
                if rate_limit_info.status != "allowed" {
                    let reset = rate_limit_info.resets_at.map_or_else(|| "unknown".into(), |at| at.to_string());
                    decoded.events.push(Self::notice(format!(
                        "Claude usage limit: {}; resets at {}.", rate_limit_info.status, reset)));
                }
            }
            Frame::ControlRequest(request) => {
                if self.seen_requests.insert(request.request_id.clone())
                    && let Some(kind) = self.request(&request)
                {
                    self.requests.insert(request.request_id.clone(), PendingRequest {
                        input: request.request.input,
                        kind: kind.clone(),
                    });
                    events.push(TurnEvent::RequestOpened {
                        native: NativeRequestRef(request.request_id), kind,
                    });
                } else {
                    decoded.replies.push(Self::unsupported_request(request.request_id));
                }
            }
            Frame::ControlResponse(_) | Frame::Unknown => {
                tracing::info!(
                    frame_type = wire["type"].as_str().unwrap_or("unknown"),
                    subtype = wire["subtype"].as_str().unwrap_or(""),
                    "ignored Claude frame"
                );
            }
        }
        if self.active.is_none() && events.iter().any(TurnEvent::opens_provider_turn) {
            let provider_turn = TurnId::new();
            self.active = Some(ActiveTurn::Provider(provider_turn));
            decoded.provider_turn = Some(provider_turn);
        }
        let ended = events
            .iter()
            .any(|event| matches!(event, TurnEvent::TurnEnded { .. }));
        decoded
            .events
            .extend(events.into_iter().map(|event| ProviderEvent::Turn {
                turn: self.prompt(),
                event,
            }));
        if ended {
            self.active = None;
            self.stream = MessageStream::default();
            self.tools.clear();
            self.error = None;
            self.interrupted = None;
            self.requests.clear();
        }
        Ok(decoded)
    }
    #[cfg(test)]
    pub fn interrupt(&mut self) -> bool {
        !self.interrupt_active(StopReason::User).is_empty()
    }
    pub fn interrupt_for(&mut self, turn: TurnId, reason: StopReason) -> bool {
        if self.active.map(ActiveTurn::target) != Some(turn) {
            return false;
        }
        self.interrupted
            .get_or_insert_with(|| super::StopLadder::new(reason));
        true
    }
    pub fn next_deadline(&self) -> Option<tokio::time::Instant> {
        self.interrupted
            .as_ref()
            .map(|interrupt| interrupt.deadline())
    }
    pub fn escalate_interrupt(&mut self) -> Option<crate::platform::Signal> {
        self.interrupted.as_mut()?.escalate()
    }
    fn request(&self, request: &ControlRequest) -> Option<RequestKind> {
        let tool = &request.request;
        if tool.subtype != "can_use_tool"
            || tool.tool_name.is_empty()
            || request.request_id.is_empty()
            || self.requests.contains_key(&request.request_id)
        {
            return None;
        }
        if tool.tool_name == "AskUserQuestion" {
            let questions = tool
                .input
                .get("questions")?
                .as_array()?
                .iter()
                .map(|question| {
                    let text = question["question"].as_str()?.to_owned();
                    if text.is_empty() {
                        return None;
                    }
                    let options = question["options"]
                        .as_array()?
                        .iter()
                        .map(|option| option["label"].as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()?;
                    Some(Question { text, options })
                })
                .collect::<Option<Vec<_>>>()?;
            let texts: BTreeSet<_> = questions.iter().map(|question| &question.text).collect();
            if questions.is_empty() || texts.len() != questions.len() {
                return None;
            }
            Some(RequestKind::Question { questions })
        } else {
            let action = self.approval_action(tool);
            let grant_key = match &action.target {
                ApprovalTarget::Command { command, .. }
                    if tool.input.keys().all(|key| {
                        ["command", "description", "timeout"].contains(&key.as_str())
                    }) =>
                {
                    Some(GrantKey {
                        tool: action.tool.clone(),
                        command: command.clone(),
                    })
                }
                _ => None,
            };
            Some(RequestKind::Approval {
                action: action.with_grant(grant_key),
            })
        }
    }
    fn unsupported_request(id: String) -> Value {
        json!({"type":"control_response","response":{
            "subtype":"error","request_id":id,"error":"This request is not supported by Veyra"
        }})
    }
    fn approval_action(&self, tool: &ToolRequest) -> ProposedAction {
        let mut arguments = tool.input.clone();
        let file_target = match tool.tool_name.as_str() {
            "Write" => Some(("file_path", FileOperation::Write)),
            "Edit" | "MultiEdit" => Some(("file_path", FileOperation::Edit)),
            "Read" => Some(("file_path", FileOperation::Read)),
            "NotebookEdit" => Some(("notebook_path", FileOperation::Edit)),
            _ => None,
        }
        .and_then(|(field, operation)| {
            tool.input
                .get(field)
                .and_then(Value::as_str)
                .map(|path| (field, operation, path))
        })
        .filter(|(field, _, path)| {
            ["file_path", "path", "notebook_path"]
                .into_iter()
                .all(|other| {
                    other == *field
                        || tool
                            .input
                            .get(other)
                            .is_none_or(|value| value.as_str() == Some(*path))
                })
        });
        let target = if let Some(command) = tool
            .input
            .get("command")
            .and_then(Value::as_str)
            .filter(|_| tool.tool_name == "Bash")
        {
            arguments.remove("command");
            ApprovalTarget::Command {
                command: command.into(),
                cwd: Default::default(),
                reason: None,
                preview: Self::preview(arguments),
            }
        } else if let Some((field, operation, path)) = file_target {
            arguments.remove(field);
            let path = std::path::PathBuf::from(path);
            let location = target_location(&self.cwd, &path);
            ApprovalTarget::File {
                operation,
                path,
                location,
                preview: Self::preview(arguments),
            }
        } else {
            ApprovalTarget::Tool {
                preview: Self::preview(arguments),
            }
        };
        ProposedAction::new(tool.tool_name.clone(), target)
    }
    fn tool_call(name: String, arguments: Value) -> ToolCall {
        let input_summary = if name == "Bash" {
            arguments
                .get("command")
                .and_then(Value::as_str)
                .map(str::to_owned)
        } else {
            None
        };
        ToolCall {
            veyra_label: false,
            name,
            arguments,
            input_summary,
        }
    }
    fn preview(arguments: serde_json::Map<String, Value>) -> ApprovalPreview {
        ApprovalPreview::new(if arguments.is_empty() {
            String::new()
        } else {
            serde_json::to_string_pretty(&arguments).expect("JSON tool arguments")
        })
    }
    pub fn answer(&mut self, native: NativeRequestRef, decision: Decision) -> Option<Value> {
        let request = self.requests.get(&native.0)?;
        let response = match request.kind.accepts(&decision)? {
            AcceptedDecision::Allow { .. } => json!({"behavior":"allow"}),
            AcceptedDecision::Deny { .. } => json!({"behavior":"deny", "message":"Denied by you"}),
            AcceptedDecision::Answers { questions, answers } => {
                let mut input = request.input.clone();
                input.insert(
                    "answers".into(),
                    Value::Object(
                        questions
                            .iter()
                            .zip(answers)
                            .map(|(question, answer)| {
                                (question.text.clone(), Value::String(answer.text.clone()))
                            })
                            .collect(),
                    ),
                );
                json!({"behavior":"allow", "updatedInput":input})
            }
        };
        self.requests.remove(&native.0);
        Some(Self::response(native.0, response))
    }
    pub fn interrupt_active(&mut self, reason: StopReason) -> Vec<Outbound> {
        self.active
            .map(|turn| self.interrupt_lines(turn.target(), reason))
            .unwrap_or_default()
    }
    pub fn interrupt_lines(&mut self, turn: TurnId, reason: StopReason) -> Vec<Outbound> {
        if !self.interrupt_for(turn, reason) {
            return Vec::new();
        }
        let mut lines: Vec<_> = self
            .cancel_requests(reason.text())
            .into_iter()
            .map(Outbound::Message)
            .collect();
        lines.push(Outbound::Interrupt {
            turn,
            line: json!({
                "type":"control_request", "request_id":"veyra-interrupt",
                "request":{"subtype":"interrupt"}
            }),
        });
        lines
    }
    pub fn cancel_requests(&mut self, reason: &str) -> Vec<Value> {
        std::mem::take(&mut self.requests)
            .into_keys()
            .map(|id| {
                Self::response(
                    id,
                    json!({
                        "behavior":"deny", "message":reason, "interrupt":true
                    }),
                )
            })
            .collect()
    }
    fn response(id: String, response: Value) -> Value {
        json!({"type":"control_response", "response":{
            "subtype":"success", "request_id":id, "response":response
        }})
    }
    pub fn end_outcome(&self, fallback: TurnOutcome) -> TurnOutcome {
        if let Some(interrupt) = &self.interrupted {
            interrupt.outcome()
        } else if let Some(error) = &self.error {
            TurnOutcome::Failed {
                message: error.message.clone(),
            }
        } else {
            fallback
        }
    }
    fn notice(text: String) -> ProviderEvent {
        ProviderEvent::Session(SessionEvent::Notice { text })
    }
    fn tool(&mut self, id: String, name: &str, key: NativeItemKey) {
        let subagent = matches!(name, "Task" | "Agent")
            .then(|| Box::new(Self::new(self.native.clone(), self.pid, &self.cwd)));
        self.tools.insert(id, Tool { key, subagent });
    }
    fn system(&mut self, system: System, decoded: &mut Decoded) {
        match system.subtype.as_str() {
            "init" => {
                #[cfg(debug_assertions)]
                {
                    decoded.recording = Some(crate::record::Metadata {
                        native: (!system.session_id.is_empty()).then(|| system.session_id.clone()),
                        version: (!system.claude_code_version.is_empty())
                            .then(|| system.claude_code_version.clone()),
                    });
                }
                if !system.session_id.is_empty() && system.session_id != self.native.0 {
                    decoded
                        .events
                        .push(ProviderEvent::Session(SessionEvent::Notice {
                            text: "Claude started a new session".into(),
                        }));
                    self.native = NativeSessionId(system.session_id);
                }
                let version =
                    (!system.claude_code_version.is_empty()).then_some(system.claude_code_version);
                decoded
                    .events
                    .push(self.ready(version, system.capabilities));
            }
            "compact_boundary" => decoded
                .events
                .push(Self::notice("Claude compacted this conversation.".into())),
            "api_retry" => decoded.events.push(Self::notice(format!(
                "Claude is retrying the API (attempt {}) in {} ms.",
                system.attempt, system.retry_delay_ms
            ))),
            "model_refusal_fallback" => decoded.events.push(Self::notice(
                "Claude is falling back to another model.".into(),
            )),
            "background_tasks_changed" => {
                decoded
                    .events
                    .push(ProviderEvent::Session(SessionEvent::BackgroundWork {
                        pending: system
                            .tasks
                            .iter()
                            .any(|task| task.get("ambient") != Some(&Value::Bool(true))),
                    }));
            }
            _ => tracing::info!(
                frame_type = "system",
                subtype = system.subtype,
                "ignored Claude system frame"
            ),
        }
    }
    fn user(&mut self, user: User, events: &mut Vec<TurnEvent>) {
        if user.is_replay
            && self
                .prompt()
                .is_some_and(|turn| user.uuid.is_empty() || user.uuid == turn.to_string())
        {
            if !user.uuid.is_empty() {
                self.echo = EchoMode::Known;
            }
            events.push(TurnEvent::Accepted);
        }
        if let Some(content) = user.message.get("content").and_then(Value::as_array) {
            for block in content {
                if block["type"] == "tool_result"
                    && let Some(tool) = block["tool_use_id"]
                        .as_str()
                        .and_then(|id| self.tools.remove(id))
                {
                    let output = block["content"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| block["content"].to_string());
                    events.push(TurnEvent::ToolFinished {
                        key: tool.key,
                        ok: block["is_error"] != true,
                        output_tail: Some(output),
                    });
                }
            }
        }
    }
    fn stream(
        &mut self,
        event: Event,
        wire: &Value,
        events: &mut Vec<TurnEvent>,
    ) -> Result<(), super::ProviderError> {
        let unknown = match &event {
            Event::Unknown => Some(("stream_event", &["event"][..])),
            Event::ContentBlockStart {
                content_block: ContentBlock::Unknown,
                ..
            } => Some(("content_block", &["event", "content_block"][..])),
            Event::ContentBlockDelta {
                delta: Delta::Unknown,
                ..
            } => Some(("delta", &["event", "delta"][..])),
            _ => None,
        };
        if let Some((kind, path)) = unknown {
            log_ignored(kind, path.iter().fold(wire, |value, key| &value[*key]));
        }
        match event {
            Event::MessageStart { message } => {
                if self.active.is_none() {
                    events.push(TurnEvent::Started);
                }
                self.stream = MessageStream {
                    message: message["id"].as_str().unwrap_or_default().into(),
                    ..MessageStream::default()
                };
            }
            Event::ContentBlockStart {
                index,
                content_block,
            } => {
                let kind = content_block.kind();
                let key = NativeItemKey(format!("{}:{index}:{kind}", self.stream.message));
                if let ContentBlock::ToolUse { id, name, .. } = &content_block {
                    self.tool(id.clone(), name, key.clone());
                }
                let text_filter = if self.interrupted.is_some()
                    && matches!(content_block, ContentBlock::Text { .. })
                {
                    let prefix = if let ContentBlock::Text { text } = &content_block {
                        text.clone()
                    } else {
                        String::new()
                    };
                    if prefix.starts_with("[ede_diagnostic]") {
                        TextFilter::Drop
                    } else {
                        TextFilter::Prefix(prefix)
                    }
                } else {
                    TextFilter::Plain
                };
                self.stream.blocks.insert(
                    index,
                    Block {
                        key,
                        content: content_block,
                        input: String::new(),
                        streamed: false,
                        text_filter,
                    },
                );
            }
            Event::ContentBlockDelta { index, delta } => {
                if let Some(block) = self.stream.blocks.get_mut(&index) {
                    match delta {
                        Delta::Text { text } => {
                            block.streamed = true;
                            if let Some(text) = block.text_filter.text(text) {
                                events.push(TurnEvent::TextDelta {
                                    key: block.key.clone(),
                                    text,
                                    streamed: true,
                                });
                            }
                        }
                        Delta::Thinking { thinking } => {
                            if thinking.is_empty() {
                                return Ok(());
                            }
                            block.streamed = true;
                            events.push(TurnEvent::ReasoningDelta {
                                key: block.key.clone(),
                                text: thinking,
                            });
                        }
                        Delta::InputJson { partial_json } => block.input.push_str(&partial_json),
                        Delta::Unknown => {}
                    }
                }
            }
            Event::ContentBlockStop { index } => {
                if let Some(block) = self.stream.blocks.get_mut(&index) {
                    if let TextFilter::Prefix(text) = &mut block.text_filter
                        && !text.is_empty()
                    {
                        events.push(TurnEvent::TextDelta {
                            key: block.key.clone(),
                            text: std::mem::take(text),
                            streamed: true,
                        });
                    }
                    if let ContentBlock::ToolUse { name, input, .. } = &block.content {
                        events.push(TurnEvent::ToolStarted {
                            key: block.key.clone(),
                            call: Self::tool_call(
                                name.clone(),
                                serde_json::from_str(&block.input)
                                    .unwrap_or_else(|_| input.clone()),
                            ),
                        });
                    } else if block.streamed {
                        events.push(TurnEvent::ItemEnded {
                            key: block.key.clone(),
                        });
                    }
                }
            }
            Event::Unknown => {}
        }
        Ok(())
    }
    fn assistant(
        &mut self,
        mut assistant: Assistant,
        events: &mut Vec<TurnEvent>,
    ) -> Result<(), super::ProviderError> {
        if let Some(error) = &assistant.error {
            self.error = Some(AssistantError::new(error));
        }
        let message = assistant.message["id"].as_str().map(str::to_owned);
        if let Some(Value::Array(content)) = assistant.message.get_mut("content").map(Value::take) {
            for value in content {
                let kind = value["type"].as_str().unwrap_or_default().to_owned();
                let count = self
                    .stream
                    .final_blocks
                    .entry((message.clone().unwrap_or_default(), kind.clone()))
                    .or_default();
                let ordinal = *count;
                *count += 1;
                let streamed = (message.as_deref() == Some(self.stream.message.as_str()))
                    .then(|| {
                        self.stream
                            .blocks
                            .values()
                            .filter(|block| block.content.kind() == kind)
                            .nth(ordinal)
                    })
                    .flatten();
                if streamed.is_some_and(|block| {
                    block.streamed || matches!(block.content, ContentBlock::ToolUse { .. })
                }) {
                    continue;
                }
                let block: ContentBlock = serde_json::from_value(value)?;
                let key = streamed.map(|block| block.key.clone()).unwrap_or_else(|| {
                    NativeItemKey(format!(
                        "{}:{ordinal}:{}",
                        message.as_deref().unwrap_or_default(),
                        block.kind()
                    ))
                });
                match block {
                    ContentBlock::Text { text } => {
                        if self.interrupted.is_some() && text.starts_with("[ede_diagnostic]") {
                            continue;
                        }
                        events.push(TurnEvent::TextDelta {
                            key: key.clone(),
                            text,
                            streamed: false,
                        });
                    }
                    ContentBlock::Thinking { thinking } => {
                        if thinking.is_empty() {
                            continue;
                        }
                        events.push(TurnEvent::ReasoningDelta {
                            key: key.clone(),
                            text: thinking,
                        });
                    }
                    ContentBlock::Unknown => {
                        log_ignored("content_block", &json!({"type":kind}));
                        continue;
                    }
                    ContentBlock::ToolUse { id, name, input } => {
                        self.tool(id, &name, key.clone());
                        events.push(TurnEvent::ToolStarted {
                            key,
                            call: Self::tool_call(name, input),
                        });
                        continue;
                    }
                }
                events.push(TurnEvent::ItemEnded { key });
            }
        }
        Ok(())
    }
    fn owns_result(&mut self, result: &ResultFrame) -> bool {
        let echoes: Vec<_> = result
            .user_message_uuids
            .iter()
            .chain(result.user_message_uuid.iter())
            .collect();
        if !echoes.is_empty() {
            self.echo = EchoMode::Known;
            return self
                .prompt()
                .is_some_and(|turn| echoes.iter().any(|uuid| **uuid == turn.to_string()));
        }
        self.prompt().is_none()
            || !matches!(self.echo, EchoMode::Known)
            || !result
                .origin
                .as_ref()
                .is_some_and(|origin| origin["kind"] != "human")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Scratch;
    fn codec() -> Codec {
        Codec::new(
            NativeSessionId("fixture-session".into()),
            1,
            std::path::Path::new("/tmp"),
        )
    }
    fn approval(
        codec: &mut Codec,
        id: &str,
        tool: &str,
        input: Value,
    ) -> (ProposedAction, Option<GrantKey>) {
        let decoded = codec
            .value(json!({"type":"control_request","request_id":id,
            "request":{"subtype":"can_use_tool","tool_name":tool,"input":input}}))
            .unwrap();
        let ProviderEvent::Turn {
            event:
                TurnEvent::RequestOpened {
                    kind: RequestKind::Approval { action },
                    ..
                },
            ..
        } = &decoded.events[0]
        else {
            panic!("missing approval")
        };
        (action.clone(), action.consent.grant_key().cloned())
    }
    #[test]
    fn tool_rows_receive_provider_neutral_input_summaries() {
        for (name, input, expected) in [
            ("Bash", json!({"command":"echo ok"}), "echo ok"),
            (
                "mcp__local__run",
                json!({"command":"remote operation"}),
                "command: remote operation",
            ),
            (
                "Write",
                json!({"content":"contents", "file_path":"/tmp/x"}),
                "content: contents\nfile_path: /tmp/x",
            ),
        ] {
            let mut codec = codec();
            let decoded = codec
                .value(json!({"type":"assistant","message":{"id":"summary",
                "content":[{"type":"tool_use","id":"tool", "name":name,"input":input}]}}))
                .unwrap();
            let call = decoded
                .events
                .iter()
                .find_map(|event| match event {
                    ProviderEvent::Turn {
                        event: TurnEvent::ToolStarted { call, .. },
                        ..
                    } => Some(call),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                call.input_summary.as_deref(),
                (name == "Bash").then_some(expected)
            );
            assert_eq!(call.arguments, input);
        }
        let old: ToolCall =
            serde_json::from_value(json!({"name":"old tool", "arguments":{"value":"old input"}}))
                .unwrap();
        assert_eq!(old.input_summary, None);
        assert_eq!(old.arguments, json!({"value":"old input"}));
    }
    #[test]
    fn approval_target_uses_the_tools_field_without_decoys() {
        let mut codec = codec();
        for (index, (tool, input)) in [
            (
                "NotebookEdit",
                json!({"file_path":"/tmp/inside.txt",
                "notebook_path":"/outside/book.ipynb", "new_source":"cell"}),
            ),
            (
                "Write",
                json!({"path":"/tmp/decoy.txt", "content":"contents"}),
            ),
            (
                "Read",
                json!({"file_path":"/tmp/inside.txt", "path":"/outside/file.txt"}),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let (action, grant_key) =
                approval(&mut codec, &format!("decoy-{index}"), tool, input.clone());
            let ApprovalTarget::Tool { preview } = &action.target else {
                panic!("conflicting or missing target must not claim a file location")
            };
            assert_eq!(serde_json::from_str::<Value>(&preview.text).unwrap(), input);
            assert!(grant_key.is_none());
        }
    }
    #[test]
    fn approval_relative_and_tilde_targets_are_not_verified() {
        let mut codec = codec();
        for (index, target) in ["~/outside.txt", "relative.txt"].into_iter().enumerate() {
            let (action, _) = approval(
                &mut codec,
                &format!("relative-{index}"),
                "Write",
                json!({"file_path":target,"content":"contents"}),
            );
            assert!(
                matches!(
                    &action.target,
                    ApprovalTarget::File {
                        location: TargetLocation::NotVerified,
                        ..
                    }
                ),
                "relative and tilde targets must never read inside: {action:?}"
            );
        }
    }
    #[test]
    fn approval_grants_only_the_displayed_bash_command() {
        let mut codec = codec();
        for (index, (tool, input, grant)) in [
            ("mcp__local__run", json!({"command":"echo ok"}), false),
            ("Write", json!({"command":"echo ok"}), false),
            (
                "Bash",
                json!({"command":"echo ok", "description":"simple", "timeout":1000}),
                true,
            ),
            (
                "Bash",
                json!({"command":"echo ok", "run_in_background":true}),
                false,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let (action, grant_key) = approval(&mut codec, &format!("grant-{index}"), tool, input);
            assert_eq!(
                grant_key.is_some(),
                grant,
                "{tool} must share the dock command rule"
            );
            if let Some(key) = grant_key {
                assert_eq!(key.command, "echo ok");
                assert!(
                    matches!(&action.target, ApprovalTarget::Command { command, .. } if command == &key.command)
                );
            }
        }
    }
    #[test]
    fn interrupted_stream_diagnostic_emits_no_text() {
        let mut codec = codec();
        codec.start(TurnId::new(), "test".into());
        codec.interrupt();
        let frames = [
            json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"diagnostic"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_start","index":0,
                "content_block":{"type":"text","text":""}}}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,
                "delta":{"type":"text_delta","text":"[ede_"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,
                "delta":{"type":"text_delta","text":"diagnostic] abort details"}}}),
            json!({"type":"stream_event","event":{"type":"content_block_stop","index":0}}),
        ];
        let events: Vec<_> = frames
            .iter()
            .flat_map(|frame| codec.line(&frame.to_string()).unwrap().events)
            .collect();
        assert!(!events.iter().any(|event| matches!(
            event,
            ProviderEvent::Turn {
                event: TurnEvent::TextDelta { .. },
                ..
            }
        )));
        codec
            .value(json!({"type":"stream_event", "event":{
                "type":"content_block_start", "index":1,
                "content_block":{"type":"text", "text":"[ede_diagnostic] abort details"}
            }}))
            .unwrap();
        let stopped = codec
            .value(json!({"type":"stream_event", "event":{
                "type":"content_block_stop", "index":1
            }}))
            .unwrap();
        assert!(!stopped.events.iter().any(|event| matches!(
            event,
            ProviderEvent::Turn {
                event: TurnEvent::TextDelta { .. },
                ..
            }
        )));
        let ordinary = codec
            .line(
                &json!({"type":"assistant","message":{"id":"ordinary",
            "content":[{"type":"text","text":"A normal reply"}]}})
                .to_string(),
            )
            .unwrap();
        assert!(
            ordinary
                .events
                .iter()
                .any(|event| matches!(event, ProviderEvent::Turn {
            event: TurnEvent::TextDelta { text, .. }, .. } if text == "A normal reply"))
        );
    }
    #[test]
    fn aborted_reason_ends_interrupted() {
        let mut codec = codec();
        let turn = TurnId::new();
        codec.start(turn, "reply".into());
        let decoded = codec
            .line(
                &json!({"type":"result", "subtype":"error_during_execution",
            "is_error":false, "errors":["Request was aborted"],
            "terminal_reason":"aborted_streaming"})
                .to_string(),
            )
            .unwrap();
        assert!(matches!(&decoded.events[0], ProviderEvent::Turn {
            event: TurnEvent::TurnEnded { outcome: TurnOutcome::Interrupted { reason }, .. }, ..
        } if reason == "Claude ended with aborted_streaming"));
    }
    #[test]
    fn unknown_wire_types_log_types_without_content() {
        use std::sync::{Arc, Mutex};
        use tracing::{
            Event, Metadata, Subscriber,
            field::{Field, Visit},
            span::{Attributes, Id, Record},
        };
        #[derive(Clone, Default)]
        struct Capture(Arc<Mutex<Vec<BTreeMap<String, String>>>>);
        #[derive(Default)]
        struct Fields(BTreeMap<String, String>);
        impl Visit for Fields {
            fn record_str(&mut self, field: &Field, value: &str) {
                self.0.insert(field.name().into(), value.into());
            }
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                self.0.insert(field.name().into(), format!("{value:?}"));
            }
        }
        impl Subscriber for Capture {
            fn enabled(&self, metadata: &Metadata<'_>) -> bool {
                *metadata.level() == tracing::Level::INFO
            }
            fn new_span(&self, _: &Attributes<'_>) -> Id {
                Id::from_u64(1)
            }
            fn record(&self, _: &Id, _: &Record<'_>) {}
            fn record_follows_from(&self, _: &Id, _: &Id) {}
            fn enter(&self, _: &Id) {}
            fn exit(&self, _: &Id) {}
            fn event(&self, event: &Event<'_>) {
                let mut fields = Fields::default();
                event.record(&mut fields);
                self.0.lock().unwrap().push(fields.0);
            }
        }
        let capture = Capture::default();
        let parallel_registration = tracing::Dispatch::new(capture.clone());
        tracing::subscriber::with_default(capture.clone(), || {
            tracing::callsite::rebuild_interest_cache();
            let mut codec = codec();
            for wire in [
                json!({"type":"stream_event","event":{"type":"future_event","content":"private-content"}}),
                json!({"type":"stream_event","event":{"type":"content_block_delta",
                    "delta":{"type":"future_delta","content":"private-content"}}}),
                json!({"type":"stream_event","event":{"type":"content_block_start",
                    "content_block":{"type":"future_block","content":"private-content"}}}),
                json!({"type":"assistant","message":{"content":[
                    {"type":"future_final_block","content":"private-content"}]}}),
            ] {
                codec.line(&wire.to_string()).unwrap();
            }
            let turn = TurnId::new();
            codec.start(turn, "reply".into());
            codec
                .line(&json!({"type":"result","user_message_uuid":"foreign-prompt"}).to_string())
                .unwrap();
            codec
                .line(
                    &json!({"type":"command_lifecycle","command_uuid":"foreign-prompt",
                "state":"completed"})
                    .to_string(),
                )
                .unwrap();
        });
        drop(parallel_registration);
        let logs = capture.0.lock().unwrap();
        assert_eq!(
            logs.iter()
                .map(|fields| (fields["kind"].as_str(), fields["frame_type"].as_str()))
                .collect::<Vec<_>>(),
            vec![
                ("stream_event", "future_event"),
                ("delta", "future_delta"),
                ("content_block", "future_block"),
                ("content_block", "future_final_block"),
            ]
        );
        assert!(!format!("{logs:?}").contains("private-content"));
        drop(logs);
        capture.0.lock().unwrap().clear();
        tracing::subscriber::with_default(capture.clone(), || {
            tracing::callsite::rebuild_interest_cache();
            let mut codec = codec();
            for wire in [
                json!({"future":"private-content"}),
                json!({"type":"system","subtype":"init","capabilities":null,"future":"private-content"}),
            ] {
                let decoded = codec.value(wire).unwrap();
                assert!(decoded.events.is_empty());
                assert!(decoded.replies.is_empty());
            }
        });
        let logs = capture.0.lock().unwrap();
        assert_eq!(
            logs.iter()
                .map(|fields| fields["frame_type"].as_str())
                .collect::<Vec<_>>(),
            ["unknown", "system"]
        );
        assert!(!format!("{logs:?}").contains("private-content"));
    }
    #[test]
    fn undecodable_control_request_gets_error_response() {
        let mut codec = codec();
        for (index, request) in [
            json!({"subtype":"can_use_tool","tool_name":null,"input":{}}),
            Value::Null,
            json!({"subtype":"can_use_tool","tool_name":"Write","input":null}),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("bad-{index}");
            let decoded = codec
                .value(json!({"type":"control_request","request_id":id,
                "request":request}))
                .unwrap();
            assert_eq!(
                decoded.replies,
                [json!({"type":"control_response","response":{
                    "subtype":"error","request_id":id,"error":"This request is not supported by Veyra"
                }})]
            );
            assert!(decoded.events.is_empty());
        }
        assert!(
            codec
                .value(json!({"type":"control_request","request_id":42,
            "request":null}))
                .is_err(),
            "an unaddressable request must fail the session"
        );
    }
    #[test]
    fn tool_request_requires_an_input_object() {
        let mut codec = codec();
        for (index, request) in [
            json!({"subtype":"can_use_tool","tool_name":"Bash"}),
            json!({"subtype":"can_use_tool","tool_name":"Bash","input":null}),
            json!({"subtype":"can_use_tool","tool_name":"Bash","input":[]}),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("missing-input-{index}");
            let decoded = codec
                .value(json!({"type":"control_request","request_id":id,
                "request":request}))
                .unwrap();
            assert!(
                decoded.events.is_empty(),
                "malformed input opened an approval"
            );
            assert_eq!(decoded.replies, [Codec::unsupported_request(id)]);
        }
    }
    #[test]
    fn undecodable_initialize_reply_fails_session() {
        let mut codec = codec();
        assert!(
            codec
                .value(json!({"type":"control_response","response":{
                    "subtype":"success","request_id":INITIALIZE_ID,"response":{
                        "models":[{"value":"fixture-model","supportedEffortLevels":null}]
                    }
                }}))
                .is_err(),
            "an undecodable initialize reply cannot leave the Ready gate waiting"
        );
    }
    #[test]
    fn approval_displays_every_argument_as_bounded_typed_data() {
        let mut codec = codec();
        for (index, (name, input)) in [
            ("Write", json!({"file_path":"/tmp/x","content":"file contents"})),
            ("Edit", json!({"file_path":"/tmp/x","old_string":"old text","new_string":"new text"})),
            ("WebFetch", json!({"url":"https://example.test/a","prompt":"read page"})),
            ("NotebookEdit", json!({"notebook_path":"/tmp/book.ipynb","new_source":"cell contents"})),
            ("mcp__local__change", json!({"nested":{"first":1,"second":"every argument"},"flag":true})),
            ("Bash", json!({"command":"echo ok","run_in_background":true,"dangerouslyDisableSandbox":true})),
        ].into_iter().enumerate() {
            let (action, grant_key) = approval(&mut codec, &format!("display-{index}"), name, input.clone());
            let display = serde_json::to_value(&action).unwrap();
            assert!(display.get("target").is_some(), "approval must expose a typed target");
            assert!(display.get("input").is_none(), "raw input must not be stored");
            let preview = &display["target"]["preview"]["text"];
            let mut arguments: Value = serde_json::from_str(preview.as_str().unwrap()).unwrap();
            match &action.target {
                ApprovalTarget::Command { command, .. } => {
                    arguments["command"] = json!(command);
                }
                ApprovalTarget::File { path, .. } => {
                    let field = if name == "NotebookEdit" { "notebook_path" } else { "file_path" };
                    arguments[field] = json!(path);
                }
                ApprovalTarget::Tool { .. } => {}
            }
            assert_eq!(arguments, input, "every argument must be visible exactly once");
            assert!(grant_key.is_none());
        }
    }
    #[test]
    fn unknown_tools_keep_command_and_path_arguments_literal() {
        let mut codec = codec();
        let (action, grant_key) = approval(
            &mut codec,
            "remote-tool",
            "mcp__local__remote",
            json!({"command":"remote operation","path":"remote/not-a-local-file"}),
        );
        let ApprovalTarget::Tool { preview } = &action.target else {
            panic!("unknown tool arguments")
        };
        assert_eq!(
            serde_json::from_str::<Value>(&preview.text).unwrap(),
            json!({"command":"remote operation","path":"remote/not-a-local-file"})
        );
        assert!(grant_key.is_none());
    }
    #[test]
    fn approval_location_resolves_deepest_existing_ancestor() {
        use std::path::Path;
        let root = Scratch(Path::new("/tmp").join(format!("veyra-approval-{}", ThreadId::new())));
        let project = root.0.join("project");
        let outside = root.0.join("outside");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir(&outside).unwrap();
        crate::platform::test_symlink(&outside, &project.join("linked")).unwrap();
        crate::platform::test_symlink(&root.0.join("absent"), &project.join("dangling")).unwrap();
        let mut codec = Codec::new(NativeSessionId("fixture-session".into()), 1, &project);
        for (index, (path, expected)) in [
            ("linked/new/file.txt", TargetLocation::OutsideProject),
            ("new/parents/file.txt", TargetLocation::InsideProject),
            ("linked/new/../file.txt", TargetLocation::NotVerified),
            ("dangling/new/file.txt", TargetLocation::NotVerified),
            ("../outside/file.txt", TargetLocation::OutsideProject),
        ]
        .into_iter()
        .enumerate()
        {
            let (action, _) = approval(
                &mut codec,
                &format!("path-{index}"),
                "Write",
                json!({"file_path":project.join(path),"content":"contents"}),
            );
            assert!(
                matches!(&action.target, ApprovalTarget::File { path: absolute, location, .. }
                if absolute == &project.join(path) && absolute.is_absolute() && *location == expected)
            );
        }
        assert_eq!(
            target_location(&root.0.join("missing-project"), &project.join("file")),
            TargetLocation::NotVerified
        );
    }
    #[test]
    fn approval_content_preview_is_cut_with_notice() {
        let mut codec = codec();
        let (action, _) = approval(
            &mut codec,
            "long-content",
            "Write",
            json!({"file_path":"/tmp/x","content":"界".repeat(10001)}),
        );
        let display = serde_json::to_value(&action).unwrap();
        assert_eq!(display["target"]["preview"]["truncated"], true);
        assert_eq!(
            display["target"]["preview"]["text"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            10000
        );
    }
    #[test]
    fn approval_preserves_typed_display_and_grants() {
        let mut codec = codec();
        codec.start(TurnId::new(), "reply".into());
        for (index, (tool, input, grant)) in [
            (
                "Write",
                json!({"file_path":"/tmp/x","content":"private contents"}),
                false,
            ),
            (
                "Bash",
                json!({"command":"{\"file_path\":\"/tmp/x\"}"}),
                true,
            ),
            (
                "Bash",
                json!({"command":"echo ok","extra":"blocks grant"}),
                false,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let (action, grant_key) = approval(
                &mut codec,
                &format!("approval-{index}"),
                tool,
                input.clone(),
            );
            assert_eq!(action.tool, tool);
            if let Some(command) = input["command"].as_str() {
                assert!(
                    matches!(&action.target, ApprovalTarget::Command { command: text, .. } if text == command)
                );
            } else {
                assert!(
                    matches!(&action.target, ApprovalTarget::File { operation: FileOperation::Write,
                    path, preview, .. } if path == std::path::Path::new("/tmp/x")
                        && preview.text == "{\n  \"content\": \"private contents\"\n}")
                );
            }
            assert_eq!(grant_key.is_some(), grant);
        }
        let old = json!({"tool":"Bash", "command":"echo old"});
        let action: ProposedAction = serde_json::from_value(old.clone()).unwrap();
        assert!(
            matches!(&action.target, ApprovalTarget::Command { command, preview, .. }
            if command == "echo old" && preview.text.is_empty() && !preview.truncated)
        );
        let stored = serde_json::to_value(&action).unwrap();
        assert_eq!(
            serde_json::from_value::<ProposedAction>(stored).unwrap(),
            action
        );
    }
    #[test]
    fn storage_interruption_names_database() {
        let mut codec = codec();
        let turn = TurnId::new();
        codec.start(turn, "reply".into());
        assert!(codec.interrupt_for(turn, StopReason::StorageBlocked));
        let decoded = codec
            .value(json!({"type":"result", "subtype":"success"}))
            .unwrap();
        assert!(
            decoded
                .events
                .iter()
                .any(|event| matches!(event, ProviderEvent::Turn {
            event: TurnEvent::TurnEnded { outcome: TurnOutcome::Interrupted { reason }, .. }, ..
        } if reason == "Stopped because the database is blocked"))
        );
        assert!(codec.interrupted.is_none());
        assert!(!codec.interrupt_for(turn, StopReason::User));
    }
    #[test]
    fn empty_final_reasoning_adds_no_item() {
        let mut codec = codec();
        let turn = TurnId::new();
        codec.start(turn, "reply".into());
        let decoded = codec
            .line(
                &json!({
                    "type":"assistant", "message":{"id":"fixture-message", "content":[
                        {"type":"thinking", "thinking":""}, {"type":"text", "text":"ok"}
                    ]}
                })
                .to_string(),
            )
            .unwrap();
        assert_eq!(decoded.events.len(), 2);
        assert!(matches!(&decoded.events[0], ProviderEvent::Turn {
            turn: Some(id), event: TurnEvent::TextDelta { text, .. }
        } if *id == turn && text == "ok"));
        assert!(matches!(
            decoded.events[1],
            ProviderEvent::Turn {
                event: TurnEvent::ItemEnded { .. },
                ..
            }
        ));
    }
    #[test]
    fn final_blocks_match_message_and_occurrence_within_kind() {
        let mut codec = codec();
        codec.start(TurnId::new(), "reply".into());
        codec
            .line(
                &json!({"type":"stream_event", "event":{
                    "type":"message_start", "message":{"id":"first"}
                }})
                .to_string(),
            )
            .unwrap();
        for (index, text) in [(1, "one"), (2, "two")] {
            codec
                .line(
                    &json!({"type":"stream_event", "event":{
                        "type":"content_block_start", "index":index,
                        "content_block":{"type":"text", "text":""}
                    }})
                    .to_string(),
                )
                .unwrap();
            let decoded = codec
                .line(
                    &json!({"type":"stream_event", "event":{
                        "type":"content_block_delta", "index":index,
                        "delta":{"type":"text_delta", "text":text}
                    }})
                    .to_string(),
                )
                .unwrap();
            assert!(matches!(&decoded.events[0], ProviderEvent::Turn {
                event: TurnEvent::TextDelta { text: value, .. }, ..
            } if value == text));
        }
        let content = json!([
            {"type":"text", "text":42}, {"type":"future_block"}, {"type":"text", "text":"two"}
        ]);
        let decoded = codec
            .line(
                &json!({"type":"assistant", "message":{
                    "id":"first", "content":content
                }})
                .to_string(),
            )
            .unwrap();
        assert_eq!(decoded.events.len(), 0);
        let decoded = codec
            .line(
                &json!({"type":"assistant", "message":{
                    "id":"second", "content":[{"type":"text", "text":"new reply"}]
                }})
                .to_string(),
            )
            .unwrap();
        assert!(matches!(&decoded.events[0], ProviderEvent::Turn {
            event: TurnEvent::TextDelta { key, text, streamed: false }, ..
        } if key.0 == "second:0:text" && text == "new reply"));
        assert_eq!(decoded.events.len(), 2);
    }
    #[test]
    fn split_final_frames_match_each_same_kind_block_and_reset_on_message_start() {
        let mut codec = codec();
        codec.start(TurnId::new(), "reply".into());
        for message in ["first", "second", "first"] {
            codec
                .line(
                    &json!({"type":"stream_event", "event":{
                        "type":"message_start", "message":{"id":message}
                    }})
                    .to_string(),
                )
                .unwrap();
            for index in [0, 1] {
                codec
                    .line(
                        &json!({"type":"stream_event", "event":{
                            "type":"content_block_start", "index":index,
                            "content_block":{"type":"text", "text":""}
                        }})
                        .to_string(),
                    )
                    .unwrap();
            }
            let streamed = codec
                .line(
                    &json!({"type":"stream_event", "event":{
                        "type":"content_block_delta", "index":1,
                        "delta":{"type":"text_delta", "text":"reply"}
                    }})
                    .to_string(),
                )
                .unwrap();
            assert!(matches!(&streamed.events[0], ProviderEvent::Turn {
                event: TurnEvent::TextDelta { key, text, streamed: true }, ..
            } if key.0 == format!("{message}:1:text") && text == "reply"));
            let empty = codec
                .line(
                    &json!({"type":"assistant", "message":{
                        "id":message, "content":[{"type":"text", "text":""}]
                    }})
                    .to_string(),
                )
                .unwrap();
            assert!(matches!(&empty.events[0], ProviderEvent::Turn {
                event: TurnEvent::TextDelta { key, text, streamed: false }, ..
            } if key.0 == format!("{message}:0:text") && text.is_empty()));
            let final_reply = codec
                .line(
                    &json!({"type":"assistant", "message":{
                        "id":message, "content":[{"type":"text", "text":"reply"}]
                    }})
                    .to_string(),
                )
                .unwrap();
            assert_eq!(
                final_reply.events.len(),
                0,
                "a streamed block must not be repeated"
            );
        }
    }
    #[test]
    fn terminal_reason_overrides_success_subtype() {
        for (reason, expected) in [
            ("completed", TurnOutcome::Completed),
            (
                "api_error",
                TurnOutcome::Failed {
                    message: "Claude ended with api_error".into(),
                },
            ),
            (
                "aborted_tools",
                TurnOutcome::Interrupted {
                    reason: "Claude ended with aborted_tools".into(),
                },
            ),
        ] {
            let mut codec = Codec::new(
                NativeSessionId("native".into()),
                1,
                std::path::Path::new("/tmp"),
            );
            let turn = TurnId::new();
            codec.start(turn, "prompt".into());
            let decoded = codec
                .line(
                    &json!({"type":"result","subtype":"success","terminal_reason":reason})
                        .to_string(),
                )
                .unwrap();
            assert!(matches!(
                &decoded.events[0],
                ProviderEvent::Turn { turn: Some(id), event: TurnEvent::TurnEnded { outcome, .. } }
                    if *id == turn && *outcome == expected
            ));
        }
    }
    #[test]
    fn changed_native_session_emits_notice_before_binding() {
        let mut codec = Codec::new(
            NativeSessionId("stored".into()),
            1,
            std::path::Path::new("/tmp"),
        );
        let turn = TurnId::new();
        codec.start(turn, "prompt".into());
        let wire = json!({"type":"system", "subtype":"init", "session_id":"replacement"});
        let decoded = codec.line(&wire.to_string()).unwrap();
        assert!(matches!(&decoded.events[0],
            ProviderEvent::Session(SessionEvent::Notice { text }) if text == "Claude started a new session"
        ));
        assert!(matches!(&decoded.events[1],
            ProviderEvent::Session(SessionEvent::Ready { native, cli_version: None, .. })
                if native.0 == "replacement"
        ));
        assert_eq!(codec.active.and_then(ActiveTurn::id), Some(turn));
        assert_eq!(codec.line(&wire.to_string()).unwrap().events.len(), 1);
    }
    #[test]
    fn claude_args_per_mode() {
        let mut spec = SessionSpec {
            thread: ThreadId::new(),
            cwd: "/tmp".into(),
            config: ThreadConfig {
                model: None,
                effort: None,
                mode: PermissionMode::AskFirst,
            },
            native: None,
            import: false,
            root: false,
        };
        for (mode, expected, dangerous) in [
            (PermissionMode::AskFirst, "default", false),
            (PermissionMode::AcceptEdits, "acceptEdits", false),
            (PermissionMode::FullAccess, "bypassPermissions", true),
        ] {
            spec.config.mode = mode;
            let args = args(Some(&spec));
            let index = args
                .iter()
                .position(|arg| arg == "--permission-mode")
                .unwrap();
            assert_eq!(args[index + 1], expected);
            assert_eq!(
                args.iter()
                    .any(|arg| arg == "--allow-dangerously-skip-permissions"),
                dangerous
            );
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--session-id", &spec.thread.to_string()])
            );
        }
        spec.native = Some(NativeSessionId("saved".into()));
        assert!(
            args(Some(&spec))
                .windows(2)
                .any(|pair| pair == ["--resume", "saved"])
        );
        assert!(
            !args(None)
                .iter()
                .any(|arg| arg == "--session-id" || arg == "--resume")
        );
    }
}
