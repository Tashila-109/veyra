use crate::{
    commands::{end_turn, queue},
    domain::*,
    events::{ChangeEvent as Ev, ChangeSet},
};

pub fn decide(p: &Projection, now: Millis) -> ChangeSet {
    let mut c = ChangeSet::default();
    for t in p.turns.values() {
        if matches!(t.state, TurnState::Queued | TurnState::Running { .. }) {
            let mut thread = p.threads[&t.thread].clone();
            thread.hold = Some(thread.hold.unwrap_or(HoldReason::Restarted));
            c.threads.insert(thread.id, thread);
        }
        if matches!(t.state, TurnState::Running { .. }) {
            end_turn(
                p,
                &mut c,
                t.clone(),
                TurnOutcome::Interrupted {
                    reason: "Veyra closed while this turn ran".into(),
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
        let mut req = req.clone();
        req.state = RequestState::Expired {
            reason: "Veyra closed while this request waited".into(),
        };
        c.events.push(Ev::RequestClosed(req.clone()));
        c.requests.insert(req.id, req.clone());
        let mut thread = c.thread(p, req.thread).unwrap().clone();
        thread.hold = Some(thread.hold.unwrap_or(HoldReason::Restarted));
        c.threads.insert(thread.id, thread);
    }
    for t in p.threads.values().filter(|t| t.view == View::Terminal) {
        let mut t = c.thread(p, t.id).unwrap().clone();
        t.view = View::Chat;
        t.import_pending = true;
        t.hold = Some(t.hold.unwrap_or(HoldReason::Restarted));
        c.threads.insert(t.id, t);
    }
    for task in p.tasks.values() {
        if let TaskState::Settled {
            delivery: Delivery::Pending { wake },
            ..
        } = task.state
        {
            if p.turns
                .get(&wake)
                .is_some_and(|t| t.state == TurnState::Queued)
            {
                continue;
            }
            let existing = c
                .turns(p, task.parent)
                .find(|t| {
                    t.state == TurnState::Queued && matches!(t.origin, TurnOrigin::Wake { .. })
                })
                .cloned();
            let new_wake = match existing {
                Some(mut turn) => {
                    if let TurnOrigin::Wake { tasks } = &mut turn.origin
                        && !tasks.contains(&task.id)
                    {
                        tasks.push(task.id);
                    }
                    let id = turn.id;
                    c.turns.insert(id, turn);
                    id
                }
                None => queue(
                    p,
                    &mut c,
                    task.parent,
                    TurnOrigin::Wake {
                        tasks: vec![task.id],
                    },
                ),
            };
            let mut task = task.clone();
            if let TaskState::Settled { delivery, .. } = &mut task.state {
                *delivery = Delivery::Pending { wake: new_wake };
            }
            c.tasks.insert(task.id, task.clone());
            let mut thread = c.thread(p, task.parent).unwrap().clone();
            thread.hold = Some(thread.hold.unwrap_or(HoldReason::Restarted));
            c.threads.insert(thread.id, thread);
        }
    }
    c
}
