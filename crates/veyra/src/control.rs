#![cfg(debug_assertions)]

use anyhow::{Context as _, bail};
use gpui_kit::{App, Entity, KeyDownEvent, Keystroke, PlatformInput, Window};
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
                            let action = cx.build_action(&name, data)?;
                            window.dispatch_action(action, cx);
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
                        Command::Text { .. } => {
                            bail!("the M0 composer is a shell with no text input")
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

fn parse_args(args: &[String]) -> anyhow::Result<Command> {
    match args {
        [command] if command == "dump" => Ok(Command::Dump),
        [command] if command == "quit" => Ok(Command::Quit),
        [command, text] if command == "text" => Ok(Command::Text { text: text.clone() }),
        [command, keystrokes] if command == "keys" => Ok(Command::Keys {
            keystrokes: keystrokes.clone(),
        }),
        [command, name] if command == "action" => Ok(Command::Action {
            name: name.clone(),
            data: None,
        }),
        [command, name, data] if command == "action" => Ok(Command::Action {
            name: name.clone(),
            data: Some(serde_json::from_str(data)?),
        }),
        _ => bail!(
            "usage: veyra ctl action NAME [JSON] | keys KEYSTROKES | text STRING | dump | quit"
        ),
    }
}

pub fn client() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    let command = match parse_args(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let data_dir = match std::env::var_os("VEYRA_DATA_DIR") {
        Some(path) => PathBuf::from(path),
        None => platform::default_data_dir()?,
    };
    let mut stream = platform::connect_socket(&data_dir.join("control.sock"))
        .context("connect to Veyra debug driver")?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    writeln!(stream, "{}", serde_json::to_string(&command)?)?;
    let reply: Value = serde_json::from_str(&read_line(stream)?)?;
    println!("{}", serde_json::to_string_pretty(&reply)?);
    if let Some(error) = reply.get("error").and_then(Value::as_str) {
        bail!("{error}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn driver_parses_five_commands() {
        assert!(matches!(
            parse_args(&["dump".into()]).unwrap(),
            Command::Dump
        ));
        assert!(matches!(
            parse_args(&["quit".into()]).unwrap(),
            Command::Quit
        ));
        assert!(
            matches!(parse_args(&["text".into(), "hello".into()]).unwrap(), Command::Text { text } if text == "hello")
        );
        assert!(
            matches!(parse_args(&["keys".into(), "secondary-n".into()]).unwrap(), Command::Keys { keystrokes } if keystrokes == "secondary-n")
        );
        assert!(
            matches!(parse_args(&["action".into(), "veyra::SetSidebarWidth".into(), "{\"width\":300}".into()]).unwrap(), Command::Action { data: Some(data), .. } if data == json!({"width":300}))
        );
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
