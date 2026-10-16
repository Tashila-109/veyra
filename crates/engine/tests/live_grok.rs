#[path = "common/live.rs"]
mod harness;
use veyra_engine::provider::ProviderKind;
#[test]
#[ignore = "requires the human's consent for a live Grok turn"]
fn live_grok_ok_reply() {
    harness::ok_reply(ProviderKind::Grok);
}
#[test]
#[ignore = "requires the human's consent for live Grok approval turns"]
fn live_grok_approval_round_trip() {
    harness::approval_round_trip(ProviderKind::Grok);
}
#[test]
#[ignore = "requires the human's consent for a live Grok full-access write"]
fn live_grok_full_access_write() {
    harness::full_access_write(ProviderKind::Grok);
}
#[test]
#[ignore = "requires the human's consent; records whether Grok sends a native user question"]
fn live_grok_question() {
    harness::question(ProviderKind::Grok);
}
