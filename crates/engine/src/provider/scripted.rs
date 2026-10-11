use super::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub struct Scripted {
    pub events: Arc<Mutex<Vec<ProviderEvent>>>,
    pub controls: Arc<Mutex<Vec<SessionControl>>>,
    pub terminals: Arc<Mutex<Vec<TerminalControl>>>,
    pub timings: Arc<Mutex<Vec<u128>>>,
    pub auto_ready: bool,
    pub auto_complete: bool,
}
impl Scripted {
    pub fn spawn(
        &self,
        spec: SessionSpec,
        out: mpsc::Sender<EngineMsg>,
        epoch: u32,
    ) -> SessionHandle {
        let (control, mut rx) = mpsc::unbounded_channel::<SessionControl>();
        let this = self.clone();
        tokio::spawn(async move {
            let emit = |event| EngineMsg::Provider {
                thread: spec.thread,
                epoch,
                event,
            };
            if this.auto_ready {
                let _ = out
                    .send(emit(ProviderEvent::Session(SessionEvent::Ready {
                        native: NativeSessionId(format!("script-{}", spec.thread)),
                        cli_version: Some("scripted".into()),
                        pid: 1,
                        pgid: 1,
                        capabilities: vec![],
                    })))
                    .await;
            }
            let events = std::mem::take(&mut *this.events.lock().unwrap());
            for event in events {
                if out.send(emit(event)).await.is_err() {
                    return;
                }
            }
            while let Some(c) = rx.recv().await {
                this.controls.lock().unwrap().push(c.clone());
                match c {
                    SessionControl::Shutdown => {
                        let _ = out
                            .send(emit(ProviderEvent::Session(SessionEvent::Exited {
                                crash: None,
                            })))
                            .await;
                        break;
                    }
                    SessionControl::StartTurn { turn, .. } if this.auto_complete => {
                        for event in [
                            TurnEvent::Accepted,
                            TurnEvent::TurnEnded {
                                park: false,
                                outcome: TurnOutcome::Completed,
                            },
                        ] {
                            if out
                                .send(emit(ProviderEvent::Turn {
                                    turn: Some(turn),
                                    event,
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    SessionControl::StartTurn { .. }
                    | SessionControl::Interrupt { .. }
                    | SessionControl::Answer { .. } => {}
                }
            }
        });
        SessionHandle { control }
    }
    pub fn terminal(&self) -> TerminalHandle {
        let (control, mut rx) = mpsc::unbounded_channel();
        let records = self.terminals.clone();
        tokio::spawn(async move {
            while let Some(c) = rx.recv().await {
                records.lock().unwrap().push(c);
            }
        });
        TerminalHandle { control }
    }
}
