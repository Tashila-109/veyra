pub mod cli;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, io, path::Path};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Record {
    TranscriptStart {
        provider: String,
        protocol: String,
        cli_version: String,
        scenario: String,
        source: String,
    },
    ExpectOutbound {
        line: Value,
    },
    EmitInbound {
        line: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after_ms: Option<u64>,
    },
    EmitRaw {
        base64: String,
        #[serde(default = "once")]
        repeat: usize,
    },
    McpStart,
    McpStop,
    RecordRelayPid {
        path: String,
    },
    McpSend {
        line: Value,
    },
    McpExpect {
        line: Value,
    },
    RecordArgv {
        path: String,
    },
    Touch {
        path: String,
    },
    Spin {
        ms: u64,
    },
    Exit {
        code: i32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        stderr: Vec<String>,
    },
}

pub fn run() -> io::Result<i32> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let tui = args.first().is_some_and(|arg| arg == "resume")
        || args.iter().any(|arg| arg == "--resume")
            && !args
                .iter()
                .any(|arg| arg == "-p" || arg == "app-server" || arg == "agent");
    if tui { run_tui(&args) } else { cli::run() }
}

fn run_tui(args: &[String]) -> io::Result<i32> {
    use std::io::{Read, Write};
    let claude = args.iter().any(|arg| arg == "--permission-mode")
        && !args.iter().any(|arg| arg == "--no-auto-update");
    let history = if claude {
        let root = std::env::var_os("CLAUDE_CONFIG_DIR")
            .ok_or_else(|| io::Error::other("fake Claude TUI requires CLAUDE_CONFIG_DIR"))?;
        let native = args
            .iter()
            .position(|arg| arg == "--resume")
            .and_then(|index| args.get(index + 1))
            .ok_or_else(|| io::Error::other("fake Claude TUI requires --resume"))?;
        let id = uuid::Uuid::parse_str(native).map_err(io::Error::other)?;
        let folder = std::path::PathBuf::from(root).join("projects/veyra-fake");
        std::fs::create_dir_all(&folder)?;
        Some((folder.join(format!("{id}.jsonl")), native))
    } else {
        None
    };
    crate::platform::raw_terminal_input()?;
    let mut output = std::io::BufWriter::with_capacity(64 * 1024, std::io::stdout().lock());
    if let Some(path) = std::env::var_os("VEYRA_FAKE_FIXTURE") {
        for record in load(Path::new(&path))? {
            match record {
                Record::TranscriptStart { .. } => {}
                Record::Spin { ms } => std::thread::sleep(std::time::Duration::from_millis(ms)),
                Record::Exit { code, stderr } => {
                    for line in stderr {
                        writeln!(output, "{line}\r")?;
                    }
                    output.flush()?;
                    return Ok(code);
                }
                Record::EmitRaw { base64, repeat } => {
                    use base64::Engine;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(base64)
                        .map_err(io::Error::other)?;
                    for _ in 0..repeat {
                        output.write_all(&bytes)?;
                    }
                    output.flush()?;
                }
                _ => {
                    return Err(io::Error::other("Unsupported TUI fixture record"));
                }
            }
        }
    }
    output.write_all(b"VEYRA_TUI_READY\r\n")?;
    output.flush()?;
    let mut input = std::io::stdin().lock();
    let mut bytes = [0; 1];
    let mut line = Vec::new();
    while input.read(&mut bytes)? != 0 {
        writeln!(output, "{:02x}\r", bytes[0])?;
        output.flush()?;
        match bytes[0] {
            b'\r' | b'\n' => {
                if let Some((path, native)) = &history {
                    append_tui_history(path, native, &String::from_utf8_lossy(&line))?;
                }
                line.clear();
            }
            byte => line.push(byte),
        }
    }
    Ok(0)
}

fn append_tui_history(path: &Path, native: &str, text: &str) -> io::Result<()> {
    use std::io::{BufRead, Write};
    let mut parent = None;
    if let Ok(file) = std::fs::File::open(path) {
        for line in std::io::BufReader::new(file).lines() {
            let entry: Value = serde_json::from_str(&line?)?;
            if let Some(uuid) = entry["uuid"].as_str() {
                parent = Some(uuid.to_owned());
            }
        }
    }
    let user = uuid::Uuid::now_v7().to_string();
    let attachment = uuid::Uuid::now_v7().to_string();
    let assistant = uuid::Uuid::now_v7().to_string();
    let common = serde_json::json!({"sessionId":native,"isSidechain":false,"entrypoint":"cli","userType":"external","cwd":std::env::current_dir()?,"version":"2.1.302","timestamp":"2026-10-17T00:00:00Z","gitBranch":""});
    let mut user_entry = common.clone();
    for (key,value) in serde_json::json!({"uuid":user,"parentUuid":parent,"promptId":uuid::Uuid::now_v7().to_string(),"type":"user","message":{"role":"user","content":text},"promptSource":"typed","turnOrigin":"human","origin":{"kind":"human"},"turnPosition":{"promptIndex":0,"turnIndex":1},"permissionMode":"default"}).as_object().unwrap() { user_entry[key] = value.clone(); }
    let mut attachment_entry = common.clone();
    for (key,value) in serde_json::json!({"uuid":attachment,"parentUuid":user,"type":"attachment","attachment":{"type":"environment"},"rendered":[],"renderedRole":"system"}).as_object().unwrap() { attachment_entry[key] = value.clone(); }
    let mut assistant_entry = common;
    for (key,value) in serde_json::json!({"uuid":assistant,"parentUuid":attachment,"type":"assistant","message":{"role":"assistant","model":"veyra-fake","id":uuid::Uuid::now_v7().to_string(),"content":[{"type":"text","text":format!("fake reply: {text}")}],"stop_reason":"end_turn","usage":{}},"apiBlockIndex":0,"requestId":uuid::Uuid::now_v7().to_string(),"requestedModel":"veyra-fake","effort":"default"}).as_object().unwrap() { assistant_entry[key] = value.clone(); }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    for entry in [user_entry, attachment_entry, assistant_entry] {
        writeln!(file, "{entry}")?;
    }
    file.flush()
}

fn once() -> usize {
    1
}

pub fn load(path: &Path) -> io::Result<Vec<Record>> {
    std::fs::read_to_string(path)?
        .lines()
        .enumerate()
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|error| {
                io::Error::other(format!("{}:{}: {error}", path.display(), index + 1))
            })
        })
        .collect()
}

pub(crate) fn wildcard(value: &str) -> bool {
    value == "<any>" || value == "<cwd>" || (value.starts_with("<id:") && value.ends_with('>'))
}

#[derive(Default)]
pub struct Matcher {
    bindings: BTreeMap<String, Value>,
}
impl Matcher {
    pub fn bind(&mut self, name: &str, value: Value) {
        self.bindings.insert(name.into(), value);
    }
    pub fn matches(&mut self, expected: &Value, actual: &Value) -> bool {
        match expected {
            Value::String(text) if text == "<any>" => true,
            Value::String(text) if wildcard(text) => {
                if let Some(value) = self.bindings.get(text) {
                    value == actual
                } else {
                    self.bindings.insert(text.clone(), actual.clone());
                    true
                }
            }
            Value::Object(object) => actual.as_object().is_some_and(|other| {
                object.len() == other.len()
                    && object.iter().all(|(key, value)| {
                        other
                            .get(key)
                            .is_some_and(|actual| self.matches(value, actual))
                    })
            }),
            Value::Array(array) => actual.as_array().is_some_and(|other| {
                array.len() == other.len()
                    && array.iter().zip(other).all(|(a, b)| self.matches(a, b))
            }),
            value => value == actual,
        }
    }
    pub fn expand(&self, value: Value) -> Value {
        match value {
            Value::String(text) => self
                .bindings
                .get(&text)
                .cloned()
                .unwrap_or(Value::String(text)),
            Value::Array(array) => {
                Value::Array(array.into_iter().map(|v| self.expand(v)).collect())
            }
            Value::Object(object) => Value::Object(
                object
                    .into_iter()
                    .map(|(k, v)| (k, self.expand(v)))
                    .collect(),
            ),
            value => value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn bindings_match_repeated_ids_and_expand_replies() {
        let mut matcher = Matcher::default();
        assert!(matcher.matches(
            &json!({"id":"<id:request>","data":"<any>"}),
            &json!({"id":7,"data":null})
        ));
        assert_eq!(
            matcher.expand(json!({"reply":"<id:request>"})),
            json!({"reply":7})
        );
        assert!(!matcher.matches(&json!("<id:request>"), &json!(8)));
        assert!(!matcher.matches(&json!({"id":7}), &json!({"id":7,"extra":true})));
    }
}
