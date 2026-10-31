use gpui_kit::SystemNotification;
use veyra_engine::domain::{Status, ThreadId};

pub(super) fn priority(status: &Status) -> u8 {
    match status {
        Status::NeedsYou => 0,
        Status::Failed => 1,
        Status::Held => 2,
        Status::Interrupted => 3,
        Status::Working { .. } => 4,

        Status::Done { .. } => 6,
        Status::Idle => 7,
    }
}
pub fn banner_for(
    active: bool,
    previous: &Status,
    current: &Status,
    root: ThreadId,
    title: &str,
) -> Option<SystemNotification> {
    if active {
        return None;
    }
    let body = attention_status(previous, current)?;
    Some(SystemNotification {
        tag: root.to_string().into(),
        title: title.to_owned().into(),
        body: body.into(),
        actions: vec![],
    })
}

pub(super) fn attention_status(previous: &Status, current: &Status) -> Option<&'static str> {
    if std::mem::discriminant(previous) == std::mem::discriminant(current) {
        return None;
    }
    match current {
        Status::NeedsYou => Some("Needs you"),
        Status::Done { .. } => Some("Done"),
        Status::Failed => Some("Failed"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn banner_for_only_when_window_inactive() {
        let root = ThreadId::new();
        assert!(banner_for(true, &Status::Idle, &Status::NeedsYou, root, "Work").is_none());
        let banner = banner_for(false, &Status::Idle, &Status::NeedsYou, root, "Work").unwrap();
        assert_eq!(banner.title.as_ref(), "Work");
        assert_eq!(banner.body.as_ref(), "Needs you");
        assert!(banner_for(false, &Status::NeedsYou, &Status::NeedsYou, root, "Work").is_none());
    }
    #[test]
    fn banner_for_tags_family_root() {
        let root = ThreadId::new();
        let banner = banner_for(
            false,
            &Status::Working { since: 1 },
            &Status::Failed,
            root,
            "Root",
        )
        .unwrap();
        assert_eq!(banner.tag.as_ref(), root.to_string());
        assert_eq!(banner.body.as_ref(), "Failed");
    }
    #[test]
    fn banner_for_waits_for_background_work() {
        use veyra_engine::{domain::*, live::LiveThread};
        let root = ThreadId::new();
        let thread = Thread {
            id: root,
            project: ProjectId::new(),
            title: "Root".into(),
            lineage: Lineage::Root,
            provider: veyra_engine::provider::ProviderKind::Claude,
            config: ThreadConfig {
                model: None,
                effort: None,
                mode: PermissionMode::AskFirst,
            },
            cwd: "/tmp".into(),
            native: None,
            cli_version: None,
            hold: None,
            archived: false,
        };
        let turn = Turn {
            id: TurnId::new(),
            thread: root,
            ordinal: 1,
            origin: TurnOrigin::Provider,
            state: TurnState::Ended {
                outcome: TurnOutcome::Completed,
                started: Some(0),
                at: 42_000,
            },
        };
        let live = LiveThread {
            background_work: true,
            ..Default::default()
        };
        let view = |live| ThreadView {
            thread: &thread,
            live,
            running: None,
            queued: false,
            pending_request: false,
            last: Some(&turn),
        };
        let working = Status::Working { since: 1 };
        let background = family_status(&view(&live), &[]);
        assert_eq!(background, Status::Working { since: 0 });
        assert!(banner_for(false, &working, &background, root, "Root").is_none());
        let completed_live = LiveThread {
            background_work: false,
            ..live.clone()
        };
        let done = family_status(&view(&completed_live), &[]);
        let banner = banner_for(false, &background, &done, root, "Root").unwrap();
        assert_eq!(banner.body.as_ref(), "Done");
    }
}
