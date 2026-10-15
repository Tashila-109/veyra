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
fn auth_method_catalog(capture: &Captures<'_>) -> bool {
    capture.get(1).is_some_and(|prefix| {
        prefix
            .as_str()
            .trim_end()
            .strip_suffix(':')
            .is_some_and(|field| field.trim_end().strip_suffix('"') == Some("authMethods"))
    }) && capture
        .get(2)
        .is_some_and(|value| value.as_str().starts_with('['))
}
pub fn secret_shapes(text: &str) -> bool {
    HOME_PATH.is_match(text)
        || MATCHES.iter().any(|rule| {
            rule.expression.captures_iter(text).any(|capture| {
                !auth_method_catalog(&capture)
                    && !crate::fixture::wildcard(&capture[rule.secret_group])
            })
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
                if auth_method_catalog(capture) {
                    return capture[0].to_owned();
                }
                let mut replacement = String::new();
                capture.expand(rule.replacement, &mut replacement);
                replacement
            })
            .into_owned();
    }
    text
}

#[cfg(debug_assertions)]
pub(crate) struct Metadata {
    pub native: Option<String>,
    pub version: Option<String>,
}

#[cfg(debug_assertions)]
mod capture {
    use super::redact;
    use crate::{fixture::Record, platform};
    use serde::Deserialize;
    use serde_json::Value;
    use std::{
        collections::BTreeMap,
        fs::{File, OpenOptions},
        io::{self, Read, Seek, SeekFrom, Write},
        path::Path,
    };

    #[derive(Default, Deserialize)]
    enum EnvelopeKind {
        #[serde(rename = "system")]
        System,
        #[default]
        #[serde(other)]
        Other,
    }
    #[derive(Deserialize)]
    #[serde(default)]
    struct Envelope<'a> {
        #[serde(rename = "type")]
        kind: EnvelopeKind,
        #[serde(borrow)]
        subtype: &'a str,
    }
    impl Default for Envelope<'_> {
        fn default() -> Self {
            Self {
                kind: EnvelopeKind::Other,
                subtype: "",
            }
        }
    }
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
            provider: crate::provider::ProviderKind,
            version: &str,
            cwd: &Path,
            home: Option<&Path>,
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
            if let Some(home) = home.filter(|home| !home.as_os_str().is_empty()) {
                recorder.bind(home.to_string_lossy().into_owned(), "<home>".into());
            }
            if let Some(token) = token {
                recorder.bind(token.into(), "<id:token>".into());
            }
            recorder.write(&Record::TranscriptStart {
                provider: provider.binary().into(),
                protocol: match provider {
                    crate::provider::ProviderKind::Claude => "stream-json",
                    crate::provider::ProviderKind::Codex | crate::provider::ProviderKind::Grok => {
                        "json-rpc"
                    }
                }
                .into(),
                cli_version: version.into(),
                scenario: "recorded".into(),
                source: "Veyra recording".into(),
            })?;
            Ok(recorder)
        }
        pub fn bind(&mut self, value: String, mut replacement: String) {
            if self.bindings.contains_key(&value) {
                return;
            }
            if replacement.starts_with("<id:") && replacement.ends_with('>') {
                let base = replacement.trim_end_matches('>').to_owned();
                let mut suffix = 1;
                while self.bindings.values().any(|known| known == &replacement) {
                    replacement = format!("{base}:{suffix}>");
                    suffix += 1;
                }
            }
            self.bindings.insert(value, replacement);
        }
        pub(crate) fn metadata(&mut self, metadata: &super::Metadata) -> io::Result<()> {
            if let Some(native) = &metadata.native {
                self.bind(native.clone(), "<id:session>".into());
            }
            self.version(metadata.version.as_deref())
        }
        pub fn version(&mut self, version: Option<&str>) -> io::Result<()> {
            if let Some(version) = version.filter(|version| !version.is_empty()) {
                self.file.seek(SeekFrom::Start(0))?;
                let mut existing = String::new();
                self.file.read_to_string(&mut existing)?;
                let (header, rest) = existing
                    .split_once('\n')
                    .ok_or_else(|| io::Error::other("capture header is missing"))?;
                let mut header: Record = serde_json::from_str(header)?;
                if let Record::TranscriptStart { cli_version, .. } = &mut header {
                    *cli_version = version.into();
                }
                self.file.set_len(0)?;
                self.file.seek(SeekFrom::Start(0))?;
                serde_json::to_writer(&mut self.file, &header)?;
                self.file.write_all(b"\n")?;
                self.file.write_all(rest.as_bytes())?;
            }
            Ok(())
        }
        pub fn line(&mut self, outbound: bool, mut value: Value) -> io::Result<()> {
            if Envelope::deserialize(&value).is_ok_and(|envelope| {
                matches!(envelope.kind, EnvelopeKind::System)
                    && envelope.subtype.starts_with("hook_")
            }) {
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
                && (text == "<id:token>"
                    || self
                        .bindings
                        .get(text)
                        .is_some_and(|binding| binding == "<id:token>"))
            {
                return "<id:token>".into();
            }
            if key == Some("authMethods")
                && let Value::Array(methods) = &value
            {
                return Value::Array(
                    methods
                        .iter()
                        .cloned()
                        .map(|method| self.sanitize(method, None))
                        .collect(),
                );
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
            if let Value::Object(pair) = &value
                && let Some(name) = pair.get("name").and_then(Value::as_str)
                && pair.contains_key("value")
                && ["token", "key", "secret", "password", "auth"]
                    .iter()
                    .any(|part| name.to_lowercase().contains(part))
            {
                let mut pair = pair.clone();
                pair.insert(
                    "value".into(),
                    self.sanitize(pair["value"].clone(), Some(name)),
                );
                return Value::Object(pair);
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
