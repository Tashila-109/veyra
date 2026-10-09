use regex::{Captures, Regex};
use std::sync::LazyLock;

struct Redaction {
    expression: Regex,
    secret_group: usize,
    replacement: &'static str,
}
static MATCHES: LazyLock<Vec<Redaction>> = LazyLock::new(|| {
    [
        (r#"(?i)(Bearer\s+)([^\s",]+)"#, 2, "${1}[redacted]"),
        (
            r#"(?i)([\w.-]*(?:token|key|secret|password|auth)[\w.-]*["\s]*[:=]\s*"?)([^\s",]+)"#,
            2,
            "${1}[redacted]",
        ),
        (
            r"\b((?:sk-[A-Za-z0-9_-]{17,}|xai-[A-Za-z0-9_-]{16,}))",
            1,
            "[redacted]",
        ),
        (r"(://[^:/\s]+:)([^@\s]+)(@)", 2, "${1}[redacted]@"),
    ]
    .into_iter()
    .map(|(pattern, secret_group, replacement)| Redaction {
        expression: Regex::new(pattern).expect("redaction expression"),
        secret_group,
        replacement,
    })
    .collect()
});
static HOME_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"/(?:Users|home)/[^/\s"<>]+(?:/|\b)"#).expect("personal home path expression")
});
pub fn secret_shapes(text: &str) -> bool {
    HOME_PATH.is_match(text)
        || MATCHES.iter().any(|rule| {
            rule.expression
                .captures_iter(text)
                .any(|capture| !crate::fixture::wildcard(&capture[rule.secret_group]))
        })
}
pub fn redact(text: &str, token: Option<&str>) -> String {
    let mut text = match token.filter(|token| !token.is_empty()) {
        Some(token) => text.replace(token, "[redacted]"),
        None => text.to_owned(),
    };
    for rule in MATCHES.iter() {
        text = rule
            .expression
            .replace_all(&text, |capture: &Captures<'_>| {
                let mut replacement = String::new();
                capture.expand(rule.replacement, &mut replacement);
                replacement
            })
            .into_owned();
    }
    text
}

#[cfg(debug_assertions)]
mod capture {
    use super::redact;
    use crate::{fixture::Record, platform};
    use serde_json::Value;
    use std::{
        collections::BTreeMap,
        fs::{File, OpenOptions},
        io::{self, Read, Seek, SeekFrom, Write},
        path::Path,
    };

    fn mask_account(value: Value) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object
                    .into_iter()
                    .map(|(key, value)| (key, mask_account(value)))
                    .collect(),
            ),
            Value::Array(array) => Value::Array(array.into_iter().map(mask_account).collect()),
            _ => "<any>".into(),
        }
    }
    fn mask_hook_output(value: &mut Value) {
        match value {
            Value::Object(object) => {
                for (key, value) in object {
                    if ["stdout", "stderr", "output", "content"].contains(&key.as_str()) {
                        *value = "<hook-output>".into();
                    } else {
                        mask_hook_output(value);
                    }
                }
            }
            Value::Array(array) => array.iter_mut().for_each(mask_hook_output),
            _ => {}
        }
    }
    pub struct Recorder {
        file: File,
        bindings: BTreeMap<String, String>,
    }
    impl Recorder {
        pub fn create(
            path: &Path,
            provider: &str,
            version: &str,
            cwd: &Path,
            token: Option<&str>,
        ) -> io::Result<Self> {
            if let Some(parent) = path.parent() {
                platform::private_dir(parent)?;
            }
            let file = platform::private_file(
                path,
                OpenOptions::new().create_new(true).read(true).write(true),
            )?;
            let mut recorder = Self {
                file,
                bindings: BTreeMap::new(),
            };
            recorder.bind(cwd.to_string_lossy().into_owned(), "<cwd>".into());
            if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
                recorder.bind(home.to_string_lossy().into_owned(), "<home>".into());
            }
            if let Some(token) = token {
                recorder.bind(token.into(), "<id:token>".into());
            }
            recorder.write(&Record::TranscriptStart {
                provider: provider.into(),
                protocol: "stream-json".into(),
                cli_version: version.into(),
                scenario: "recorded".into(),
                source: "Veyra recording".into(),
            })?;
            Ok(recorder)
        }
        pub fn bind(&mut self, value: String, replacement: String) {
            self.bindings.insert(value, replacement);
        }
        pub fn line(&mut self, outbound: bool, mut value: Value) -> io::Result<()> {
            if value["type"] == "system"
                && value["subtype"] == "init"
                && let Some(version) = value["claude_code_version"].as_str()
            {
                self.file.seek(SeekFrom::Start(0))?;
                let mut existing = String::new();
                self.file.read_to_string(&mut existing)?;
                let (header, rest) = existing
                    .split_once('\n')
                    .ok_or_else(|| io::Error::other("capture header is missing"))?;
                let mut header: Value = serde_json::from_str(header)?;
                header["cli_version"] = version.into();
                self.file.set_len(0)?;
                self.file.seek(SeekFrom::Start(0))?;
                serde_json::to_writer(&mut self.file, &header)?;
                self.file.write_all(b"\n")?;
                self.file.write_all(rest.as_bytes())?;
            }
            if value["type"] == "system"
                && value["subtype"]
                    .as_str()
                    .is_some_and(|subtype| subtype.starts_with("hook_"))
            {
                mask_hook_output(&mut value);
            }
            let line = self.sanitize(value, None);
            self.write(&if outbound {
                Record::ExpectOutbound { line }
            } else {
                Record::EmitInbound {
                    line,
                    after_ms: None,
                }
            })
        }
        pub fn exit(&mut self, code: i32) -> io::Result<()> {
            self.write(&Record::Exit {
                code,
                stderr: vec![],
            })
        }
        fn sanitize(&mut self, value: Value, key: Option<&str>) -> Value {
            if let Value::String(text) = &value
                && self
                    .bindings
                    .get(text)
                    .is_some_and(|binding| binding == "<id:token>")
            {
                return "<id:token>".into();
            }
            if key == Some("account") {
                return mask_account(value);
            }
            if key.is_some_and(|key| {
                ["token", "key", "secret", "password", "auth"]
                    .iter()
                    .any(|needle| key.to_lowercase().contains(needle))
            }) {
                return Value::String("<any>".into());
            }
            match value {
                Value::Object(object) => Value::Object(
                    object
                        .into_iter()
                        .map(|(key, value)| {
                            let value = self.sanitize(value, Some(&key));
                            (key, value)
                        })
                        .collect(),
                ),
                Value::Array(array) => Value::Array(
                    array
                        .into_iter()
                        .map(|value| self.sanitize(value, key))
                        .collect(),
                ),
                Value::String(text) => {
                    if let Some(binding) = self.bindings.get(&text) {
                        return Value::String(binding.clone());
                    }
                    if key.is_some_and(|key| {
                        [
                            "uuid",
                            "session_id",
                            "request_id",
                            "command_uuid",
                            "user_message_uuid",
                            "user_message_uuids",
                        ]
                        .contains(&key)
                    }) {
                        let binding = format!("<id:wire{}>", self.bindings.len());
                        self.bind(text, binding.clone());
                        return Value::String(binding);
                    }
                    let mut text = text;
                    for (value, binding) in &self.bindings {
                        text = text.replace(value, binding);
                    }
                    Value::String(redact(&text, None).replace("[redacted]", "<any>"))
                }
                value => value,
            }
        }
        fn write(&mut self, record: &Record) -> io::Result<()> {
            serde_json::to_writer(&mut self.file, record)?;
            self.file.write_all(b"\n")?;
            self.file.flush()
        }
    }
}
#[cfg(debug_assertions)]
pub use capture::Recorder;
