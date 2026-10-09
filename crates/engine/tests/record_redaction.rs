#![cfg(debug_assertions)]
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
};
use veyra_engine::{
    domain::ThreadId,
    platform,
    record::{self, Recorder},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("veyra-record-{}", ThreadId::new()));
        platform::private_dir(&root).unwrap();
        Self(root)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn capture_never_contains_token() {
    let scratch = Scratch::new();
    let path = scratch.0.join("t1-s1.jsonl");
    let token = "veyra-test-credential-abcdefgh";
    let mut recorder =
        Recorder::create(&path, "claude", "test", &scratch.0, None, Some(token)).unwrap();
    recorder.bind("native-session".into(), "<id:session>".into());
    recorder.line(true, json!({
        "type":"user", "uuid":"prompt-id", "session_id":"native-session", "cwd":scratch.0,
        "access_token":token, "password":"my-password", "message":"Bearer another-credential"
    })).unwrap();
    recorder
        .line(
            false,
            json!({"api_key":"another-secret","output":format!("token={token}")}),
        )
        .unwrap();
    recorder.exit(0).unwrap();
    let text = fs::read_to_string(path).unwrap();
    assert!(
        !text.contains(token)
            && !text.contains("another-secret")
            && !text.contains("my-password")
            && !text.contains("another-credential")
    );
    assert!(text.contains("<id:session>") && text.contains("<cwd>") && text.contains("<any>"));
    assert!(!record::secret_shapes(&text));
}
#[test]
fn capture_binds_token_and_updates_cli_version() {
    let scratch = Scratch::new();
    let path = scratch.0.join("capture.jsonl");
    let token = "session-credential-value";
    let mut recorder =
        Recorder::create(&path, "claude", "unknown", &scratch.0, None, Some(token)).unwrap();
    recorder.line(true, json!({"access_token":token})).unwrap();
    recorder
        .line(
            false,
            json!({
                "type":"system", "subtype":"init", "claude_code_version":"2.1.295",
                "account":{"email":"private-account-placeholder"}
            }),
        )
        .unwrap();
    recorder.exit(0).unwrap();
    let rows: Vec<serde_json::Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["cli_version"], "2.1.295");
    assert_eq!(rows[1]["line"]["access_token"], "<id:token>");
    assert_eq!(rows[2]["line"]["account"], json!({"email":"<any>"}));
    assert!(
        !fs::read_to_string(&path)
            .unwrap()
            .contains("private-account-placeholder")
    );
    assert!(platform::is_private(&path).unwrap());
}
#[test]
fn capture_files_are_0600() {
    let scratch = Scratch::new();
    let path = scratch.0.join("capture.jsonl");
    let mut recorder = Recorder::create(&path, "claude", "test", &scratch.0, None, None).unwrap();
    recorder
        .line(true, json!({"type":"user","message":"ok"}))
        .unwrap();
    assert!(platform::is_private(&path).unwrap());
    assert_eq!(fs::read_to_string(path).unwrap().lines().count(), 2);
}
fn sweep(path: &Path) {
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sweep(&path);
        } else {
            for (line, text) in fs::read_to_string(&path).unwrap().lines().enumerate() {
                assert!(
                    !private_detail(text),
                    "{}:{} contains a private or secret shape",
                    path.display(),
                    line + 1
                );
            }
        }
    }
}
#[test]
fn fixtures_hold_no_secret_shapes() {
    let path = std::env::var_os("VEYRA_TEST_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"));
    sweep(&path);
    assert!(record::secret_shapes(&format!("Bearer {}", "a".repeat(30))));
    assert!(record::secret_shapes(&format!("xai-{}", "a".repeat(16))));
    assert!(!record::secret_shapes(&format!("xai-{}", "a".repeat(15))));
    assert!(record::secret_shapes("password=[redacted]"));
    assert!(!record::secret_shapes(
        "Bearer <id:token> password=<any> url=https://user:<any>@example.invalid"
    ));
    assert!(record::secret_shapes(
        "/Users/example-person/private-project"
    ));
}

#[test]
fn capture_hides_hook_output_and_preserves_account_keys() {
    let scratch = Scratch::new();
    let path = scratch.0.join("capture.jsonl");
    let mut recorder = Recorder::create(&path, "claude", "test", &scratch.0, None, None).unwrap();
    recorder
        .line(
            false,
            json!({
                "type":"system", "subtype":"hook_response", "stdout":"personal hook prose",
                "stderr":"personal diagnostic", "output":{"content":"personal nested output"},
            }),
        )
        .unwrap();
    recorder
        .line(
            false,
            json!({"account":{"organization":{"name":"personal account prose"}}}),
        )
        .unwrap();
    let text = fs::read_to_string(path).unwrap();
    assert!(!text.contains("personal"));
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[1]["line"]["stdout"], "<hook-output>");
    assert_eq!(rows[1]["line"]["stderr"], "<hook-output>");
    assert_eq!(
        rows[2]["line"]["account"],
        json!({"organization":{"name":"<any>"}})
    );
}

fn joined_blocks(rows: &[serde_json::Value]) -> Vec<String> {
    let mut blocks = std::collections::BTreeMap::<(String, u64), String>::new();
    let mut message = String::new();
    for row in rows {
        let wire = &row["line"];
        let event = &wire["event"];
        if event["type"] == "message_start" {
            message = event["message"]["id"].as_str().unwrap_or_default().into();
        }
        let index = event["index"].as_u64().unwrap_or_default();
        for value in [
            &event["content_block"]["text"],
            &event["content_block"]["thinking"],
            &event["delta"]["text"],
            &event["delta"]["thinking"],
            &event["delta"]["partial_json"],
        ] {
            if let Some(text) = value.as_str() {
                blocks
                    .entry((message.clone(), index))
                    .or_default()
                    .push_str(text);
            }
        }
        if let Some(content) = wire["message"]["content"].as_array() {
            for (index, block) in content.iter().enumerate() {
                for field in ["text", "thinking", "content"] {
                    if let Some(text) = block[field].as_str() {
                        blocks.insert(
                            (
                                format!("final:{}:{}", wire["type"], wire["message"]["id"]),
                                index as u64,
                            ),
                            text.into(),
                        );
                    }
                }
            }
        }
    }
    blocks.into_values().collect()
}
fn private_detail(text: &str) -> bool {
    static DETAIL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(concat!(
            r"(?i)\b[\w.+-]+@[\w.-]+\.[a-z]{2,}\b|",
            r#"\b(?:account|org(?:anization)?|user)[_-]?id["\s]*[:=]\s*"?[^\s",<>]+"#,
        ))
        .unwrap()
    });
    record::secret_shapes(text) || DETAIL.is_match(text) || text.contains("fixture-private-detail")
}
#[test]
fn fixtures_joined_blocks_hold_no_personal_detail_or_secrets() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude");
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let rows: Vec<serde_json::Value> = fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        for text in joined_blocks(&rows) {
            assert!(
                !private_detail(&text),
                "{} has private assembled block text",
                path.display()
            );
        }
    }
    let planted = [
        json!({"line":{"event":{"type":"content_block_delta","delta":{"text":"Bear"}}}}),
        json!({"line":{"event":{"type":"content_block_delta","delta":{"text":"er fixture-credential"}}}}),
    ];
    assert!(private_detail(&joined_blocks(&planted)[0]));
    assert!(!private_detail("Veyra checks the command."));
}

#[test]
fn capture_binds_home_folder_in_nested_paths_and_text() {
    let scratch = Scratch::new();
    let path = scratch.0.join("capture.jsonl");
    let home = "/fixture-home";
    let mut recorder = Recorder::create(
        &path,
        "claude",
        "test",
        &scratch.0,
        Some(Path::new(home)),
        None,
    )
    .unwrap();
    recorder
        .line(
            false,
            json!({"paths":[format!("{home}/.config/fixture")],
        "text":format!("Read {home}/notes")}),
        )
        .unwrap();
    let rows: Vec<serde_json::Value> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        rows[1]["line"],
        json!({"paths":["<home>/.config/fixture"], "text":"Read <home>/notes"})
    );
}

#[test]
fn privacy_sweep_detects_shapes_and_a_planted_split_needle() {
    for detail in [
        format!("fixture-person@{}.invalid", "example"),
        "account_id=fixture-account-0001".into(),
        "/Users/fixture-person/notes".into(),
    ] {
        assert!(private_detail(&detail), "private shape escaped the sweep");
    }
    let scratch = Scratch::new();
    let path = scratch.0.join("planted.jsonl");
    let rows = [
        json!({"line":{"event":{"type":"content_block_delta","delta":{"text":"fixture-private-"}}}}),
        json!({"line":{"event":{"type":"content_block_delta","delta":{"text":"detail"}}}}),
    ];
    fs::write(&path, rows.map(|row| row.to_string()).join("\n")).unwrap();
    let planted: Vec<serde_json::Value> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(private_detail(&joined_blocks(&planted)[0]));
    assert!(!private_detail("Veyra checks the command."));
}
