use crate::{
    commands::{self, CommandError},
    domain::*,
    events::{ChangeEvent as Ev, ChangeSet},
    live::LiveThread,
    provider::{Probe, ProviderKind, Readiness},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const CHILD_GUIDANCE: &str = "Your last message is your report to the parent; Veyra delivers up to 4,000 characters after its current turn ends. You cannot start child threads.\n\n";
const DESCRIPTIONS: [(&str, &str); 5] = [
    (
        "list_providers",
        "Read available providers, models, your permission ceiling and remaining child slots before delegating. A refusal explains the fix.",
    ),
    (
        "start_thread",
        "Start a child only when the user's request needs one, using a fresh client_request_id. Children inherit your mode unless you choose a narrower one and cannot delegate. A refusal explains the fix.",
    ),
    (
        "get_task",
        "Read one of your tasks when you need its status or report. Reports arrive in a new turn after your current turn ends; do not poll for them. A refusal explains the fix.",
    ),
    (
        "message_thread",
        "Send a follow-up to your own child with a fresh client_request_id. Read delivery and waits_for: queued work must not be resent. A refusal explains the fix.",
    ),
    (
        "stop_thread",
        "Stop your own child when its work is no longer needed. Its report arrives after your current turn ends. A refusal explains the fix.",
    ),
];
pub fn instructions() -> String {
    format!(
        "{}\nOnly a running root may delegate, and you may read or control only your own tasks and children. Depth is 1. At most 4 children may have open tasks and a family may have 16 threads in total, including archived threads. Children default to your mode and may never use a broader mode: ask_first < accept_edits < full_access. Prompts and follow-ups may contain at most 100,000 characters. Use a client_request_id for start_thread and message_thread; retry the same key only with the same provider, target and text. Child reports carry up to 4,000 characters and arrive as a wake after your turn ends, so do not poll. Every refusal names the rule, its limit when applicable, and the next tool and ids needed to fix it.",
        DESCRIPTIONS
            .iter()
            .map(|(name, text)| format!("{name}: {text}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}
pub fn list() -> Value {
    Value::Array(DESCRIPTIONS.iter().map(|(name,description)| {
        let (properties,required) = match *name {
            "list_providers" => (json!({}), json!([])),
            "start_thread" => (json!({"provider":{"type":"string","enum":["claude","codex","grok"]},"model":{"type":"string"},"mode":{"type":"string","enum":["ask_first","accept_edits","full_access"]},"title":{"type":"string"},"prompt":{"type":"string"},"client_request_id":{"type":"string","minLength":1}}),json!(["provider","title","prompt","client_request_id"])),
            "get_task" => (json!({"task_id":{"type":"string","format":"uuid"}}),json!(["task_id"])),
            "message_thread" => (json!({"thread_id":{"type":"string","format":"uuid"},"text":{"type":"string"},"client_request_id":{"type":"string","minLength":1}}),json!(["thread_id","text","client_request_id"])),
            "stop_thread" => (json!({"thread_id":{"type":"string","format":"uuid"}}),json!(["thread_id"])),
            _ => unreachable!(),
        };
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
    }).collect())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    provider: ProviderKind,
    model: Option<String>,
    mode: Option<PermissionMode>,
    title: String,
    prompt: String,
    client_request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    thread_id: ThreadId,
    text: String,
    client_request_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Get {
    task_id: TaskId,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stop {
    thread_id: ThreadId,
}
fn parse<T: serde::de::DeserializeOwned>(arguments: Value, tool: &str) -> Result<T, CommandError> {
    serde_json::from_value(arguments).map_err(|_| {
        invalid(format!(
            "invalid {tool} arguments; call {tool} with the fields shown by tools/list"
        ))
    })
}
fn invalid(text: String) -> CommandError {
    CommandError::Invalid(text)
}
fn prompt_cap(text: &str, tool: &str) -> Result<(), CommandError> {
    let count = text.chars().count();
    if count > 100000 {
        return Err(invalid(format!(
            "prompt has {count} characters against the 100000 character limit; shorten it and call {tool} with a new client_request_id"
        )));
    }
    Ok(())
}
fn request_key(tool: &str, id: &str) -> Result<String, CommandError> {
    if id.is_empty() {
        return Err(invalid(format!(
            "client_request_id must be nonempty; call {tool} with a new client_request_id"
        )));
    }
    Ok(format!("{tool}:{id}"))
}
fn owned_child(p: &Projection, caller: ThreadId, child: ThreadId) -> Result<&Thread, CommandError> {
    p.threads.get(&child).filter(|row| matches!(row.lineage,Lineage::Child{parent,..} if parent==caller)).ok_or_else(|| invalid(format!("thread {child} is not your own child; call get_task with one of your task ids to find its thread_id")))
}
fn prior_task<'a>(p: &'a Projection, caller: ThreadId, key: &str) -> Option<&'a Task> {
    p.tasks
        .values()
        .find(|task| task.parent == caller && task.request_key == key)
}
fn prior_text<'a>(p: &'a Projection, task: &Task) -> Option<&'a str> {
    p.items_for(task.child)
        .filter(|item| item.turn == task.turn)
        .find_map(|item| match &item.body {
            ItemBody::UserText(text) => Some(text.as_str()),
            _ => None,
        })
}
fn queue_task(
    p: &Projection,
    c: &mut ChangeSet,
    parent: ThreadId,
    child: ThreadId,
    key: String,
    text: String,
) -> Task {
    let id = TaskId::derived(parent.0, &key);
    let turn = commands::queue(p, c, child, TurnOrigin::Task { task: id });
    commands::add_item(
        p,
        c,
        Item {
            id: ItemId::new(),
            thread: child,
            turn,
            seq: 0,
            body: ItemBody::UserText(text),
        },
    );
    commands::add_item(
        p,
        c,
        Item {
            id: ItemId::new(),
            thread: child,
            turn,
            seq: 0,
            body: ItemBody::TaskCard(id),
        },
    );
    let task = Task {
        id,
        parent,
        child,
        turn,
        request_key: key,
        state: TaskState::Running,
    };
    c.events.push(Ev::TaskStarted(task.clone()));
    c.tasks.insert(id, task.clone());
    task
}
fn message_result(
    p: &Projection,
    l: &BTreeMap<ThreadId, LiveThread>,
    c: &ChangeSet,
    task: &Task,
) -> Value {
    let turn = c
        .turns
        .get(&task.turn)
        .or_else(|| p.turns.get(&task.turn))
        .unwrap();
    let running = matches!(turn.state, TurnState::Running { .. });
    let view = c.view(p, l, task.child).unwrap();
    json!({"task_id":task.id,"delivery":if running {"started"}else{"queued"},"waits_for":if running {None}else{wait_reason(&view).map(|reason|reason.to_string())}})
}
pub fn decide(
    p: &Projection,
    l: &BTreeMap<ThreadId, LiveThread>,
    caller: ThreadId,
    call: ToolCall,
    providers: &[Probe],
    now: Millis,
) -> Result<(ChangeSet, Value), CommandError> {
    let parent = p.threads.get(&caller).ok_or_else(|| {
        invalid(
            "caller thread no longer exists; call list_providers from a running root thread".into(),
        )
    })?;
    let mut c = ChangeSet::default();
    if parent.archived
        || !c
            .view(p, l, caller)
            .is_some_and(|view| view.running.is_some())
    {
        return Err(invalid("tools require a running turn; finish this call and call list_providers in your next running turn".into()));
    }
    let result = match call.name.as_str() {
        "list_providers" => {
            let args: BTreeMap<String, Value> = parse(call.arguments, "list_providers")?;
            if !args.is_empty() {
                return Err(invalid(
                    "list_providers takes no arguments; call list_providers with {}".into(),
                ));
            }
            let open: BTreeSet<_> = p
                .tasks
                .values()
                .filter(|task| task.parent == caller && task.state == TaskState::Running)
                .map(|task| task.child)
                .collect();
            json!({"providers":providers.iter().filter(|probe|probe.readiness==Readiness::Ready && probe.path.is_some()).map(|probe|json!({"provider":probe.provider,"models":probe.models})).collect::<Vec<_>>(),"mode_ceiling":parent.config.mode,"remaining_fan_out":4usize.saturating_sub(open.len())})
        }
        "start_thread" => {
            let args: Start = parse(call.arguments, "start_thread")?;
            if !matches!(parent.lineage, Lineage::Root) {
                return Err(invalid("depth limit is 1: a child cannot start threads; report to your parent, which can call get_task with its task_id".into()));
            }
            prompt_cap(&args.prompt, "start_thread")?;
            let key = request_key("start_thread", &args.client_request_id)?;
            if let Some(task) = prior_task(p, caller, &key) {
                if p.threads[&task.child].provider != args.provider
                    || prior_text(p, task) != Some(&args.prompt)
                {
                    return Err(invalid(format!(
                        "request key {key} already names a different provider or prompt; call get_task with task_id {} or start_thread with a new client_request_id",
                        task.id
                    )));
                }
                return Ok((c, json!({"task_id":task.id,"thread_id":task.child})));
            }
            let mode = args.mode.unwrap_or(parent.config.mode);
            if mode > parent.config.mode {
                return Err(invalid(format!(
                    "mode exceeds your {:?} ceiling ({}); call start_thread with a mode at or below that ceiling",
                    parent.config.mode,
                    serde_json::to_value(parent.config.mode).unwrap()
                )));
            }
            if !providers.iter().any(|probe| {
                probe.provider == args.provider
                    && probe.readiness == Readiness::Ready
                    && probe.path.is_some()
            }) {
                return Err(invalid("provider is not installed and ready; call list_providers and choose an available provider".into()));
            }
            let open: BTreeSet<_> = p
                .tasks
                .values()
                .filter(|task| task.parent == caller && task.state == TaskState::Running)
                .map(|task| task.child)
                .collect();
            if open.len() >= 4 {
                return Err(invalid(format!(
                    "this parent has {} child threads with open tasks against the limit 4; wait for a report or call stop_thread with one of these thread_id values: {}",
                    open.len(),
                    open.iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            let family = p
                .threads
                .values()
                .filter(|row| row.root() == parent.root())
                .count();
            if family >= 16 {
                return Err(invalid(format!(
                    "family has {family} threads against the limit 16, including archived threads; call message_thread with an existing child thread_id, text and a new client_request_id"
                )));
            }
            let child = Thread {
                id: ThreadId::new(),
                project: parent.project,
                title: args.title,
                lineage: Lineage::Child {
                    parent: caller,
                    relationship: Relationship::Subagent,
                    root: parent.root(),
                },
                provider: args.provider,
                config: ThreadConfig {
                    model: args.model,
                    effort: None,
                    mode,
                },
                cwd: parent.cwd.clone(),
                native: None,
                native_cursor: None,
                cli_version: None,
                hold: None,
                view: View::Chat,
                import_pending: false,
                archived: false,
            };
            let child_id = child.id;
            c.events.push(Ev::ThreadCreated(child.clone()));
            c.threads.insert(child_id, child);
            c.live.insert(child_id, LiveThread::default());
            let task = queue_task(p, &mut c, caller, child_id, key, args.prompt);
            json!({"task_id":task.id,"thread_id":child_id})
        }
        "get_task" => {
            let args: Get = parse(call.arguments, "get_task")?;
            let task=p.tasks.get(&args.task_id).filter(|task|task.parent==caller).ok_or_else(||invalid(format!("task {} is not your own task; call get_task with a task_id returned to this thread by start_thread or message_thread",args.task_id)))?;
            let view = c.view(p, l, task.child).unwrap();
            json!({"task_id":task.id,"thread_id":task.child,"status":thread_status(&view).words(),"summary":match &task.state {TaskState::Running=>None,TaskState::Settled{summary,..}=>Some(summary)}})
        }
        "message_thread" => {
            let args: Message = parse(call.arguments, "message_thread")?;
            let child = owned_child(p, caller, args.thread_id)?;
            prompt_cap(&args.text, "message_thread")?;
            let key = request_key("message_thread", &args.client_request_id)?;
            if let Some(task) = prior_task(p, caller, &key) {
                if task.child != args.thread_id || prior_text(p, task) != Some(&args.text) {
                    return Err(invalid(format!(
                        "request key {key} already names different text or a different child than requested {}; call get_task with task_id {} or message_thread with a new client_request_id",
                        args.thread_id, task.id
                    )));
                }
                return Ok((c, message_result(p, l, &ChangeSet::default(), task)));
            }
            if child.archived {
                return Err(invalid(format!(
                    "child {} is archived; call get_task with its task_id to read its report or start_thread with a new client_request_id",
                    child.id
                )));
            }
            let task = queue_task(p, &mut c, caller, child.id, key, args.text);
            commands::start_changed(p, l, &mut c, now);
            message_result(p, l, &c, &task)
        }
        "stop_thread" => {
            let args: Stop = parse(call.arguments, "stop_thread")?;
            owned_child(p, caller, args.thread_id)?;
            commands::stop_child_from_tool(p, l, &mut c, args.thread_id, now);
            json!({"thread_id":args.thread_id,"stopped":true})
        }
        _ => return Err(invalid(
            "unknown tool; call list_providers or inspect tools/list for the five supported tools"
                .into(),
        )),
    };
    commands::start_changed(p, l, &mut c, now);
    Ok((c, result))
}
pub fn task_text(p: &Projection, turn: &Turn, text: String) -> String {
    if p.turns_for(turn.thread)
        .filter(|prior| matches!(prior.origin, TurnOrigin::Task { .. }))
        .min_by_key(|prior| prior.ordinal)
        .is_some_and(|first| first.id == turn.id)
    {
        format!("{CHILD_GUIDANCE}{text}")
    } else {
        text
    }
}
