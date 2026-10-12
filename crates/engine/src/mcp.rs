use crate::{
    actor::EngineMsg,
    domain::*,
    platform,
    provider::session::{ByteLines, Line},
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
        platform::remove_stale_mcp_configs(data)?;
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
            if !matches!(registered, Ok(Ok(_))) {
                return;
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
                    tracing::warn!(thread=%entry.map(|entry|entry.thread.to_string()).unwrap_or_else(||"unknown".into()),reason="peer, token, or single-connection rule","MCP connection refused");
                    None
                }
            }
        };
        let Some(credential) = credential else { return };
        tracing::info!(thread=%credential.thread,"MCP connection accepted");
        let pid = (*credential.pid.borrow()).expect("registered CLI pid");
        loop {
            let line = tokio::select! {result=lines.next()=>result,_=out.closed()=>break};
            if !self.active(&hash) {
                break;
            }
            let mut value = match line {
                Ok(Some(Line::Text(line))) => match serde_json::from_str(&line) {
                    Ok(value) => value,
                    Err(_) => {
                        let _=write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"Parse error\"}}\n").await;
                        continue;
                    }
                },
                _ => break,
            };
            TokenHash(hash).sanitize(&mut value, "[redacted]");
            if let Some(reply) = response_for(
                credential.thread,
                Some(Caller {
                    epoch: credential.epoch,
                    pid,
                    token: TokenHash(hash),
                }),
                value,
                &out,
            )
            .await
            {
                let Ok(mut bytes) = serde_json::to_vec(&reply) else {
                    break;
                };
                bytes.push(b'\n');
                if write.write_all(&bytes).await.is_err() {
                    break;
                }
            }
        }
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
            json!({"protocolVersion":value["params"]["protocolVersion"].as_str().unwrap_or("2024-11-05"),"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"veyra","version":env!("CARGO_PKG_VERSION")},"instructions":tools::instructions()})
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
            &json!({"mcpServers":{"veyra":{"command":command,"args":["mcp"],"env":{"VEYRA_SOCKET":socket,"VEYRA_MCP_TOKEN":token.expose()}}}}),
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
