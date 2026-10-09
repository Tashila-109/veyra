use crate::{domain::*, events::ChangeSet, live::Live, provider::ProviderKind};
use rusqlite::{Connection, Transaction, params};
use serde::{Serialize, de::DeserializeOwned};
use std::{path::Path, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}")]
    Sql(#[from] rusqlite::Error),
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid stored value: {0}")]
    Invalid(String),
}
pub const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE projects (id TEXT PRIMARY KEY, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE TABLE threads (
 id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id), title TEXT NOT NULL,
 parent_thread_id TEXT, relationship TEXT, root_thread_id TEXT NOT NULL, provider TEXT NOT NULL,
 config_json TEXT NOT NULL, cwd TEXT NOT NULL, native_id TEXT, native_cursor TEXT, cli_version TEXT,
 hold_reason TEXT CHECK (hold_reason IN ('interrupted','stopped','restarted','child_report')),
 view TEXT NOT NULL, import_pending INTEGER NOT NULL, live_pid INTEGER, live_pgid INTEGER,
 created_at INTEGER NOT NULL, archived_at INTEGER,
 CHECK ((parent_thread_id IS NULL) = (relationship IS NULL)),
 CHECK ((parent_thread_id IS NULL AND root_thread_id = id) OR
        (parent_thread_id IS NOT NULL AND parent_thread_id = root_thread_id AND parent_thread_id <> id)),
 UNIQUE (id, root_thread_id),
 FOREIGN KEY (parent_thread_id, root_thread_id) REFERENCES threads(id, root_thread_id));
CREATE TABLE turns (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES threads(id), ordinal INTEGER NOT NULL,
 origin_json TEXT NOT NULL, state TEXT NOT NULL, outcome_json TEXT, queued_at INTEGER NOT NULL,
  started_at INTEGER, ended_at INTEGER,
 UNIQUE(thread_id,ordinal));
CREATE TABLE items (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES threads(id),
  turn_id TEXT NOT NULL REFERENCES turns(id),
 seq INTEGER NOT NULL, kind TEXT NOT NULL, body_json TEXT NOT NULL, done INTEGER NOT NULL, UNIQUE(thread_id,seq));
CREATE TABLE requests (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES threads(id),
  turn_id TEXT NOT NULL REFERENCES turns(id),
 kind TEXT NOT NULL, body_json TEXT NOT NULL, state TEXT NOT NULL, decision_json TEXT);
CREATE TABLE grants (thread_id TEXT NOT NULL REFERENCES threads(id), tool TEXT NOT NULL,
  command TEXT NOT NULL, created_at INTEGER NOT NULL,
 PRIMARY KEY(thread_id,tool,command));
CREATE TABLE tasks (id TEXT PRIMARY KEY, parent_thread_id TEXT NOT NULL REFERENCES threads(id),
  child_thread_id TEXT NOT NULL REFERENCES threads(id),
 turn_id TEXT NOT NULL REFERENCES turns(id), request_key TEXT NOT NULL, state TEXT NOT NULL,
  outcome_json TEXT, summary TEXT,
 delivery TEXT, wake_turn TEXT REFERENCES turns(id), UNIQUE(parent_thread_id,request_key));
CREATE TABLE settings (id INTEGER PRIMARY KEY CHECK (id = 1), default_mode TEXT NOT NULL DEFAULT 'full_access'
 CHECK (default_mode IN ('ask_first','accept_edits','full_access')));
INSERT INTO settings(id) VALUES(1);
"#];

pub fn open(path: &Path) -> Result<Connection, StoreError> {
    let mut db = Connection::open(path)?;
    initialize(&mut db)?;
    Ok(db)
}
pub fn initialize(db: &mut Connection) -> Result<(), StoreError> {
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    db.busy_timeout(Duration::from_secs(2))?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    let version: u32 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version as usize > MIGRATIONS.len() {
        return Err(StoreError::Invalid(format!(
            "schema version {version} is newer than this Veyra"
        )));
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = db.transaction()?;
        tx.execute_batch(migration)?;
        tx.pragma_update(None, "user_version", (index + 1) as u32)?;
        tx.commit()?;
    }
    db.execute(
        "UPDATE threads SET cli_version=NULL WHERE cli_version=''",
        [],
    )?;
    Ok(())
}
fn json(value: &impl Serialize) -> Result<String, StoreError> {
    Ok(serde_json::to_string(value)?)
}
fn word(value: &impl Serialize) -> Result<String, StoreError> {
    Ok(serde_json::from_str(&json(value)?)?)
}
fn parse<T: DeserializeOwned>(text: String) -> Result<T, StoreError> {
    Ok(serde_json::from_str(&text)?)
}
fn parse_word<T: DeserializeOwned>(text: String) -> Result<T, StoreError> {
    parse(json(&text)?)
}
fn id<T: DeserializeOwned>(text: String) -> Result<T, StoreError> {
    parse(json(&text)?)
}

pub fn apply(tx: &Transaction<'_>, c: &ChangeSet, now: Millis) -> Result<(), StoreError> {
    for p in c.projects.values() {
        tx.execute(
            "INSERT INTO projects(id,path,name,created_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(id) DO UPDATE SET path=excluded.path,name=excluded.name",
            params![p.id.to_string(), p.path.to_string_lossy(), p.name, now],
        )?;
    }
    let mut threads: Vec<_> = c.threads.values().collect();
    threads.sort_by_key(|t| !matches!(t.lineage, Lineage::Root));
    for t in threads {
        let (parent, rel) = match &t.lineage {
            Lineage::Root => (None, None),
            Lineage::Child {
                parent,
                relationship,
                ..
            } => (Some(parent.to_string()), Some(word(relationship)?)),
        };
        tx.execute(
            "INSERT INTO threads(id,project_id,title,parent_thread_id,relationship,root_thread_id,
             provider,config_json,cwd,native_id,native_cursor,cli_version,hold_reason,view,
             import_pending,created_at,archived_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,
             ?14,?15,?16,?17) ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,
             title=excluded.title,parent_thread_id=excluded.parent_thread_id,
             relationship=excluded.relationship, root_thread_id=excluded.root_thread_id,
             provider=excluded.provider,config_json=excluded.config_json,cwd=excluded.cwd,
             native_id=excluded.native_id, native_cursor=excluded.native_cursor,
             cli_version=excluded.cli_version,hold_reason=excluded.hold_reason,view=excluded.view,
             import_pending=excluded.import_pending,
             archived_at=CASE WHEN excluded.archived_at IS NULL THEN NULL
             ELSE coalesce(threads.archived_at,excluded.archived_at) END",
            params![
                t.id.to_string(),
                t.project.to_string(),
                t.title,
                parent,
                rel,
                t.root().to_string(),
                word(&t.provider)?,
                json(&t.config)?,
                t.cwd.to_string_lossy(),
                t.native.as_ref().map(|n| &n.0),
                t.native_cursor,
                t.cli_version.as_deref().filter(|version| !version.is_empty()),
                t.hold.as_ref().map(word).transpose()?,
                word(&t.view)?,
                t.import_pending,
                now,
                t.archived.then_some(now),
            ],
        )?;
    }
    for t in c.turns.values() {
        let (state, start, end, outcome) = match &t.state {
            TurnState::Queued => ("queued", None, None, None),
            TurnState::Running { since } => ("running", Some(*since), None, None),
            TurnState::Ended {
                outcome,
                started,
                at,
            } => ("ended", *started, Some(*at), Some(json(outcome)?)),
        };
        tx.execute(
            "INSERT INTO turns(id,thread_id,ordinal,origin_json,state,outcome_json,queued_at,
             started_at,ended_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)
             ON CONFLICT(id) DO UPDATE SET origin_json=excluded.origin_json,state=excluded.state,
             outcome_json=excluded.outcome_json,started_at=excluded.started_at,
             ended_at=excluded.ended_at",
            params![
                t.id.to_string(),
                t.thread.to_string(),
                t.ordinal,
                json(&t.origin)?,
                state,
                outcome,
                now,
                start,
                end
            ],
        )?;
    }
    for i in c.items.values() {
        let (kind, done) = match &i.body {
            ItemBody::UserText(_) => ("user_text", true),
            ItemBody::AssistantText { done, .. } => ("assistant_text", *done),
            ItemBody::Reasoning { done, .. } => ("reasoning", *done),
            ItemBody::Tool(t) => ("tool", t.result.is_some()),
            ItemBody::Plan(_) => ("plan", true),
            ItemBody::Notice(_) => ("notice", true),
            ItemBody::TaskCard(_) => ("task_card", true),
            ItemBody::TerminalSpan(_) => ("terminal_span", true),
        };
        tx.execute(
            "INSERT INTO items(id,thread_id,turn_id,seq,kind,body_json,done)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(id) DO UPDATE SET body_json=excluded.body_json,done=excluded.done",
            params![
                i.id.to_string(),
                i.thread.to_string(),
                i.turn.to_string(),
                i.seq,
                kind,
                json(&i.body)?,
                done,
            ],
        )?;
    }
    for r in c.requests.values() {
        let kind = match r.kind {
            RequestKind::Approval { .. } => "approval",
            RequestKind::Question { .. } => "question",
        };
        let (state, decision) = match &r.state {
            RequestState::Pending => ("pending", None),
            RequestState::Answered(d) => ("answered", Some(json(d)?)),
            RequestState::Expired { reason } => ("expired", Some(json(reason)?)),
        };
        tx.execute(
            "INSERT INTO requests(id,thread_id,turn_id,kind,body_json,state,decision_json)
             VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET state=excluded.state,
             decision_json=excluded.decision_json",
            params![
                r.id.to_string(),
                r.thread.to_string(),
                r.turn.to_string(),
                kind,
                json(&(&r.native, &r.kind))?,
                state,
                decision,
            ],
        )?;
    }
    for t in c.tasks.values() {
        let (state, outcome, summary, delivery, wake) = match &t.state {
            TaskState::Running => ("running", None, None, None, None),
            TaskState::Settled {
                outcome,
                summary,
                delivery,
            } => {
                let (name, wake) = match delivery {
                    Delivery::Pending { wake } => ("pending", wake),
                    Delivery::Delivered { wake } => ("delivered", wake),
                };
                (
                    "settled",
                    Some(json(outcome)?),
                    Some(summary.as_str()),
                    Some(name),
                    Some(wake.to_string()),
                )
            }
        };
        tx.execute(
            "INSERT INTO tasks(id,parent_thread_id,child_thread_id,turn_id,request_key,state,
             outcome_json,summary,delivery,wake_turn) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
             ON CONFLICT(id) DO UPDATE SET state=excluded.state,outcome_json=excluded.outcome_json,
             summary=excluded.summary,delivery=excluded.delivery,wake_turn=excluded.wake_turn",
            params![
                t.id.to_string(),
                t.parent.to_string(),
                t.child.to_string(),
                t.turn.to_string(),
                t.request_key,
                state,
                outcome,
                summary,
                delivery,
                wake,
            ],
        )?;
    }
    for (thread, key) in &c.grants {
        tx.execute(
            "INSERT OR IGNORE INTO grants(thread_id,tool,command,created_at) VALUES(?1,?2,?3,?4)",
            params![thread.to_string(), key.tool, key.command, now],
        )?;
    }
    if let Some(settings) = c.settings {
        tx.execute(
            "UPDATE settings SET default_mode=?1 WHERE id=1",
            params![word(&settings.default_mode)?],
        )?;
    }
    for (id, live) in &c.live {
        if matches!(live.live, Live::Stopping { .. }) {
            continue;
        }
        let (pid, pgid) = match live.live {
            Live::Structured { pid, pgid, .. } => (Some(pid), Some(pgid)),
            Live::Terminal { pid, .. } => (Some(pid), None),
            _ => (None, None),
        };
        tx.execute(
            "UPDATE threads SET live_pid=?1,live_pgid=?2 WHERE id=?3",
            params![pid, pgid, id.to_string()],
        )?;
    }
    Ok(())
}

pub fn load(db: &Connection) -> Result<Projection, StoreError> {
    let mut p = Projection::default();
    p.settings.default_mode = parse_word(db.query_row(
        "SELECT default_mode FROM settings WHERE id=1",
        [],
        |r| r.get(0),
    )?)?;
    let mut q = db.prepare("SELECT id,path,name FROM projects")?;
    for row in q.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (i, path, name) = row?;
        let project = Project {
            id: id(i)?,
            path: path.into(),
            name,
        };
        p.projects.insert(project.id, project);
    }
    let mut q = db.prepare(
        "SELECT id,project_id,title,parent_thread_id,relationship,root_thread_id,provider,
             config_json,cwd,native_id,native_cursor,cli_version,hold_reason,view,import_pending,
             archived_at FROM threads",
    )?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let parent: Option<String> = r.get(3)?;
        let t = Thread {
            id: id(r.get(0)?)?,
            project: id(r.get(1)?)?,
            title: r.get(2)?,
            lineage: match parent {
                None => Lineage::Root,
                Some(parent) => Lineage::Child {
                    parent: id(parent)?,
                    relationship: parse_word(r.get(4)?)?,
                    root: id(r.get(5)?)?,
                },
            },
            provider: parse_word::<ProviderKind>(r.get(6)?)?,
            config: parse(r.get(7)?)?,
            cwd: Path::new(&r.get::<_, String>(8)?).to_owned(),
            native: r.get::<_, Option<String>>(9)?.map(NativeSessionId),
            native_cursor: r.get(10)?,
            cli_version: r
                .get::<_, Option<String>>(11)?
                .filter(|version| !version.is_empty()),
            hold: r
                .get::<_, Option<String>>(12)?
                .map(parse_word)
                .transpose()?,
            view: parse_word(r.get(13)?)?,
            import_pending: r.get(14)?,
            archived: r.get::<_, Option<Millis>>(15)?.is_some(),
        };
        p.threads.insert(t.id, t);
    }
    let mut q = db.prepare(
        "SELECT id,thread_id,ordinal,origin_json,state,outcome_json,started_at,ended_at FROM turns",
    )?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let state: String = r.get(4)?;
        let state = match state.as_str() {
            "queued" => TurnState::Queued,
            "running" => TurnState::Running { since: r.get(6)? },
            "ended" => TurnState::Ended {
                outcome: parse(r.get(5)?)?,
                started: r.get(6)?,
                at: r.get(7)?,
            },
            _ => return Err(StoreError::Invalid(state)),
        };
        let t = Turn {
            id: id(r.get(0)?)?,
            thread: id(r.get(1)?)?,
            ordinal: r.get(2)?,
            origin: parse(r.get(3)?)?,
            state,
        };
        p.insert_turn(t);
    }
    let mut q = db.prepare("SELECT id,thread_id,turn_id,seq,body_json FROM items")?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let i = Item {
            id: id(r.get(0)?)?,
            thread: id(r.get(1)?)?,
            turn: id(r.get(2)?)?,
            seq: r.get(3)?,
            body: parse(r.get(4)?)?,
        };
        p.insert_item(i);
    }
    let mut q =
        db.prepare("SELECT id,thread_id,turn_id,body_json,state,decision_json FROM requests")?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let (native, kind) = parse(r.get(3)?)?;
        let state: String = r.get(4)?;
        let state = match state.as_str() {
            "pending" => RequestState::Pending,
            "answered" => RequestState::Answered(parse(r.get(5)?)?),
            "expired" => RequestState::Expired {
                reason: parse(r.get(5)?)?,
            },
            _ => return Err(StoreError::Invalid(state)),
        };
        let req = Request {
            id: id(r.get(0)?)?,
            thread: id(r.get(1)?)?,
            turn: id(r.get(2)?)?,
            native,
            kind,
            state,
        };
        p.requests.insert(req.id, req);
    }
    let mut q = db.prepare(
        "SELECT id,parent_thread_id,child_thread_id,turn_id,request_key,state,outcome_json,summary,
             delivery,wake_turn FROM tasks",
    )?;
    let mut rows = q.query([])?;
    while let Some(r) = rows.next()? {
        let state: String = r.get(5)?;
        let state = match state.as_str() {
            "running" => TaskState::Running,
            "settled" => {
                let wake = id(r.get(9)?)?;
                let d: String = r.get(8)?;
                let delivery = match d.as_str() {
                    "pending" => Delivery::Pending { wake },
                    "delivered" => Delivery::Delivered { wake },
                    _ => return Err(StoreError::Invalid(d)),
                };
                TaskState::Settled {
                    outcome: parse(r.get(6)?)?,
                    summary: r.get(7)?,
                    delivery,
                }
            }
            _ => return Err(StoreError::Invalid(state)),
        };
        let t = Task {
            id: id(r.get(0)?)?,
            parent: id(r.get(1)?)?,
            child: id(r.get(2)?)?,
            turn: id(r.get(3)?)?,
            request_key: r.get(4)?,
            state,
        };
        p.tasks.insert(t.id, t);
    }
    Ok(p)
}
