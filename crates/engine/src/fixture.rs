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
