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
        } else if aborted {
            TurnOutcome::Interrupted {
                reason: format!("Claude ended with {}", reason.unwrap()),
            }
        } else {
            TurnOutcome::Completed
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) enum ActiveTurn {
    Veyra(TurnId),
    Provider,
}
impl ActiveTurn {
    pub fn id(self) -> Option<TurnId> {
        match self {
            Self::Veyra(turn) => Some(turn),
            Self::Provider => None,
        }
    }
}
pub(crate) struct Codec {
    pub active: Option<ActiveTurn>,
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
            active: None,
            echo: EchoMode::Unknown,
            native,
            pid,
            message: String::new(),
            blocks: BTreeMap::new(),
            tools: BTreeMap::new(),
        }
    }
    pub fn start(&mut self, turn: TurnId, text: String) -> Value {
        self.active = Some(ActiveTurn::Veyra(turn));
        self.blocks.clear();
        json!({
            "type":"user", "uuid":turn.to_string(), "session_id":self.native.0,
            "parent_tool_use_id":null, "message":{"role":"user","content":text}
        })
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
    pub fn line(&mut self, line: &str) -> Result<Decoded, super::ProviderError> {
        let frame: Frame = serde_json::from_str(line)?;
        let mut decoded = Decoded::default();
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
                {
                    events.push(TurnEvent::Started);
                }
            }
            Frame::User(user) => self.user(user, &mut events),
            Frame::StreamEvent(stream) => {
                let unknown = match &stream.event {
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
                    let wire: Value = serde_json::from_str(line)?;
                    let value = path.iter().fold(&wire, |value, key| &value[*key]);
                    tracing::info!(
                        kind,
                        frame_type = value["type"].as_str().unwrap_or("unknown"),
                        "ignored Claude wire type"
                    );
                }
                self.stream(stream.event, &mut events);
            }
            Frame::Assistant(assistant) => self.assistant(assistant, &mut events)?,
            Frame::Result(result) => {
                if self.owns_result(&result) {
                    events.push(TurnEvent::TurnEnded {
                        outcome: result.outcome(),
                    });
                }
            }
            Frame::ControlRequest(request) => decoded.replies.push(json!({
                "type": "control_response",
                "response": {
                    "subtype": "error", "request_id": request.request_id,
                    "error": "This request is not supported by Veyra"
                }
            })),
            Frame::ControlResponse(_) | Frame::Unknown => {
                let wire: Value = serde_json::from_str(line)?;
                tracing::info!(
                    frame_type = wire["type"].as_str().unwrap_or("unknown"),
                    subtype = wire["subtype"].as_str().unwrap_or(""),
                    "ignored Claude frame"
                );
            }
        }
        if self.active.is_none() && events.iter().any(TurnEvent::opens_provider_turn) {
            self.active = Some(ActiveTurn::Provider);
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
            self.blocks.clear();
            self.tools.clear();
        }
        Ok(decoded)
    }
    fn system(&mut self, system: System, decoded: &mut Decoded) {
        match system.subtype.as_str() {
            "init" => {
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
                    && let Some(key) = block["tool_use_id"]
                        .as_str()
                        .and_then(|id| self.tools.remove(id))
                {
                    let output = block["content"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| block["content"].to_string());
                    events.push(TurnEvent::ToolFinished {
                        key,
                        ok: block["is_error"] != true,
                        output_tail: Some(output),
                    });
                }
            }
        }
    }
    fn stream(&mut self, event: Event, events: &mut Vec<TurnEvent>) {
        match event {
            Event::MessageStart { message } => {
                if self.active.is_none() {
                    events.push(TurnEvent::Started);
                }
                self.message = message["id"].as_str().unwrap_or_default().into();
                self.blocks.clear();
            }
            Event::ContentBlockStart {
                index,
                content_block,
            } => {
                let kind = content_block.kind();
                let key = NativeItemKey(format!("{}:{index}:{kind}", self.message));
                if let ContentBlock::ToolUse { id, .. } = &content_block {
                    self.tools.insert(id.clone(), key.clone());
                }
                self.blocks.insert(
                    index,
                    Block {
                        key,
                        content: content_block,
                        input: String::new(),
                        streamed: false,
                    },
                );
            }
            Event::ContentBlockDelta { index, delta } => {
                if let Some(block) = self.blocks.get_mut(&index) {
                    match delta {
                        Delta::Text { text } => {
                            block.streamed = true;
                            events.push(TurnEvent::TextDelta {
                                key: block.key.clone(),
                                text,
                                streamed: true,
                            });
                        }
                        Delta::Thinking { thinking } => {
                            if thinking.is_empty() {
                                return;
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
                if let Some(block) = self.blocks.get(&index) {
                    if let ContentBlock::ToolUse { name, input, .. } = &block.content {
                        events.push(TurnEvent::ToolStarted {
                            key: block.key.clone(),
                            call: ToolCall {
                                name: name.clone(),
                                arguments: serde_json::from_str(&block.input)
                                    .unwrap_or_else(|_| input.clone()),
                            },
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
    }
    fn assistant(
        &self,
        assistant: Assistant,
        events: &mut Vec<TurnEvent>,
    ) -> Result<(), super::ProviderError> {
        if let Some(content) = assistant.message["content"].as_array() {
            for (index, value) in content.iter().enumerate() {
                let block: ContentBlock = serde_json::from_value(value.clone())?;
                if !matches!(block, ContentBlock::Unknown)
                    && self.blocks.get(&(index as u32)).is_some_and(|block| {
                        block.streamed || matches!(block.content, ContentBlock::ToolUse { .. })
                    })
                {
                    continue;
                }
                let key = NativeItemKey(format!(
                    "{}:{index}:{}",
                    assistant.message["id"].as_str().unwrap_or_default(),
                    block.kind()
                ));
                match block {
                    ContentBlock::Text { text } => events.push(TurnEvent::TextDelta {
                        key: key.clone(),
                        text,
                        streamed: false,
                    }),
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
                        tracing::info!(
                            kind = "content_block",
                            frame_type = value["type"].as_str().unwrap_or("unknown"),
                            "ignored Claude wire type"
                        );
                        continue;
                    }
                    ContentBlock::ToolUse { .. } => continue,
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
        tracing::subscriber::with_default(capture.clone(), || {
            let mut codec = Codec::new(NativeSessionId("fixture-session".into()), 1);
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
    }
    #[test]
    fn empty_final_reasoning_adds_no_item() {
        let mut codec = Codec::new(NativeSessionId("fixture-session".into()), 1);
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
            assert!(matches!(
                &decoded.events[0],
                ProviderEvent::Turn { turn: Some(id), event: TurnEvent::TurnEnded { outcome } }
                    if *id == turn && *outcome == expected
            ));
        }
    }
    #[test]
    fn changed_native_session_emits_notice_before_binding() {
        let mut codec = Codec::new(NativeSessionId("stored".into()), 1);
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
