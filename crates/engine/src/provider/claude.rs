use super::{Model, ProviderEvent, SessionEvent, SessionSpec, TurnEvent};
use crate::domain::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

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
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Stream {
    event: Event,
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
        content_block: Value,
    },
    ContentBlockDelta {
        #[serde(default)]
        index: u32,
        #[serde(default)]
        delta: Value,
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
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Assistant {
    message: Value,
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
}

pub(crate) fn probe_reply(line: &str) -> Result<Option<InitializeReply>, super::ProviderError> {
    let frame: Frame = serde_json::from_str(line)?;
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

#[derive(Default)]
enum EchoMode {
    #[default]
    Unknown,
    Known,
}
struct Block {
    key: NativeItemKey,
    kind: String,
    name: String,
    input: String,
    streamed: bool,
}
pub(crate) struct Codec {
    pub current: Option<TurnId>,
    echo: EchoMode,
    native: NativeSessionId,
    pid: u32,
    message: String,
    blocks: BTreeMap<u32, Block>,
    tools: BTreeMap<String, NativeItemKey>,
}
#[derive(Default)]
pub(crate) struct Decoded {
    pub events: Vec<ProviderEvent>,
    pub replies: Vec<Value>,
}
impl Codec {
    pub fn new(native: NativeSessionId, pid: u32) -> Self {
        Self {
            current: None,
            echo: EchoMode::Unknown,
            native,
            pid,
            message: String::new(),
            blocks: BTreeMap::new(),
            tools: BTreeMap::new(),
        }
    }
    pub fn start(&mut self, turn: TurnId, text: String) -> Value {
        self.current = Some(turn);
        self.blocks.clear();
        json!({"type":"user","uuid":turn.to_string(),"session_id":self.native.0,"parent_tool_use_id":null,"message":{"role":"user","content":text}})
    }
    fn ready(&self, version: String, capabilities: Vec<String>) -> ProviderEvent {
        ProviderEvent::Session(SessionEvent::Ready {
            native: self.native.clone(),
            cli_version: version,
            pid: self.pid,
            pgid: self.pid,
            capabilities,
        })
    }
    pub fn line(&mut self, line: &str) -> Result<Decoded, super::ProviderError> {
        let frame: Frame = serde_json::from_str(line)?;
        let mut decoded = Decoded::default();
        let mut events = Vec::new();
        match frame {
            Frame::ControlResponse(reply) if reply.response.request_id == INITIALIZE_ID => {
                if reply.response.subtype == "error" { return Err(super::ProviderError::Protocol(reply.response.error)); }
                decoded.events.push(self.ready(String::new(), reply.response.response.capabilities));
            }
            Frame::System(system) => match system.subtype.as_str() {
                "init" => {
                    if !system.session_id.is_empty() { self.native = NativeSessionId(system.session_id); }
                    decoded.events.push(self.ready(system.claude_code_version, system.capabilities));
                }
                "background_tasks_changed" => decoded.events.push(ProviderEvent::Session(SessionEvent::BackgroundWork { pending: system.tasks.iter().any(|task| task.get("ambient") != Some(&Value::Bool(true))) })),
                _ => tracing::debug!(subtype = system.subtype, "ignored Claude system frame"),
            },
            Frame::CommandLifecycle(lifecycle) => {
                if self.current.is_some_and(|turn| lifecycle.command_uuid == turn.to_string()) {
                    self.echo = EchoMode::Known;
                    if ["queued", "started"].contains(&lifecycle.state.as_str()) { events.push(TurnEvent::Accepted); }
                }
            }
            Frame::User(user) => {
                if user.is_replay && self.current.is_some_and(|turn| user.uuid.is_empty() || user.uuid == turn.to_string()) {
                    if !user.uuid.is_empty() { self.echo = EchoMode::Known; }
                    events.push(TurnEvent::Accepted);
                }
                if let Some(content) = user.message.get("content").and_then(Value::as_array) {
                    for block in content {
                        if block["type"] == "tool_result" && let Some(key) = block["tool_use_id"].as_str().and_then(|id| self.tools.remove(id)) {
                            let output = block["content"].as_str().map(str::to_owned).or_else(|| Some(block["content"].to_string()));
                            events.push(TurnEvent::ToolFinished { key, ok: block["is_error"] != true, output_tail: output });
                        }
                    }
                }
            }
            Frame::StreamEvent(stream) => match stream.event {
                Event::MessageStart { message } => { self.message = message["id"].as_str().unwrap_or_default().into(); self.blocks.clear(); }
                Event::ContentBlockStart { index, content_block } => {
                    let kind = content_block["type"].as_str().unwrap_or_default().to_owned();
                    let key = NativeItemKey(format!("{}:{index}:{kind}", self.message));
                    let name = content_block["name"].as_str().unwrap_or_default().to_owned();
                    if kind == "tool_use" && let Some(id) = content_block["id"].as_str() { self.tools.insert(id.into(), key.clone()); }
                    self.blocks.insert(index, Block { key, kind, name, input: String::new(), streamed: false });
                }
                Event::ContentBlockDelta { index, delta } => {
                    if let Some(block) = self.blocks.get_mut(&index) {
                        match delta["type"].as_str() {
                            Some("text_delta") => { block.streamed = true; events.push(TurnEvent::TextDelta { key: block.key.clone(), text: delta["text"].as_str().unwrap_or_default().into() }); }
                            Some("thinking_delta") => { block.streamed = true; events.push(TurnEvent::ReasoningDelta { key: block.key.clone(), text: delta["thinking"].as_str().unwrap_or_default().into() }); }
                            Some("input_json_delta") => block.input.push_str(delta["partial_json"].as_str().unwrap_or_default()),
                            _ => {}
                        }
                    }
                }
                Event::ContentBlockStop { index } => {
                    if let Some(block) = self.blocks.get(&index) {
                        if block.kind == "tool_use" { events.push(TurnEvent::ToolStarted { key: block.key.clone(), call: ToolCall { name: block.name.clone(), arguments: serde_json::from_str(&block.input).unwrap_or(Value::Null) } }); }
                        else if block.streamed { events.push(TurnEvent::ItemEnded { key: block.key.clone() }); }
                    }
                }
                Event::Unknown => {}
            },
            Frame::Assistant(assistant) => {
                if let Some(content) = assistant.message["content"].as_array() {
                    for (index, block) in content.iter().enumerate() {
                        if self.blocks.get(&(index as u32)).is_some_and(|block| block.streamed || block.kind == "tool_use") { continue; }
                        let key = NativeItemKey(format!("{}:{index}:{}", assistant.message["id"].as_str().unwrap_or_default(), block["type"].as_str().unwrap_or_default()));
                        match block["type"].as_str() {
                            Some("text") => events.push(TurnEvent::TextDelta { key: key.clone(), text: block["text"].as_str().unwrap_or_default().into() }),
                            Some("thinking") => events.push(TurnEvent::ReasoningDelta { key: key.clone(), text: block["thinking"].as_str().unwrap_or_default().into() }),
                            _ => continue,
                        }
                        events.push(TurnEvent::ItemEnded { key });
                    }
                }
            }
            Frame::Result(result) => {
                let echoes: Vec<_> = result.user_message_uuids.iter().chain(result.user_message_uuid.iter()).collect();
                let foreign = if !echoes.is_empty() {
                    self.echo = EchoMode::Known;
                    !self.current.is_some_and(|turn| echoes.iter().any(|uuid| **uuid == turn.to_string()))
                } else {
                    matches!(self.echo, EchoMode::Known) && result.origin.as_ref().is_some_and(|origin| origin["kind"] != "human")
                };
                if foreign { return Ok(decoded); }
                let aborted = result.terminal_reason.as_deref().is_some_and(|reason| reason.starts_with("aborted_"));
                let incomplete = result.terminal_reason.as_deref().is_some_and(|reason| !["completed","background_requested","tool_deferred","hook_stopped","stop_hook_prevented"].contains(&reason));
                let outcome = if result.is_error || result.subtype.starts_with("error") || (!aborted && incomplete) || matches!(result.stop_reason.as_deref(), Some("max_tokens" | "refusal")) {
                    TurnOutcome::Failed { message: crate::record::redact(&if !result.errors.is_empty() { result.errors.join("\n") } else if !result.result.is_empty() { result.result } else { format!("Claude ended with {}", result.terminal_reason.as_deref().unwrap_or(&result.subtype)) },None) }
                } else if aborted { TurnOutcome::Interrupted { reason:format!("Claude ended with {}",result.terminal_reason.unwrap()) } }
                else { TurnOutcome::Completed };
                events.push(TurnEvent::TurnEnded { outcome });
            }
            Frame::ControlRequest(request) => decoded.replies.push(json!({"type":"control_response","response":{"subtype":"error","request_id":request.request_id,"error":"This request is not supported by Veyra"}})),
            Frame::ControlResponse(_) | Frame::Unknown => tracing::debug!("ignored Claude frame"),
        }
        let ended = events
            .iter()
            .any(|event| matches!(event, TurnEvent::TurnEnded { .. }));
        decoded
            .events
            .extend(events.into_iter().map(|event| ProviderEvent::Turn {
                turn: self.current,
                event,
            }));
        if ended {
            self.current = None;
            self.blocks.clear();
            self.tools.clear();
        }
        Ok(decoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let mut codec = Codec::new(NativeSessionId("native".into()), 1);
            let turn = TurnId::new();
            codec.start(turn, "prompt".into());
            let decoded = codec
                .line(
                    &json!({"type":"result","subtype":"success","terminal_reason":reason})
                        .to_string(),
                )
                .unwrap();
            assert!(
                matches!(&decoded.events[0],ProviderEvent::Turn { turn:Some(id),event:TurnEvent::TurnEnded { outcome } } if *id==turn && *outcome==expected)
            );
        }
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
