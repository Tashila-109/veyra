use super::{
    Outbound, ProviderError,
    session::{ByteLines, Line},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub code: i64,
    pub message: String,
}
impl Error {
    fn closed(message: &str) -> Self {
        Self {
            code: -32000,
            message: message.into(),
        }
    }
}
struct Pending<T> {
    tag: T,
    reply: oneshot::Sender<Result<Value, Error>>,
}
pub struct Peer<T> {
    version_field: bool,
    next_id: u64,
    pending: BTreeMap<u64, Pending<T>>,
}
pub enum Frame<T> {
    Reply {
        tag: T,
        result: Result<Value, Error>,
    },
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Unknown,
}
impl<T> Peer<T> {
    pub fn new(version_field: bool) -> Self {
        Self {
            version_field,
            next_id: 0,
            pending: BTreeMap::new(),
        }
    }
    fn envelope(&self, mut value: Value) -> Value {
        if self.version_field {
            value["jsonrpc"] = json!("2.0");
        }
        value
    }
    pub fn request(
        &mut self,
        tag: T,
        method: &str,
        params: Value,
    ) -> (Value, oneshot::Receiver<Result<Value, Error>>) {
        self.next_id += 1;
        let id = self.next_id;
        let (reply, receiver) = oneshot::channel();
        self.pending.insert(id, Pending { tag, reply });
        (
            self.envelope(json!({"id":id,"method":method,"params":params})),
            receiver,
        )
    }
    pub fn send(&mut self, tag: T, method: &str, params: Value) -> Value {
        self.request(tag, method, params).0
    }
    pub fn notification(&self, method: &str, params: Value) -> Value {
        self.envelope(if params.is_null() {
            json!({"method":method})
        } else {
            json!({"method":method,"params":params})
        })
    }
    pub fn response(&self, id: Value, result: Value) -> Value {
        self.envelope(json!({"id":id,"result":result}))
    }
    pub fn error(&self, id: Value, code: i64, message: &str) -> Value {
        self.envelope(json!({"id":id,"error":{"code":code,"message":message}}))
    }
    pub fn receive(&mut self, wire: Value) -> Frame<T> {
        if let Some(method) = wire["method"].as_str() {
            return match wire.get("id").filter(|id| id.is_string() || id.is_number()) {
                Some(id) => Frame::Request {
                    id: id.clone(),
                    method: method.into(),
                    params: wire["params"].clone(),
                },
                None => Frame::Notification {
                    method: method.into(),
                    params: wire["params"].clone(),
                },
            };
        }
        if let Some(id) = wire["id"].as_u64()
            && let Some(pending) = self.pending.remove(&id)
        {
            let result = if let Some(error) = wire.get("error") {
                Err(Error {
                    code: error["code"].as_i64().unwrap_or(-32000),
                    message: error["message"]
                        .as_str()
                        .unwrap_or("Invalid provider error")
                        .into(),
                })
            } else if let Some(value) = wire.get("result") {
                Ok(value.clone())
            } else {
                Err(Error::closed("Provider reply has no result or error"))
            };
            let _ = pending.reply.send(result.clone());
            return Frame::Reply {
                tag: pending.tag,
                result,
            };
        }
        Frame::Unknown
    }
    pub fn eof(&mut self, message: &str) {
        for (_, pending) in std::mem::take(&mut self.pending) {
            let _ = pending.reply.send(Err(Error::closed(message)));
        }
    }
}
impl<T> Drop for Peer<T> {
    fn drop(&mut self) {
        self.eof("JSON-RPC connection closed");
    }
}
pub(crate) struct Reader {
    receiver: mpsc::Receiver<Result<Option<Line>, std::io::Error>>,
    task: tokio::task::JoinHandle<()>,
}
impl Reader {
    pub(crate) fn start(stdout: impl AsyncRead + Unpin + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::channel(32);
        let task = tokio::spawn(async move {
            let mut lines = ByteLines::new(stdout);
            loop {
                let next = lines.next().await;
                let ended = !matches!(next, Ok(Some(_)));
                if sender.send(next).await.is_err() || ended {
                    break;
                }
            }
        });
        Self { receiver, task }
    }
    pub(crate) async fn next(&mut self) -> std::io::Result<Option<Line>> {
        match self.receiver.recv().await {
            Some(Ok(Some(Line::Oversize(_)))) => Err(std::io::Error::other(
                "JSON-RPC line exceeded the 4 MiB cap",
            )),
            Some(value) => value,
            None => Ok(None),
        }
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(crate) async fn write(
    mut stdin: impl AsyncWrite + Unpin,
    mut input: mpsc::UnboundedReceiver<Outbound>,
    active: Arc<Mutex<Option<crate::domain::TurnId>>>,
    closing: Arc<crate::LaunchGate>,
    #[cfg(test)] close_before_prompt: Arc<std::sync::atomic::AtomicBool>,
) -> std::io::Result<()> {
    while let Some(outbound) = input.recv().await {
        let mut bytes = serde_json::to_vec(&outbound.value())?;
        bytes.push(b'\n');
        #[cfg(test)]
        if matches!(outbound, Outbound::Prompt(_))
            && close_before_prompt.swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            closing.close();
        }
        if matches!(outbound, Outbound::Prompt(_)) && closing.is_closing() {
            continue;
        }
        match outbound {
            Outbound::Interrupt(turn) | Outbound::RpcInterrupt { turn, .. } => {
                super::session::write_interrupt(&mut stdin, &bytes, turn, &active).await?
            }
            Outbound::Prompt(_) | Outbound::Message(_) => stdin.write_all(&bytes).await?,
        }
        stdin.flush().await?;
    }
    Ok(())
}
pub(crate) fn failure(error: Error) -> ProviderError {
    ProviderError::Protocol(error.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn jsonrpc_eof_fails_pending() {
        let mut peer = Peer::new(false);
        let (_, a) = peer.request((), "one", json!({}));
        let (_, b) = peer.request((), "two", json!({}));
        peer.eof("JSON-RPC stdout closed");
        for reply in [a, b] {
            assert_eq!(
                reply.await.unwrap().unwrap_err().message,
                "JSON-RPC stdout closed"
            );
        }
    }
    #[test]
    fn jsonrpc_flag_omits_field() {
        let mut codex = Peer::new(false);
        let mut acp = Peer::new(true);
        assert_eq!(
            codex.send((), "initialize", json!({})),
            json!({"id":1,"method":"initialize","params":{}})
        );
        assert_eq!(
            acp.send((), "initialize", json!({})),
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
        );
    }
    #[tokio::test]
    async fn jsonrpc_oversize_line_fails_session() {
        let bytes = vec![b'x'; 4 * 1024 * 1024 + 1];
        let mut reader = Reader::start(std::io::Cursor::new(bytes));
        assert_eq!(
            reader.next().await.err().unwrap().to_string(),
            "JSON-RPC line exceeded the 4 MiB cap"
        );
    }
    #[tokio::test]
    async fn jsonrpc_replies_route_by_id() {
        let mut peer = Peer::new(false);
        let (_, a) = peer.request("first", "one", json!({}));
        let (_, b) = peer.request("second", "two", json!({}));
        assert!(matches!(
            peer.receive(json!({"id":2,"result":{"ok":true}})),
            Frame::Reply { tag: "second", .. }
        ));
        assert!(matches!(
            peer.receive(json!({"id":1,"error":{"code":-32601,"message":"unknown"}})),
            Frame::Reply { tag: "first", .. }
        ));
        assert_eq!(b.await.unwrap().unwrap(), json!({"ok":true}));
        assert_eq!(a.await.unwrap().unwrap_err().code, -32601);
    }
}
