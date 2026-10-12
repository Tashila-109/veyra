use crate::{
    commands::{end_turn, queue_wake, set_hold},
    domain::*,
    events::{ChangeEvent as Ev, ChangeSet},
};

pub fn decide(p: &Projection, now: Millis) -> ChangeSet {
    let mut c = ChangeSet::default();
    c.live.extend(
        p.threads
            .keys()
            .map(|&id| (id, crate::live::LiveThread::default())),
    );
    for t in p.turns.values() {
        if matches!(t.state, TurnState::Queued | TurnState::Running { .. }) {
            set_hold(p, &mut c, t.thread, Some(HoldReason::Restarted));
        }
        if matches!(t.state, TurnState::Running { .. })
            || (t.state == TurnState::Queued && p.attempted_starts.contains_key(&t.id))
        {
            end_turn(
                p,
                &mut c,
                {
                    let mut t = t.clone();
                    if let Some(since) = p.attempted_starts.get(&t.id) {
                        t.state = TurnState::Running { since: *since };
                    }
                    t
                },
                TurnOutcome::Interrupted {
                    reason: crate::provider::StopReason::AppClosing.text().into(),
                },
                now,
            );
        }
    }
    for req in p
        .requests
        .values()
        .filter(|r| r.state == RequestState::Pending)
    {
        if !c.requests.contains_key(&req.id) {
            let mut req = req.clone();
            req.state = RequestState::Expired {
                reason: "Veyra closed while this request waited".into(),
            };
            c.events.push(Ev::RequestClosed(req.clone()));
            c.requests.insert(req.id, req);
        }
        set_hold(p, &mut c, req.thread, Some(HoldReason::Restarted));
    }
    for t in p.threads.values().filter(|t| t.view == View::Terminal) {
        let mut t = c.thread(p, t.id).unwrap().clone();
        t.view = View::Chat;
        t.import_pending = true;
        let id = t.id;
        c.threads.insert(id, t);
        set_hold(p, &mut c, id, Some(HoldReason::Restarted));
    }
    let tasks: Vec<_> = c.tasks(p).cloned().collect();
    for mut task in tasks {
        if let TaskState::Settled {
            delivery: Delivery::Pending { wake },
            ..
        } = task.state
        {
            if c.turns(p, task.parent)
                .any(|t| t.id == wake && t.state == TurnState::Queued)
            {
                continue;
            }
            let wake = queue_wake(p, &mut c, task.parent, [task.id]);
            if let TaskState::Settled { delivery, .. } = &mut task.state {
                *delivery = Delivery::Pending { wake };
            }
            let parent = task.parent;
            c.tasks.insert(task.id, task);
            set_hold(p, &mut c, parent, Some(HoldReason::Restarted));
        }
    }
    c
}
