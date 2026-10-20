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

pub const OPEN_CHILD_LIMIT: usize = 4;
pub const FAMILY_LIMIT: usize = 16;
pub const PROMPT_CHARS: usize = 100_000;
pub const REPORT_CHARS: usize = 4_000;
fn grouped(value: usize) -> String {
    let digits = value.to_string();
    digits
        .chars()
        .enumerate()
        .map(|(index, ch)| {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                format!(",{ch}")
            } else {
                ch.to_string()
            }
        })
        .collect()
}
pub static CHILD_GUIDANCE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "Your last message is your report to the parent; Veyra delivers up to {} characters after its current turn ends. You cannot start child threads.\n\n",
        grouped(REPORT_CHARS)
    )
});
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    ListProviders,
    StartThread,
    GetTask,
    MessageThread,
    StopThread,
}
impl Tool {
    pub const ALL: [Self; 5] = [
        Self::ListProviders,
        Self::StartThread,
        Self::GetTask,
        Self::MessageThread,
        Self::StopThread,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Self::ListProviders => "list_providers",
            Self::StartThread => "start_thread",
            Self::GetTask => "get_task",
            Self::MessageThread => "message_thread",
            Self::StopThread => "stop_thread",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.name() == name)
    }
    fn base_description(self) -> &'static str {
        match self {
            Self::ListProviders => {
                "Read available providers, models, your permission ceiling and remaining child slots before delegating. A refusal explains the fix."
            }
            Self::StartThread => {
                "Start a child only when the user's request needs one, using a fresh client_request_id. Children inherit your mode unless you choose a narrower one and cannot delegate. A refusal explains the fix."
            }
            Self::GetTask => {
                "Read one of your tasks when you need its status or report. Reports arrive in a new turn after your current turn ends; do not poll for them. A refusal explains the fix."
            }
            Self::MessageThread => {
                "Send a follow-up to your own child with a fresh client_request_id. Read delivery and waits_for: queued work must not be resent. A refusal explains the fix."
            }
            Self::StopThread => {
                "Stop your own child when its work is no longer needed. Its report arrives after your current turn ends. A refusal explains the fix."
            }
        }
    }
    fn description(self) -> String {
        match self {
            Self::StartThread => format!("{} {}", self.base_description(), first_call_rules()),
            _ => format!(
                "{} Family rules are in start_thread's description.",
                self.base_description()
            ),
        }
    }
    fn schema(self) -> Value {
        let (properties, required) = match self {
            Self::ListProviders => (json!({}), json!([])),
            Self::StartThread => (
                json!({"provider":{"type":"string","enum":ProviderKind::ALL},"model":{"type":"string"},"mode":{"type":"string","enum":["ask_first","accept_edits","full_access"]},"title":{"type":"string"},"prompt":{"type":"string"},"client_request_id":{"type":"string","minLength":1}}),
                json!(["provider", "title", "prompt", "client_request_id"]),
            ),
            Self::GetTask => (
                json!({"task_id":{"type":"string","format":"uuid"}}),
                json!(["task_id"]),
            ),
            Self::MessageThread => (
                json!({"thread_id":{"type":"string","format":"uuid"},"text":{"type":"string"},"client_request_id":{"type":"string","minLength":1}}),
                json!(["thread_id", "text", "client_request_id"]),
            ),
            Self::StopThread => (
                json!({"thread_id":{"type":"string","format":"uuid"}}),
                json!(["thread_id"]),
            ),
        };
        json!({"name":self.name(),"description":self.description(),"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
    }
}
fn first_call_rules() -> String {
    format!(
        "Only a running root may delegate, and you may read or control only your own tasks and children. Depth is 1. At most {} children may have open tasks and a family may have {} threads in total, including archived threads. Children default to your mode and may never use a broader mode: ask_first < accept_edits < full_access. Prompts and follow-ups may contain at most {} characters. Use a client_request_id for start_thread and message_thread; retry the same key only with the same provider, target and text. Child reports carry up to {} characters and arrive as a wake after your turn ends, so do not poll. Every refusal names the rule, its limit when applicable, and the next tool and ids needed to fix it.",
        OPEN_CHILD_LIMIT,
        FAMILY_LIMIT,
        grouped(PROMPT_CHARS),
        grouped(REPORT_CHARS)
    )
}
pub fn instructions() -> String {
    format!(
        "{}\n{}",
        Tool::ALL
            .into_iter()
            .map(|tool| format!("{}: {}", tool.name(), tool.base_description()))
            .collect::<Vec<_>>()
            .join("\n"),
        first_call_rules()
    )
}

pub fn list() -> Value {
    Value::Array(Tool::ALL.into_iter().map(Tool::schema).collect())
}
pub(crate) fn storage_recovery(call: &ToolCall) -> String {
    let (name, args) = Tool::from_name(&call.name)
        .map(|tool| (tool.name(), call.arguments.clone()))
        .unwrap_or((Tool::ListProviders.name(), json!({})));
    format!(
        "When storage recovers and this root has a running turn, retry {name} with {args}, keeping the same client_request_id for a task request"
    )
}
fn open_children(p: &Projection, parent: ThreadId) -> BTreeSet<ThreadId> {
    p.tasks
        .values()
        .filter(|task| task.parent == parent && task.state == TaskState::Running)
        .map(|task| task.child)
        .collect()
}
fn check_child_slot(
    p: &Projection,
    parent: ThreadId,
    child: Option<ThreadId>,
) -> Result<(), CommandError> {
    let open = open_children(p, parent);
    if child.is_some_and(|child| open.contains(&child)) || open.len() < OPEN_CHILD_LIMIT {
        return Ok(());
    }
    Err(invalid(format!(
        "this parent has {} child threads with open tasks against the limit {}; wait for a report or call stop_thread with one of these thread_id values: {}",
        open.len(),
        OPEN_CHILD_LIMIT,
        open.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    )))
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
fn parse<T: serde::de::DeserializeOwned>(arguments: Value, tool: Tool) -> Result<T, CommandError> {
    let tool = tool.name();
    serde_json::from_value(arguments).map_err(|_| {
        invalid(format!(
            "invalid {tool} arguments; call {tool} with the fields shown by tools/list"
        ))
    })
}
fn invalid(text: String) -> CommandError {
    CommandError::Invalid(text)
}
fn prompt_cap(text: &str, tool: Tool) -> Result<(), CommandError> {
    let tool = tool.name();
    let count = text.chars().count();
    if count > PROMPT_CHARS {
        return Err(invalid(format!(
            "prompt has {count} characters against the {PROMPT_CHARS} character limit; shorten it and call {tool} with a new client_request_id"
        )));
    }
    Ok(())
}
fn request_key(tool: Tool, id: &str) -> Result<String, CommandError> {
    let tool = tool.name();
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
fn prior_text<'a>(p: &'a Projection, task: &Task, tool: Tool) -> Option<&'a str> {
    p.items_for(task.child)
        .filter(|item| item.turn == task.turn)
        .find_map(|item| match &item.body {
            ItemBody::UserText(text) => Some(if tool == Tool::StartThread {
                text.strip_prefix(CHILD_GUIDANCE.as_str()).unwrap_or(text)
            } else {
                text.as_str()
            }),
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
    let parent_mode = parent.provider.run_mode(parent.config.mode);
    if parent.archived
        || !c
            .view(p, l, caller)
            .is_some_and(|view| view.running.is_some())
    {
        return Err(invalid("tools require a running turn; finish this call and call list_providers in your next running turn".into()));
    }
    let tool=Tool::from_name(&call.name).ok_or_else(||invalid("unknown tool; call list_providers with {} or inspect tools/list for the supported tools".into()))?;
    let result = match tool {
        Tool::ListProviders => {
            let args: BTreeMap<String, Value> = parse(call.arguments, tool)?;
            if !args.is_empty() {
                return Err(invalid(
                    "list_providers takes no arguments; call list_providers with {}".into(),
                ));
            }
            let open = open_children(p, caller);
            json!({"providers":providers.iter().filter(|probe|probe.readiness==Readiness::Ready && probe.path.is_some()).map(|probe|json!({"provider":probe.provider,"models":probe.models,"offered_modes":probe.provider.offered_modes()})).collect::<Vec<_>>(),"mode_ceiling":parent_mode,"remaining_fan_out":OPEN_CHILD_LIMIT.saturating_sub(open.len())})
        }
        Tool::StartThread => {
            let args: Start = parse(call.arguments, tool)?;
            if !matches!(parent.lineage, Lineage::Root) {
                return Err(invalid("depth limit is 1: a child cannot start threads; report to your parent, which can call get_task with its task_id".into()));
            }
            prompt_cap(&args.prompt, tool)?;
            let key = request_key(tool, &args.client_request_id)?;
            if let Some(task) = prior_task(p, caller, &key) {
                if p.threads[&task.child].provider != args.provider
                    || prior_text(p, task, Tool::StartThread) != Some(&args.prompt)
                {
                    return Err(invalid(format!(
                        "request key {key} already names a different provider or prompt; call get_task with task_id {} or start_thread with a new client_request_id",
                        task.id
                    )));
                }
                return Ok((c, json!({"task_id":task.id,"thread_id":task.child})));
            }
            let mode = args.mode.unwrap_or(parent_mode);
            if mode > parent_mode {
                let ceiling = serde_json::to_value(parent_mode).unwrap();
                return Err(invalid(format!(
                    "mode exceeds your {} ceiling; call start_thread with a mode at or below that ceiling",
                    ceiling.as_str().unwrap()
                )));
            }
            if !providers.iter().any(|probe| {
                probe.provider == args.provider
                    && probe.readiness == Readiness::Ready
                    && probe.path.is_some()
            }) {
                return Err(invalid("provider is not installed and ready; call list_providers with {} and choose an available provider".into()));
            }
            check_child_slot(p, caller, None)?;
            let family = p
                .threads
                .values()
                .filter(|row| row.root() == parent.root())
                .count();
            if family >= FAMILY_LIMIT {
                return Err(invalid(format!(
                    "family has {family} threads against the limit {FAMILY_LIMIT}, including archived threads; call message_thread with an existing child thread_id, text and a new client_request_id"
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
            c.events.push(Ev::ThreadCreated(Box::new(child.clone())));
            c.threads.insert(child_id, child);
            c.live.insert(child_id, LiveThread::default());
            let task = queue_task(
                p,
                &mut c,
                caller,
                child_id,
                key,
                format!("{}{}", CHILD_GUIDANCE.as_str(), args.prompt),
            );
            json!({"task_id":task.id,"thread_id":child_id})
        }
        Tool::GetTask => {
            let args: Get = parse(call.arguments, tool)?;
            let task=p.tasks.get(&args.task_id).filter(|task|task.parent==caller).ok_or_else(||invalid(format!("task {} is not your own task; call get_task with a task_id returned to this thread by start_thread or message_thread",args.task_id)))?;
            let view = c.view(p, l, task.child).unwrap();
            json!({"task_id":task.id,"thread_id":task.child,"status":thread_status(&view).words(),"summary":match &task.state {TaskState::Running=>None,TaskState::Settled{summary,..}=>Some(summary)}})
        }
        Tool::MessageThread => {
            let args: Message = parse(call.arguments, tool)?;
            let child = owned_child(p, caller, args.thread_id)?;
            prompt_cap(&args.text, tool)?;
            let key = request_key(tool, &args.client_request_id)?;
            if let Some(task) = prior_task(p, caller, &key) {
                if task.child != args.thread_id
                    || prior_text(p, task, Tool::MessageThread) != Some(&args.text)
                {
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
            check_child_slot(p, caller, Some(child.id))?;
            let task = queue_task(p, &mut c, caller, child.id, key, args.text);
            commands::start_changed(p, l, &mut c, now);
            message_result(p, l, &c, &task)
        }
        Tool::StopThread => {
            let args: Stop = parse(call.arguments, tool)?;
            owned_child(p, caller, args.thread_id)?;
            commands::stop_child_from_tool(p, l, &mut c, args.thread_id, now);
            json!({"thread_id":args.thread_id,"stopped":true})
        }
    };
    commands::start_changed(p, l, &mut c, now);
    Ok((c, result))
}

#[cfg(test)]
mod description_tests {
    use super::*;
    #[test]
    fn start_thread_description_teaches_the_first_call_rules() {
        let tools = list();
        for tool in tools.as_array().unwrap() {
            let text = tool["description"].as_str().unwrap();
            if tool["name"] != "start_thread" {
                assert!(text.contains("Family rules are in start_thread's description."));
                continue;
            }
            for rule in [
                "Only a running root",
                "Depth is 1",
                "4 children",
                "16 threads",
                "ask_first < accept_edits < full_access",
                "100,000 characters",
                "4,000 characters",
                "wake after your turn ends",
                "do not poll",
                "client_request_id",
                "own tasks and children",
            ] {
                assert!(text.contains(rule), "{} omits {rule}", tool["name"]);
            }
        }
    }
}

#[cfg(test)]
mod description_size_tests {
    use super::*;
    #[test]
    fn family_rules_have_one_description_carrier() {
        let listed = list();
        let tools = listed.as_array().unwrap();
        let carriers: Vec<_> = tools
            .iter()
            .filter(|tool| tool["description"].as_str().unwrap().contains("Depth is 1"))
            .collect();
        assert_eq!(carriers.len(), 1);
        assert_eq!(carriers[0]["name"], "start_thread");
        assert!(
            tools
                .iter()
                .map(|tool| tool["description"].as_str().unwrap().chars().count())
                .sum::<usize>()
                < 3_000
        );
        for tool in tools.iter().filter(|tool| tool["name"] != "start_thread") {
            assert!(
                tool["description"]
                    .as_str()
                    .unwrap()
                    .contains("start_thread")
            );
        }
    }
}
