#[path = "common/live.rs"]
mod harness;
use veyra_engine::provider::ProviderKind;
#[test]
#[ignore = "requires the human's consent for a live Codex turn"]
fn live_codex_ok_reply() {
    harness::ok_reply(ProviderKind::Codex);
}
#[test]
#[ignore = "requires the human's consent for live Codex approval turns"]
fn live_codex_approval_round_trip() {
    harness::approval_round_trip(ProviderKind::Codex);
}
#[test]
#[ignore = "requires the human's consent for a live Codex full-access write"]
fn live_codex_full_access_write() {
    harness::full_access_write(ProviderKind::Codex);
}
#[test]
#[ignore = "requires the human's consent; records whether Codex sends a native user question"]
fn live_codex_question() {
    harness::question(ProviderKind::Codex);
}
