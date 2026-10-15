use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{runtime::Runtime, sync::mpsc};
use veyra_engine::{
    actor::EngineMsg,
    domain::*,
    provider::{env::ChildEnv, *},
};
pub struct Scratch(pub PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
pub fn fixture(provider: ProviderKind, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(provider.binary())
        .join(format!("{name}.jsonl"))
}
pub fn environment(provider: ProviderKind, name: &str) -> (Scratch, ChildEnv) {
    let root = Path::new("/tmp").join(format!(
        "veyra-{}-replay-{}",
        provider.binary(),
        ThreadId::new()
    ));
    fs::create_dir(&root).unwrap();
    fs::copy(fixture(provider, name), root.join("t1-s1.jsonl")).unwrap();
    let probe = if provider == ProviderKind::Grok {
        "probe-grok"
    } else {
        "probe-ready"
    };
    fs::copy(
        fixture(provider, probe),
        root.join(format!("probe-{}.jsonl", provider.binary())),
    )
    .unwrap();
    let mut env =
        ChildEnv::isolated().with_binary(provider, env!("CARGO_BIN_EXE_veyra-fake-cli").into());
    env.fixtures = Some(root.clone());
    env.record = None;
    (Scratch(root), env)
}
#[derive(Default)]
pub struct Inputs {
    pub stop_when_accepted: bool,
    pub requests: std::collections::VecDeque<Input>,
    pub multi: bool,
}
pub enum Input {
    Answer(Decision),
    Stop,
}
pub fn replay(
    provider: ProviderKind,
    name: &str,
    mode: PermissionMode,
    root: bool,
    resume: bool,
    mut inputs: Inputs,
) -> (Vec<ProviderEvent>, bool) {
    let (scratch, mut env) = environment(provider, name);
    Runtime::new().unwrap().block_on(async {
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let (out, mut rx) = mpsc::channel(256);
        if root {
            veyra_engine::platform::private_dir(&scratch.0.join("data")).unwrap();
            env.mcp = Some(std::sync::Arc::new(
                veyra_engine::mcp::Server::bind(
                    &scratch.0.join("data"),
                    std::env::current_exe().unwrap(),
                    out.clone(),
                )
                .unwrap(),
            ));
        }
        let handle = Spawner::Cli(Box::new(env))
            .spawn(
                provider,
                SessionSpec {
                    thread,
                    cwd: scratch.0.canonicalize().unwrap(),
                    config: ThreadConfig {
                        model: (provider == ProviderKind::Codex).then(|| "veyra-model".into()),
                        effort: (provider == ProviderKind::Codex).then(|| "medium".into()),
                        mode,
                    },
                    native: resume.then(|| {
                        NativeSessionId(
                            if provider == ProviderKind::Codex {
                                "missing-native"
                            } else {
                                "retained-session"
                            }
                            .into(),
                        )
                    }),
                    import: false,
                    root,
                },
                out,
                1,
            )
            .unwrap();
        let events = tokio::time::timeout(Duration::from_secs(15), async {
            let mut events = Vec::new();
            let mut sent = false;
            while let Some(EngineMsg::Provider { event, .. }) = rx.recv().await {
                match &event {
                    ProviderEvent::Session(SessionEvent::Ready { .. }) if !sent => {
                        handle
                            .control
                            .send(SessionControl::StartTurn {
                                turn,
                                text: "reply with the word ok".into(),
                            })
                            .unwrap();
                        sent = true;
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::Accepted,
                        ..
                    } if inputs.stop_when_accepted => {
                        inputs.stop_when_accepted = false;
                        handle
                            .control
                            .send(SessionControl::Interrupt {
                                turn,
                                reason: StopReason::User,
                            })
                            .unwrap();
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::RequestOpened { native, .. },
                        ..
                    } => {
                        let input = inputs
                            .requests
                            .pop_front()
                            .expect("test must supply an input for every request");
                        let message = match input {
                            Input::Stop => SessionControl::Interrupt {
                                turn,
                                reason: StopReason::User,
                            },
                            Input::Answer(reply) => SessionControl::Answer {
                                native: native.clone(),
                                reply,
                            },
                        };
                        handle.control.send(message).unwrap();
                    }
                    ProviderEvent::Turn {
                        event: TurnEvent::TurnEnded { .. },
                        ..
                    } if inputs.multi => {
                        inputs.multi = false;
                        handle
                            .control
                            .send(SessionControl::StartTurn {
                                turn: TurnId::new(),
                                text: "second prompt".into(),
                            })
                            .unwrap();
                    }
                    _ => {}
                }
                let exited = matches!(event, ProviderEvent::Session(SessionEvent::Exited { .. }));
                events.push(event);
                if exited {
                    break;
                }
            }
            assert!(inputs.requests.is_empty(), "test inputs were not consumed");
            assert!(sent, "provider never reached Ready: {events:?}");
            assert!(
                events.iter().any(|e| matches!(
                    e,
                    ProviderEvent::Session(SessionEvent::Exited { crash: None })
                )),
                "provider crashed: {events:?}"
            );
            events
        })
        .await
        .unwrap();
        (events, scratch.0.join("veyra-check.txt").exists())
    })
}
