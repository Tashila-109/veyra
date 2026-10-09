#![cfg(debug_assertions)]

use anyhow::{Context as _, bail};
use gpui_kit::{App, Entity, Focusable, KeyDownEvent, Keystroke, PlatformInput, Window};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};
use tokio::sync::mpsc as channel;
use veyra_engine::platform;

const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_LINE: u64 = 65536;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Action { name: String, data: Option<Value> },
    Keys { keystrokes: String },
    Text { text: String },
    Dump,
    Quit,
}

pub struct Request {
    command: Command,
    schema_version: u32,
    reply: mpsc::SyncSender<Value>,
}

fn read_line(reader: impl Read) -> anyhow::Result<String> {
    let mut line = String::new();
    BufReader::new(reader.take(MAX_LINE + 1)).read_line(&mut line)?;
    if line.len() as u64 > MAX_LINE || !line.ends_with('\n') {
        bail!("control command must be one JSON line of at most {MAX_LINE} bytes");
    }
    Ok(line)
}

pub fn listen(data_dir: &Path, schema_version: u32) -> anyhow::Result<channel::Receiver<Request>> {
    let socket = data_dir.join("control.sock");
    let listener = platform::bind_socket(&socket).context("bind debug control socket")?;
    let (sender, receiver) = channel::channel(16);
    thread::Builder::new()
        .name("veyra-control".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let response = (|| {
                    stream.set_read_timeout(Some(TIMEOUT))?;
                    stream.set_write_timeout(Some(TIMEOUT))?;
                    serve(&mut stream, &sender, schema_version)
                })()
                .unwrap_or_else(|error| json!({"error": error.to_string()}));
                if let Ok(line) = serde_json::to_string(&response) {
                    let _ = writeln!(stream, "{line}");
                }
            }
        })?;
    Ok(receiver)
}

fn serve(
    stream: &mut (impl Read + Write),
    sender: &channel::Sender<Request>,
    schema_version: u32,
) -> anyhow::Result<Value> {
    let command = serde_json::from_str(&read_line(&mut *stream)?)?;
    let (reply, response) = mpsc::sync_channel(1);
    sender
        .try_send(Request {
            command,
            reply,
            schema_version,
        })
        .context("debug UI queue unavailable")?;
    response
        .recv_timeout(TIMEOUT)
        .context("debug UI did not answer within 2 seconds")
}

pub fn attach(
    shell: &Entity<crate::ui::Shell>,
    mut requests: channel::Receiver<Request>,
    data_dir: PathBuf,
    window: &mut Window,
    cx: &mut App,
) {
    shell.update(cx, |_, cx| {
        cx.spawn_in(window, async move |shell, cx| {
            while let Some(request) = requests.recv().await {
                let response = cx.update(|window, cx| -> anyhow::Result<Value> {
                    match request.command {
                        Command::Dump => {
                            let mut dump =
                                shell.update(cx, |shell, cx| shell.dump(&data_dir, cx))?;
                            dump["schema_version"] = json!(request.schema_version);
                            Ok(dump)
                        }
                        Command::Action { name, data } => {
                            let action = match cx.build_action(&name, data) {
                                Ok(action) => action,
                                Err(error) => {
                                    shell.update(cx, |shell, cx| {
                                        shell.notice(error.to_string(), cx)
                                    })?;
                                    return Err(error.into());
                                }
                            };
                            let root = shell.update(cx, |shell, cx| shell.focus_handle(cx))?;
                            root.dispatch_action(action.as_ref(), window, cx);
                            Ok(json!({"ok": true}))
                        }
                        Command::Keys { keystrokes } => {
                            let keys = keystrokes
                                .split_whitespace()
                                .map(Keystroke::parse)
                                .collect::<Result<Vec<_>, _>>()?;
                            if keys.is_empty() {
                                bail!("keys needs at least one keystroke");
                            }
                            for keystroke in keys {
                                window.dispatch_event(
                                    PlatformInput::KeyDown(KeyDownEvent {
                                        keystroke,
                                        is_held: false,
                                        prefer_character_input: false,
                                    }),
                                    cx,
                                );
                            }
                            Ok(json!({"ok": true}))
                        }
                        Command::Text { text } => {
                            shell
                                .update(cx, |shell, cx| shell.insert_text(&text, window, cx))??;
                            Ok(json!({"ok":true}))
                        }
                        Command::Quit => {
                            window.dispatch_action(Box::new(crate::ui::actions::Quit), cx);
                            Ok(json!({"ok": true}))
                        }
                    }
                });
                let value = match response {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => json!({"error": error.to_string()}),
                    Err(error) => json!({"error": error.to_string()}),
                };
                let _ = request.reply.send(value);
            }
        })
        .detach();
    });
}

enum ClientCommand {
    Request(Command),
    Wait {
        thread: String,
        status: String,
        seconds: u64,
    },
}
impl From<Command> for ClientCommand {
    fn from(command: Command) -> Self {
        Self::Request(command)
    }
}
fn parse_args(args: &[String]) -> anyhow::Result<ClientCommand> {
    match args {
        [command] if command == "dump" => Ok(Command::Dump.into()),
        [command] if command == "quit" => Ok(Command::Quit.into()),
        [command, text] if command == "text" => Ok(Command::Text { text: text.clone() }.into()),
        [command, keystrokes] if command == "keys" => Ok(Command::Keys {
            keystrokes: keystrokes.clone(),
        }
        .into()),
        [command, name] if command == "action" => Ok(Command::Action {
            name: name.clone(),
            data: None,
        }
        .into()),
        [command, name, data] if command == "action" => Ok(Command::Action {
            name: name.clone(),
            data: Some(serde_json::from_str(data)?),
        }
        .into()),
        [command, thread, status] | [command, thread, status, _] if command == "wait" => {
            if thread.is_empty()
                || ![
                    "working",
                    "done",
                    "failed",
                    "idle",
                    "held",
                    "interrupted",
                    "needsyou",
                    "interminal",
                ]
                .contains(&status.to_lowercase().as_str())
            {
                bail!("usage: veyra ctl wait THREAD STATUS [SECONDS]");
            }
            let seconds = if args.len() == 4 {
                args[3].parse()?
            } else {
                30
            };
            Ok(ClientCommand::Wait {
                thread: thread.clone(),
                status: status.clone(),
                seconds,
            })
        }
        _ => bail!(concat!(
            "usage: veyra ctl action NAME [JSON] | keys KEYSTROKES | text STRING | ",
            "dump | wait THREAD STATUS [SECONDS] | quit"
        )),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ClientExit {
    Success = 0,
    Timeout = 1,
    Usage = 2,
    MissingThread = 3,
}
fn wait_exit_code(dump: &Value, thread: &str, status: &str) -> Option<ClientExit> {
    let row = dump["sidebar"]["rows"].as_array().and_then(|rows| {
        rows.iter()
            .find(|row| row["kind"] == "thread" && row["id"].as_str() == Some(thread))
    });
    match row {
        None => Some(ClientExit::MissingThread),
        Some(row) if row["archived"] == true => Some(ClientExit::MissingThread),
        Some(row)
            if row["status"]
                .as_str()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(status)) =>
        {
            Some(ClientExit::Success)
        }
        Some(_) => None,
    }
}
fn request(data_dir: &Path, command: &Command) -> anyhow::Result<Value> {
    request_inner(data_dir, command)
        .map_err(|error| anyhow::anyhow!("veyra ctl driver error: {error:#}"))
}
fn request_inner(data_dir: &Path, command: &Command) -> anyhow::Result<Value> {
    let mut stream = platform::connect_socket(&data_dir.join("control.sock"))
        .context("connect to Veyra debug driver")?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    writeln!(stream, "{}", serde_json::to_string(command)?)?;
    let reply: Value = serde_json::from_str(&read_line(stream)?)?;
    if let Some(error) = reply.get("error").and_then(Value::as_str) {
        bail!("{error}");
    }
    Ok(reply)
}
pub fn client() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(ClientExit::Usage as i32);
        }
    };
    let data_dir = std::env::var_os("VEYRA_DATA_DIR")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(platform::default_data_dir)?;
    match command {
        ClientCommand::Request(command) => {
            let reply = request(&data_dir, &command)?;
            println!("{}", serde_json::to_string_pretty(&reply)?);
        }
        ClientCommand::Wait {
            thread: id,
            status,
            seconds,
        } => {
            let start = std::time::Instant::now();
            loop {
                let dump = request(&data_dir, &Command::Dump)?;
                let code = wait_exit_code(&dump, &id, &status).or_else(|| {
                    (start.elapsed() >= Duration::from_secs(seconds)).then_some(ClientExit::Timeout)
                });
                if let Some(code) = code {
                    if code == ClientExit::Timeout {
                        eprintln!("veyra ctl wait timed out: thread={id} status={status}");
                    }
                    println!("{}", serde_json::to_string_pretty(&dump)?);
                    std::process::exit(code as i32);
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ctl_wait_exit_codes() {
        let dump = json!({"sidebar":{"rows":[
            {"kind":"thread","id":"one","status":"Done","archived":false},
            {"kind":"thread","id":"old","status":"Done","archived":true},
            {"kind":"thread","id":"running","status":"Working","archived":false}
        ]}});
        assert_eq!(
            wait_exit_code(&dump, "one", "done"),
            Some(ClientExit::Success)
        );
        assert_eq!(
            wait_exit_code(&dump, "missing", "Done"),
            Some(ClientExit::MissingThread)
        );
        assert_eq!(
            wait_exit_code(&dump, "old", "Done"),
            Some(ClientExit::MissingThread)
        );
        assert_eq!(wait_exit_code(&dump, "running", "Done"), None);
        assert!(matches!(
            parse_args(&["wait".into(), "one".into(), "Done".into()]).unwrap(),
            ClientCommand::Wait { seconds: 30, .. }
        ));
        assert!(parse_args(&["wait".into(), "one".into(), "Bogus".into()]).is_err());
        assert!(parse_args(&["wait".into(), "one".into(), "Done".into(), "-1".into()]).is_err());
    }
    #[test]
    fn driver_parses_five_commands() {
        assert!(matches!(
            parse_args(&["dump".into()]).unwrap(),
            ClientCommand::Request(Command::Dump)
        ));
        assert!(matches!(
            parse_args(&["quit".into()]).unwrap(),
            ClientCommand::Request(Command::Quit)
        ));
        assert!(matches!(
            parse_args(&["text".into(), "hello".into()]).unwrap(),
            ClientCommand::Request(Command::Text { text }) if text == "hello"
        ));
        assert!(matches!(
            parse_args(&["keys".into(), "secondary-n".into()]).unwrap(),
            ClientCommand::Request(Command::Keys { keystrokes }) if keystrokes == "secondary-n"
        ));
        let action = [
            "action".into(),
            "veyra::SetSidebarWidth".into(),
            "{\"width\":300}".into(),
        ];
        assert!(matches!(
            parse_args(&action).unwrap(),
            ClientCommand::Request(Command::Action { data: Some(data), .. }) if data == json!({"width":300})
        ));
        assert!(parse_args(&["dump".into(), "extra".into()]).is_err());
        assert!(parse_args(&["action".into(), "name".into(), "bad json".into()]).is_err());
    }
    #[test]
    fn driver_rejects_oversized_or_unterminated_lines() {
        assert!(read_line("{}\n".as_bytes()).is_ok());
        assert!(read_line("{}".as_bytes()).is_err());
        assert!(read_line(format!("{}\n", "x".repeat(MAX_LINE as usize)).as_bytes()).is_err());
    }
}
