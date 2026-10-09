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
    let mut recorder = Recorder::create(&path, "claude", "test", &scratch.0, Some(token)).unwrap();
    recorder.bind("native-session".into(), "<id:session>".into());
    recorder.line(true, json!({"type":"user","uuid":"prompt-id","session_id":"native-session","cwd":scratch.0,"access_token":token,"password":"my-password","message":"Bearer another-credential"})).unwrap();
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
        Recorder::create(&path, "claude", "unknown", &scratch.0, Some(token)).unwrap();
    recorder.line(true, json!({"access_token":token})).unwrap();
    recorder.line(false,json!({"type":"system","subtype":"init","claude_code_version":"2.1.295","account":{"email":"private-account-placeholder"}})).unwrap();
    recorder.exit(0).unwrap();
    let rows: Vec<serde_json::Value> = fs::read_to_string(&path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows[0]["cli_version"], "2.1.295");
    assert_eq!(rows[1]["line"]["access_token"], "<id:token>");
    assert_eq!(rows[2]["line"]["account"], "<account>");
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
    let mut recorder = Recorder::create(&path, "claude", "test", &scratch.0, None).unwrap();
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
                    !record::secret_shapes(text),
                    "{}:{} contains a secret shape",
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
}
