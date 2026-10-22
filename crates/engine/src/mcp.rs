#![allow(clippy::disallowed_types)] // MCP envelopes, schemas and provider configuration are wire JSON.
use crate::{
    actor::EngineMsg,
    domain::*,
    platform,
    provider::session::{ByteLines, Line},
    provider::{Outbound, ProviderError},
    tools,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{mpsc, oneshot, watch},
};

fn remove_stale_configs(data: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(data.join("run")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && uuid::Uuid::parse_str(&entry.file_name().to_string_lossy()).is_ok()
        {
            platform::remove_optional_file(&entry.path().join("mcp.json"))?;
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct McpToken(String);
impl McpToken {
    pub fn new() -> Self {
        Self(
            platform::random_token_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        )
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
    pub fn hash(&self) -> [u8; 32] {
        Sha256::digest(self.0.as_bytes()).into()
    }
}
impl Default for McpToken {
    fn default() -> Self {
        Self::new()
    }
}
impl std::fmt::Debug for McpToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("McpToken(***)")
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenHash(pub [u8; 32]);
impl TokenHash {
    pub fn of(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }
    pub fn redact(&self, text: &str, replacement: &str) -> String {
        static HEX: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new("[0-9a-f]{64,}").unwrap());
        let mut result = String::new();
        let mut copied = 0;
        for run in HEX.find_iter(text) {
            let mut offset = run.start();
            while offset + 64 <= run.end() {
                if Self::of(&text[offset..offset + 64]) == *self {
                    result.push_str(&text[copied..offset]);
                    result.push_str(replacement);
                    copied = offset + 64;
                    offset += 64;
                } else {
                    offset += 1;
                }
            }
        }
        result.push_str(&text[copied..]);
        result
    }
    pub fn sanitize(&self, value: &mut Value, replacement: &str) {
        match value {
            Value::String(text) => *text = self.redact(text, replacement),
            Value::Array(values) => values
                .iter_mut()
                .for_each(|value| self.sanitize(value, replacement)),
            Value::Object(values) => {
                let old = std::mem::take(values);
                for (key, mut value) in old {
                    self.sanitize(&mut value, replacement);
                    values.insert(self.redact(&key, replacement), value);
                }
            }
            _ => {}
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Caller {
    pub epoch: u32,
    pub pid: u32,
    pub token: TokenHash,
}
#[derive(Clone, Debug)]
struct Credential {
    thread: ThreadId,
    epoch: u32,
    pid: watch::Receiver<Option<u32>>,
    used: bool,
}
#[derive(Clone, Debug)]
pub struct Server {
    pub socket: PathBuf,
    pub command: PathBuf,
    credentials: Arc<Mutex<BTreeMap<[u8; 32], Credential>>>,
}
impl Server {
    pub fn bind(
        data: &Path,
        command: PathBuf,
        out: mpsc::Sender<EngineMsg>,
    ) -> std::io::Result<Self> {
        if !platform::is_private(data)? {
            return Err(std::io::Error::other("MCP data folder must be mode 0700"));
        }
        remove_stale_configs(data)?;
        let server = Self {
            socket: data.join("veyra.sock"),
            command: command.canonicalize()?,
            credentials: Default::default(),
        };
        let listener = platform::bind_async_socket(&server.socket)?;
        let this = server.clone();
        tokio::spawn(async move {
            loop {
                let accepted =
                    tokio::select! {result=listener.accept()=>result,_=out.closed()=>break};
                let Ok((socket, _)) = accepted else { break };
                let server = this.clone();
                let out = out.clone();
                tokio::spawn(async move {
                    server.connection(socket, out).await;
                });
            }
            if let Err(error) = platform::remove_optional_file(&this.socket) {
                tracing::warn!(%error,"MCP socket cleanup failed");
            }
        });
        Ok(server)
    }
    pub fn for_spawn<T>(
        &self,
        thread: ThreadId,
        epoch: u32,
        spawn: impl FnOnce(&McpToken) -> Result<(T, u32), crate::provider::ProviderError>,
    ) -> Result<T, crate::provider::ProviderError> {
        let token = McpToken::new();
        let hash = token.hash();
        let (ready, pid) = watch::channel(None);
        {
            let mut credentials = self.credentials.lock().unwrap();
            credentials.retain(|_, entry| entry.thread != thread);
            credentials.insert(
                hash,
                Credential {
                    thread,
                    epoch,
                    pid,
                    used: false,
                },
            );
        }
        match spawn(&token) {
            Ok((value, pid)) => {
                let _ = ready.send(Some(pid));
                Ok(value)
            }
            Err(error) => {
                self.credentials.lock().unwrap().remove(&hash);
                Err(error)
            }
        }
    }
    pub fn revoke(&self, thread: ThreadId, epoch: u32) {
        self.credentials
            .lock()
            .unwrap()
            .retain(|_, entry| entry.thread != thread || entry.epoch != epoch);
    }
    pub(crate) fn allows(&self, thread: ThreadId, caller: Caller) -> bool {
        self.credentials
            .lock()
            .unwrap()
            .get(&caller.token.0)
            .is_some_and(|entry| {
                entry.thread == thread
                    && entry.epoch == caller.epoch
                    && *entry.pid.borrow() == Some(caller.pid)
                    && entry.used
            })
    }
    fn active(&self, hash: &[u8; 32]) -> bool {
        self.credentials.lock().unwrap().contains_key(hash)
    }
    async fn connection(&self, socket: platform::Socket, out: mpsc::Sender<EngineMsg>) {
        let peer = platform::peer_pid(&socket)
            .and_then(|pid| Ok((platform::parent_pid(pid)?, platform::exe_path(pid)?)));
        let (read, mut write) = socket.into_split();
        let mut lines = ByteLines::new(read);
        let first = tokio::time::timeout(Duration::from_secs(3), lines.next()).await;
        let token = match first {
            Ok(Ok(Some(Line::Text(token)))) if token.len() == 64 => token,
            _ => {
                tracing::warn!(
                    thread = "unknown",
                    reason = "missing authentication",
                    "MCP connection refused"
                );
                return;
            }
        };
        let hash: [u8; 32] = Sha256::digest(token.as_bytes()).into();
        drop(token);
        let pending = self.credentials.lock().unwrap().get(&hash).cloned();
        if let Some(mut pending) = pending {
            let registered = tokio::time::timeout(
                Duration::from_secs(3),
                pending.pid.wait_for(|pid| pid.is_some()),
            )
            .await;
            match registered {
                Ok(Ok(_)) => {}
                failed => {
                    let reason = if failed.is_err() {
                        "registration timeout"
                    } else {
                        "closed registration"
                    };
                    tracing::warn!(thread=%pending.thread, reason, "MCP connection refused");
                    return;
                }
            }
        }
        let credential = {
            let mut credentials = self.credentials.lock().unwrap();
            match credentials.get_mut(&hash) {
                Some(entry)
                    if !entry.used
                        && peer.as_ref().is_ok_and(|(parent, exe)| {
                            Some(*parent) == *entry.pid.borrow() && *exe == self.command
                        }) =>
                {
                    entry.used = true;
                    Some(entry.clone())
                }
                entry => {
                    tracing::warn!(thread=%entry.map(|entry|entry.thread.to_string()).unwrap_or_else(||"unknown\
                        ".into()),reason="peer, token, or single-connection rule","MCP connection refused");
                    None
                }
            }
        };
        let Some(credential) = credential else { return };
        tracing::info!(thread=%credential.thread,"MCP connection accepted");
        let pid = (*credential.pid.borrow()).expect("registered CLI pid");
        let (input, mut frames) = mpsc::channel(4);
        let (closed, mut ended) = oneshot::channel();
        let credentials = self.credentials.clone();
        let reader = tokio::spawn(async move {
            loop {
                let next = lines.next().await;
                let terminal = !matches!(next, Ok(Some(Line::Text(_))));
                if input.try_send(next).is_err() || terminal {
                    break;
                }
            }
            credentials.lock().unwrap().remove(&hash);
            let _ = closed.send(());
        });
        let _reader = ConnectionRead(reader.abort_handle());
        loop {
            let line = tokio::select! {result=frames.recv()=>match result {Some(line)=>line,None=>break},
            _=out.closed()=>break};
            if !self.active(&hash) {
                break;
            }
            let mut value = match line {
                Ok(Some(Line::Text(line))) => match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(_) => {
                        let written = tokio::select! {
                            biased;
                            _ = &mut ended => break,
                            _ = out.closed() => break,
                            result = write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32\
                                700,\"message\":\"Parse error\"}}\n") => result,
                        };
                        if written.is_err() {
                            break;
                        }
                        continue;
                    }
                },
                _ => break,
            };
            TokenHash(hash).sanitize(&mut value, "[redacted]");
            let reply = tokio::select! {
                biased;
                _ = &mut ended => break,
                _ = out.closed() => break,
                reply = response_for(
                    credential.thread,
                    Some(Caller {epoch: credential.epoch, pid, token: TokenHash(hash)}),
                    value,
                    &out,
                ) => reply,
            };
            if let Some(reply) = reply {
                let Ok(mut bytes) = serde_json::to_vec(&reply) else {
                    break;
                };
                bytes.push(b'\n');
                let written = tokio::select! {
                    biased;
                    _ = &mut ended => break,
                    _ = out.closed() => break,
                    result = write.write_all(&bytes) => result,
                };
                if written.is_err() {
                    break;
                }
            }
        }
    }
}
struct ConnectionRead(tokio::task::AbortHandle);
impl Drop for ConnectionRead {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn response(
    caller: ThreadId,
    value: Value,
    out: &mpsc::Sender<EngineMsg>,
) -> Option<Value> {
    response_for(caller, None, value, out).await
}
async fn response_for(
    caller: ThreadId,
    session: Option<Caller>,
    value: Value,
    out: &mpsc::Sender<EngineMsg>,
) -> Option<Value> {
    let id = value.get("id")?.clone();
    let result = match value.get("method").and_then(Value::as_str) {
        Some("initialize") => {
            json!({"protocolVersion":value["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),"capab\
                ilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"veyra","version":env!("CARGO_PKG\
                    _VERSION")},"instructions":tools::instructions()})
        }
        Some("ping") => json!({}),
        Some("tools/list") => json!({"tools":tools::list()}),
        Some("tools/call") => {
            let Some(name) = value["params"]["name"].as_str() else {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"tools/call needs a tool name"}}),
                );
            };
            let logged_name = tools::Tool::from_name(name)
                .map(tools::Tool::name)
                .unwrap_or("unknown");
            tracing::info!(thread=%caller,tool=logged_name,"MCP tools/call");
            let (reply, rx) = oneshot::channel();
            let call = ToolCall {
                veyra_label: false,
                name: name.into(),
                arguments: value["params"]
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                input_summary: None,
            };
            if out
                .send(EngineMsg::Tool {
                    caller,
                    session,
                    call,
                    reply,
                })
                .await
                .is_err()
            {
                return Some(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32603,"message":"engine is closed"}}),
                );
            }
            let result = rx.await.unwrap_or(ToolResult {
                ok: false,
                output_tail: Some(
                    "engine is closed; call list_providers in a new running turn".into(),
                ),
            });
            json!({"content":[{"type":"text","text":result.output_tail.unwrap_or_default()}],"isError":!result.ok})
        }
        _ => {
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}}),
            );
        }
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}

pub struct Configuration {
    pub path: PathBuf,
    thread: ThreadId,
}
impl Configuration {
    pub fn create(
        data: &Path,
        thread: ThreadId,
        command: &Path,
        socket: &Path,
        token: &McpToken,
    ) -> std::io::Result<Self> {
        let folder = data.join("run").join(thread.to_string());
        platform::private_dir(&folder)?;
        let path = folder.join("mcp.json");
        let config = Self { path, thread };
        let mut file = platform::private_file(
            &config.path,
            std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true),
        )?;
        serde_json::to_writer(
            &mut file,
            &json!({"mcpServers":{"veyra":{"command":command,"args":["mcp"],"env":{"VEYRA_SOCKET":socket,"VEYRA\
                _MCP_TOKEN":token.expose()}}}}),
        )?;
        Ok(config)
    }
    pub fn system_init(&self, wire: &Value) -> std::io::Result<()> {
        if wire["type"] == "system"
            && wire["subtype"] == "init"
            && let Some(entry) = wire["mcp_servers"]
                .as_array()
                .and_then(|entries| entries.iter().find(|entry| entry["name"] == "veyra"))
        {
            platform::remove_optional_file(&self.path)?;
            if entry["status"] != "connected" {
                let status = match entry["status"].as_str() {
                    Some("failed") => "failed",
                    Some("needs-auth") => "needs-auth",
                    Some("pending") => "pending",
                    _ => "other",
                };
                tracing::warn!(thread=%self.thread,status,"Veyra MCP server did not report connected");
            }
        }
        Ok(())
    }
}
impl Drop for Configuration {
    fn drop(&mut self) {
        if let Err(error) = platform::remove_optional_file(&self.path) {
            tracing::warn!(thread=%self.thread,%error,"MCP config cleanup failed");
        }
    }
}

pub(crate) struct Session {
    pub config: Configuration,
    pub hash: TokenHash,
    pub server: Arc<Server>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.server.credentials.lock().unwrap().remove(&self.hash.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn registration_failure(closed: bool) {
        use tracing::instrument::WithSubscriber;
        let server = Server {
            socket: PathBuf::new(),
            command: PathBuf::new(),
            credentials: Default::default(),
        };
        let thread = ThreadId::new();
        let token = McpToken::new();
        let (registration, pid) = watch::channel(None);
        server.credentials.lock().unwrap().insert(
            token.hash(),
            Credential {
                thread,
                epoch: 1,
                pid,
                used: false,
            },
        );
        let registration = (!closed).then_some(registration);
        let (socket, mut client) = platform::Socket::pair().unwrap();
        client
            .write_all(format!("{}\n", token.0).as_bytes())
            .await
            .unwrap();
        let (out, _messages) = mpsc::channel(1);
        let logs = crate::tests::CapturedLogs::default();
        server
            .connection(socket, out)
            .with_subscriber(logs.clone())
            .await;
        let reason = if closed {
            "closed registration"
        } else {
            "registration timeout"
        };
        let events = logs.0.lock().unwrap();
        assert!(
            events.iter().any(|event| event
                .get("message")
                .is_some_and(|text| text == "MCP connection refused")
                && event.get("thread") == Some(&thread.to_string())
                && event.get("reason") == Some(&reason.to_owned())),
            "missing refusal for {reason}: {events:?}"
        );
        drop(registration);
    }
    #[tokio::test(start_paused = true)]
    async fn pending_registration_timeout_logs_thread_and_reason() {
        registration_failure(false).await;
    }
    #[tokio::test(start_paused = true)]
    async fn closed_registration_logs_thread_and_reason() {
        registration_failure(true).await;
    }
    #[test]
    fn revoked_or_replaced_session_cannot_authorize_a_queued_call() {
        let server = Server {
            socket: PathBuf::new(),
            command: PathBuf::new(),
            credentials: Default::default(),
        };
        let thread = ThreadId::new();
        let token = McpToken::new();
        let caller = Caller {
            epoch: 1,
            pid: 10,
            token: TokenHash(token.hash()),
        };
        server.credentials.lock().unwrap().insert(
            caller.token.0,
            Credential {
                thread,
                epoch: 1,
                pid: watch::channel(Some(10)).1,
                used: true,
            },
        );
        assert!(server.allows(thread, caller));
        server.revoke(thread, 1);
        assert!(!server.allows(thread, caller));
        server.credentials.lock().unwrap().insert(
            McpToken::new().hash(),
            Credential {
                thread,
                epoch: 1,
                pid: watch::channel(Some(11)).1,
                used: true,
            },
        );
        assert!(!server.allows(thread, caller));
    }
}

#[cfg(test)]
mod launch_lock_tests {
    use super::*;
    #[test]
    fn revocation_does_not_wait_for_a_pending_launch() {
        let server = Server {
            socket: PathBuf::new(),
            command: PathBuf::new(),
            credentials: Default::default(),
        };
        let thread = ThreadId::new();
        let (entered, seen) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let launching = server.clone();
        let launch = std::thread::spawn(move || {
            launching.for_spawn(thread, 1, |_| {
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(2)).unwrap();
                Ok(((), 1))
            })
        });
        seen.recv_timeout(Duration::from_secs(2)).unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let revoke = std::thread::spawn(move || {
            server.revoke(thread, 1);
            done.send(()).unwrap();
        });
        let prompt = finished.recv_timeout(Duration::from_secs(1)).is_ok();
        release.send(()).unwrap();
        launch.join().unwrap().unwrap();
        revoke.join().unwrap();
        assert!(prompt, "revocation waited for the launch gate owner");
    }
}

#[cfg(test)]
mod eof_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn half_closed_peer_ends_pending_call() {
        let root = PathBuf::from("/tmp").join(format!("veyra-codex-replay-{}", ThreadId::new()));
        platform::private_dir(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let (out, mut messages) = mpsc::channel(16);
        let server = Server::bind(&root, std::env::current_exe().unwrap(), out).unwrap();
        let thread = ThreadId::new();
        let mut client = server
            .for_spawn(thread, 1, |token| {
                Ok((
                    (token.expose().to_owned(), server.socket.clone()),
                    platform::parent_pid(std::process::id())?,
                ))
            })
            .unwrap();
        let token = TokenHash::of(&client.0);
        let mut socket = platform::connect_async_socket(&client.1).await.unwrap();
        socket.write_all(client.0.as_bytes()).await.unwrap();
        socket.write_all(b"\n").await.unwrap();
        client.0.clear();
        drop(client);
        socket
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();
        let mut response = ByteLines::new(&mut socket);
        let Some(Line::Text(reply)) = response.next().await.unwrap() else {
            panic!("no positive-control reply")
        };
        assert_eq!(
            serde_json::from_str::<Value>(&reply).unwrap(),
            json!({"jsonrpc":"2.0","id":1,"result":{}})
        );
        drop(response);
        socket.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"list_\
            providers\",\"arguments\":{}}}\n").await.unwrap();
        let EngineMsg::Tool { reply, .. } =
            tokio::time::timeout(Duration::from_secs(2), messages.recv())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("tool did not reach actor")
        };
        socket.shutdown().await.unwrap();
        let mut bytes = vec![];
        let closed =
            tokio::time::timeout(Duration::from_millis(500), socket.read_to_end(&mut bytes)).await;
        drop(reply);
        drop(messages);
        assert!(
            closed.is_ok(),
            "a half-closed peer remained connected while an actor reply was pending"
        );
        assert_eq!(closed.unwrap().unwrap(), 0);
        assert!(!server.allows(
            thread,
            Caller {
                epoch: 1,
                pid: platform::parent_pid(std::process::id()).unwrap(),
                token
            }
        ));
    }
}

#[cfg(test)]
#[tokio::test]
async fn full_frame_queue_closes_and_revokes_pending_call() {
    use tokio::io::AsyncReadExt;
    for pipelined in [0, 33] {
        let root = PathBuf::from("/tmp").join(format!("veyra-mcp-queue-{}", ThreadId::new()));
        platform::private_dir(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let (out, mut messages) = mpsc::channel(16);
        let server = Server::bind(&root, std::env::current_exe().unwrap(), out).unwrap();
        let thread = ThreadId::new();
        let mut authentication = server
            .for_spawn(thread, 1, |token| {
                Ok((
                    token.expose().to_owned(),
                    platform::parent_pid(std::process::id())?,
                ))
            })
            .unwrap();
        let token = TokenHash::of(&authentication);
        let mut socket = platform::connect_async_socket(&server.socket)
            .await
            .unwrap();
        socket.write_all(authentication.as_bytes()).await.unwrap();
        socket.write_all(b"\n").await.unwrap();
        authentication.clear();
        drop(authentication);
        socket.write_all(b"{\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"list_providers\",\"argume\
            nts\":{}}}\n").await.unwrap();
        let held = tokio::time::timeout(Duration::from_secs(2), messages.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(&held, EngineMsg::Tool { .. }));
        for id in 2..pipelined + 2 {
            if socket
                .write_all(format!("{{\"id\":{id},\"method\":\"ping\"}}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = socket.shutdown().await;
        let mut bytes = vec![];
        let closed =
            tokio::time::timeout(Duration::from_millis(750), socket.read_to_end(&mut bytes)).await;
        drop(held);
        drop(messages);
        assert!(
            closed.is_ok(),
            "{pipelined} pipelined frames hid EOF behind a held actor call"
        );
        assert!(!server.allows(
            thread,
            Caller {
                epoch: 1,
                pid: platform::parent_pid(std::process::id()).unwrap(),
                token
            }
        ));
        assert_eq!(bytes, b"");
        drop(socket);
        drop(server);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod attempt_tests {
    use super::*;
    use crate::provider::{SessionSpec, codex};

    fn hash(frame: crate::provider::Outbound) -> (TokenHash, Value) {
        let bytes = frame.into_bytes().unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let hash = TokenHash::of(
            value["params"]["config"]["mcp_servers.veyra.env"]["VEYRA_MCP_TOKEN"]
                .as_str()
                .unwrap(),
        );
        (hash, value)
    }

    #[tokio::test(start_paused = true)]
    async fn codex_attempt_replaces_and_redacts_previous_credential() {
        for message in [
            "no rollout found for thread id old-thread",
            "already has an active writer",
        ] {
            let thread = ThreadId::new();
            let server = Arc::new(Server {
                socket: "/owned/veyra.sock".into(),
                command: "/owned/veyra".into(),
                credentials: Default::default(),
            });
            let config = Arc::new(codex::McpConfig::new(
                server.clone(),
                McpWire::Codex,
                thread,
                1,
                std::process::id(),
            ));
            let mut codec = codex::Codec::new(
                SessionSpec {
                    thread,
                    cwd: "/owned".into(),
                    config: ThreadConfig {
                        mode: PermissionMode::AskFirst,
                        model: None,
                        effort: None,
                    },
                    native: Some(NativeSessionId("old-thread".into())),

                    root: true,
                },
                std::process::id(),
                Some(config.clone()),
            );
            codec.initialize();
            let mut opened = codec
                .value(json!({"id":1,"result":{"userAgent":"veyra-test"}}))
                .unwrap();
            let (first, mut first_wire) = hash(opened.controls.pop().unwrap());
            assert!(server.active(&first.0));
            let mut refused = codec
                .value(json!({"id":2,"error":{"code":-32600,"message":message}}))
                .unwrap();
            assert!(
                !server.active(&first.0),
                "refused attempt credential remains registered"
            );
            let next = if let Some(frame) = refused.controls.pop() {
                frame
            } else {
                tokio::time::advance(Duration::from_millis(250)).await;
                codec.timer_lines().unwrap().pop().unwrap()
            };
            let (second, mut second_wire) = hash(next);
            assert!(first != second, "thread attempts reused a credential");
            assert!(server.active(&second.0));
            config.sanitize(&mut first_wire, "<id:token>");
            config.sanitize(&mut second_wire, "<id:token>");
            for wire in [first_wire, second_wire] {
                assert!(
                    wire["params"]["config"]["mcp_servers.veyra.env"]["VEYRA_MCP_TOKEN"]
                        == "<id:token>"
                );
            }
            drop(codec);
            drop(config);
            assert!(!server.active(&second.0));
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum McpWire {
    Codex,
    Grok,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CodexOption {
    StartupGrace,
    ToolTimeout,
}
impl CodexOption {
    const ALL: [Self; 2] = [Self::StartupGrace, Self::ToolTimeout];
    fn key(self) -> &'static str {
        match self {
            Self::StartupGrace => "mcp_optional_startup_grace_ms",
            Self::ToolTimeout => "mcp_servers.veyra.tool_timeout_sec",
        }
    }
    fn value(self) -> Value {
        match self {
            Self::StartupGrace => json!(5000),
            Self::ToolTimeout => json!(30),
        }
    }
    pub(crate) fn rejected_by(message: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|option| message.contains(option.key()))
    }
}
pub(crate) struct McpConfig {
    server: std::sync::Arc<crate::mcp::Server>,
    wire: McpWire,
    thread: ThreadId,
    epoch: u32,
    pid: u32,
    hashes: std::sync::Mutex<Vec<crate::mcp::TokenHash>>,
    unsupported_codex_options: Mutex<std::collections::BTreeSet<CodexOption>>,
}
impl McpConfig {
    pub(crate) fn new(
        server: std::sync::Arc<crate::mcp::Server>,
        wire: McpWire,
        thread: ThreadId,
        epoch: u32,
        pid: u32,
    ) -> Self {
        Self {
            wire,
            server,
            thread,
            epoch,
            pid,
            hashes: Default::default(),
            unsupported_codex_options: Default::default(),
        }
    }
    pub(crate) fn frame(&self, line: Value) -> Result<Outbound, ProviderError> {
        let token = self.server.for_spawn(self.thread, self.epoch, |token| {
            Ok((token.clone(), self.pid))
        })?;
        self.hashes
            .lock()
            .unwrap()
            .push(crate::mcp::TokenHash(token.hash()));
        Ok(Outbound::Provider(Box::new(ThreadFrame {
            line,
            wire: self.wire,
            command: self.server.command.clone(),
            socket: self.server.socket.clone(),
            token,
            unsupported_codex_options: self.unsupported_codex_options.lock().unwrap().clone(),
        })))
    }
    pub(crate) fn reject_codex_option(&self, option: CodexOption) -> bool {
        if !self
            .unsupported_codex_options
            .lock()
            .unwrap()
            .insert(option)
        {
            return false;
        }
        self.revoke();
        tracing::warn!(
            option = option.key(),
            "Codex rejected optional MCP config; retrying without it"
        );
        true
    }
    pub(crate) fn sanitize(&self, value: &mut Value, replacement: &str) {
        for hash in self.hashes.lock().unwrap().iter() {
            hash.sanitize(value, replacement);
        }
    }
    pub(crate) fn redact(&self, text: &str, replacement: &str) -> String {
        self.hashes
            .lock()
            .unwrap()
            .iter()
            .fold(text.to_owned(), |text, hash| {
                hash.redact(&text, replacement)
            })
    }
    pub(crate) fn revoke(&self) {
        self.server.revoke(self.thread, self.epoch);
    }
}
impl Drop for McpConfig {
    fn drop(&mut self) {
        self.revoke();
    }
}
pub(crate) struct ThreadFrame {
    wire: McpWire,
    line: Value,
    command: std::path::PathBuf,
    socket: std::path::PathBuf,
    token: crate::mcp::McpToken,
    unsupported_codex_options: std::collections::BTreeSet<CodexOption>,
}
impl ThreadFrame {
    fn config(&self, token: &str) -> Value {
        let mut config = json!({"mcp_servers.veyra.command":self.command,
            "mcp_servers.veyra.args":["mcp"],
            "mcp_servers.veyra.env":{"VEYRA_SOCKET":self.socket,"VEYRA_MCP_TOKEN":token}});
        for option in CodexOption::ALL {
            if !self.unsupported_codex_options.contains(&option) {
                config[option.key()] = option.value();
            }
        }
        config
    }
    fn inject(&self, line: &mut Value, token: &str) {
        match self.wire {
            McpWire::Grok => {
                line["params"]["mcpServers"] = json!([{
                    "name":"veyra", "command":self.command, "args":["mcp"],
                    "env":[{"name":"VEYRA_SOCKET","value":self.socket},
                        {"name":"VEYRA_MCP_TOKEN","value":token}]
                }])
            }
            McpWire::Codex => line["params"]["config"] = self.config(token),
        }
    }
    #[cfg(any(debug_assertions, test))]
    pub(crate) fn value(&self) -> Value {
        let mut line = self.line.clone();
        self.inject(&mut line, "<id:token>");
        line
    }
    pub(crate) fn into_bytes(mut self) -> serde_json::Result<Vec<u8>> {
        let mut line = std::mem::take(&mut self.line);
        self.inject(&mut line, self.token.expose());
        serde_json::to_vec(&line)
    }
}

#[cfg(test)]
mod credential_frame_tests {
    use super::*;
    #[test]
    fn grok_root_credential_stays_typed_until_serialization() {
        let token = McpToken::new();
        let outbound = Outbound::Provider(Box::new(ThreadFrame {
            wire: McpWire::Grok,
            line: json!({"id":1,"method":"session/new","params":{"mcpServers":[],"config":{"user":"kept"}}}),
            command: "/owned/veyra".into(),
            socket: "/owned/veyra.sock".into(),
            token: token.clone(),
            unsupported_codex_options: Default::default(),
        }));
        let recorded = outbound.value();
        assert!(
            recorded["params"]["mcpServers"]
                == json!([{"name":"veyra","command":"/owned/veyra","args":["mcp"],"env":[{"name":"VEYRA_SOCKET\
                    ","value":"/owned/veyra.sock"},{"name":"VEYRA_MCP_TOKEN","value":"<id:token>"}]}])
        );
        assert!(recorded["params"]["config"] == json!({"user":"kept"}));
        assert!(!recorded.to_string().contains(token.expose()));
        let bytes = outbound.into_bytes().unwrap();
        let written: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(
            written["params"]["mcpServers"][0]["env"][1]["value"].as_str() == Some(token.expose())
        );
    }
    #[test]
    fn codex_thread_credential_stays_typed_until_serialization() {
        let token = crate::mcp::McpToken::new();
        let outbound = Outbound::Provider(Box::new(ThreadFrame {
            wire: McpWire::Codex,
            line: json!({"id":1,"method":"thread/start","params":{}}),
            command: "/owned/veyra".into(),
            socket: "/owned/veyra.sock".into(),
            token: token.clone(),
            unsupported_codex_options: Default::default(),
        }));
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
}

#[test]
fn codex_root_config_sets_grace_and_tool_timeout() {
    for method in ["thread/start", "thread/resume"] {
        let frame = ThreadFrame {
            wire: McpWire::Codex,
            line: json!({"method":method,"params":{}}),
            command: "/owned/veyra".into(),
            socket: "/owned/socket".into(),
            token: McpToken::new(),
            unsupported_codex_options: Default::default(),
        };
        let wire = frame.value();
        assert_eq!(
            wire["params"]["config"]["mcp_optional_startup_grace_ms"],
            5000
        );
        assert_eq!(
            wire["params"]["config"]["mcp_servers.veyra.tool_timeout_sec"],
            30
        );
        assert_eq!(
            wire["params"]["config"]["mcp_servers.veyra.args"],
            json!(["mcp"])
        );
    }
}

#[tokio::test]
async fn codex_rejected_optional_keys_retry_independently() {
    let root =
        std::path::Path::new("/tmp").join(format!("veyra-provider-options-{}", ThreadId::new()));
    platform::private_dir(&root).unwrap();
    let _scratch = crate::tests::Scratch(root.clone());
    let (out, _rx) = mpsc::channel(256);
    let server = Arc::new(Server::bind(&root, std::env::current_exe().unwrap(), out).unwrap());
    let config = Arc::new(McpConfig::new(
        server,
        McpWire::Codex,
        ThreadId::new(),
        1,
        std::process::id(),
    ));
    for resume in [false, true] {
        let mut codec = crate::provider::codex::Codec::new(
            crate::provider::SessionSpec {
                thread: ThreadId::new(),
                cwd: root.clone(),
                config: crate::domain::ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: resume.then(|| NativeSessionId("retained".into())),
                root: true,
            },
            std::process::id(),
            Some(config.clone()),
        );
        codec.initialize();
        let opened = codec.value(json!({"id":1,"result":{}})).unwrap();
        let id = opened.controls[0].value()["id"].clone();
        let rejected = codec
            .value(
                json!({"id":id,"error":{"code":-32602,"message":"unknown config mcp_optional\
            _startup_grace_ms"}}),
            )
            .unwrap();
        let first = rejected.controls[0].value();
        assert!(
            first["params"]["config"]
                .get("mcp_optional_startup_grace_ms")
                .is_none()
        );
        assert_eq!(
            first["params"]["config"]["mcp_servers.veyra.tool_timeout_sec"],
            30
        );
        let rejected = codec
            .value(
                json!({"id":first["id"],"error":{"code":-32602,"message":"unknown config mcp\
            _servers.veyra.tool_timeout_sec"}}),
            )
            .unwrap();
        let second = rejected.controls[0].value();
        assert!(
            second["params"]["config"]
                .get("mcp_optional_startup_grace_ms")
                .is_none()
        );
        assert!(
            second["params"]["config"]
                .get("mcp_servers.veyra.tool_timeout_sec")
                .is_none()
        );
        let ready = codec
            .value(json!({"id":second["id"],"result":{"thread":{"id":"usable"}}}))
            .unwrap();
        assert!(
            matches!(ready.events.as_slice(), [crate::provider::ProviderEvent::Session(
                crate::provider::SessionEvent::Ready {native,..})] if native.0=="usable")
        );
        config.unsupported_codex_options.lock().unwrap().clear();
    }
}
