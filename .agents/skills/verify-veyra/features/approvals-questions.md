# Approvals and questions

Status: passed, 2026-10-14, 192cffa, none

## Sub-features

- Typed commands, file operations and other tools, with absolute file targets and bounded argument previews.
- Exact command, Deny, AllowOnce and AllowInThread, including a compound command.
- AnswerQuestion choices, FocusDock and FocusComposer, digit focus isolation and two pending requests.
- NextNeedsYou selects the request's thread across the sidebar.
- The request closes as Answered and its answer appears in the transcript.

## How to get to it (user POV)

In Ask first, ask for `touch veyra-check.txt`.
The request docks above the input, with the command and its consequence.
Deny is ⌘., Allow once is ⌘↵ and Allow in this thread is ⌘⌥↵ while the dock has focus.
The dock shows its answer chords only while focused; otherwise it shows ⌘⇧Y to FocusDock.
Requests do not take focus automatically.
Composer ⌘↵ sends a nonempty draft and leaves the request pending; an empty draft during a turn dispatches Stop.
Every button's shortcut_chip reads the same bindings table as the keymap.
Buttons answer their displayed request; an expired request or a question index that already advanced sends no answer.
A question shows its options; click Focus to use shortcuts or press ⌘⇧Y to FocusDock.
Keys 1 to 9 choose only while the dock has focus; Escape runs FocusComposer.
More than one pending request shows 1 of N.
Answering one keeps dock focus while another request remains; the last answer returns focus to the composer.
NextNeedsYou uses ⌘⇧J pending the operator's chord decision.

## Driving it with veyra ctl

Run Launch, Doctor and new-thread first, using the fake lane.
All added wires below are proof copies of Veyra's own fixtures.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
assert LANE == 'fake'
check = DATA/'project'/'veyra-check.txt'
check.unlink(missing_ok=True)
action('approval-mode','SetDefaultMode',{'mode':'ask_first'})
thread = create('approval-deny','Run touch veyra-check.txt',scenario='approval-deny')
dock = poll('approval-deny-dock',lambda d: d['composer']['dock'] is not None)
assert dock['composer']['dock']['request']['kind']['Approval']['action']['target']['command'] == 'touch veyra-check.txt'
assert dock['selected_thread']['status'] == 'NeedsYou'
capture('approval-deny')
action('approval-deny-focus','FocusDock')
poll('approval-deny-focused',lambda d: d['composer']['dock']['focused'])
capture('approval-deny-focused')
ctl('approval-deny-key','keys','secondary-.')
ctl('approval-deny-done','wait',thread,'Done')
assert not check.exists()
records = [json.loads(line) for line in (SOURCE/'approval-allow.jsonl').read_text().splitlines()]
reply = [json.loads(line) for line in (SOURCE/'ok-reply.jsonl').read_text().splitlines()][3:]
reply = json.loads(json.dumps(reply).replace('<id:prompt>','<id:queued-prompt>'))
stage_records(records[:-1]+reply)
thread = create('approval-allow','Run touch veyra-check.txt',scenario=None)
poll('approval-allow-dock',lambda d: d['composer']['dock'] is not None)
capture('approval-allow')
ctl('approval-composer-draft','text','reply with the word ok')
ctl('approval-composer-deny','keys','secondary-.')
assert sql("select state from requests where thread_id='"+thread+"'") == 'pending'
ctl('approval-composer-send','keys','secondary-enter')
pending = poll('approval-composer-pending',lambda d: d['composer_text']=='')
assert pending['composer']['dock'] is not None and not pending['composer']['dock']['focused']
assert sql("select state from requests where thread_id='"+thread+"'") == 'pending'
assert sql("select count(*) from turns where thread_id='"+thread+"' and state='queued'") == '1'
action('approval-allow-focus','FocusDock')
poll('approval-allow-focused',lambda d: d['composer']['dock']['focused'])
capture('approval-allow-focused')
ctl('approval-allow-key','keys','secondary-enter')
poll('approval-allow-done',lambda d: d['selected_thread']['status']=='Done'
    and sql("select count(*) from turns where thread_id='"+thread+"' and state='ended'") == '2')
assert check.is_file()
thread = create('approval-grant','Run touch veyra-grant.txt',scenario='approval-grant')
grant = poll('approval-grant-dock',lambda d: d['composer']['dock'] is not None)
capture('approval-grant')
action('approval-grant-answer','AllowInThread',{'request':grant['composer']['dock']['request']['id']})
compound = poll('approval-compound',lambda d: d['composer']['dock'] is not None
    and '&&' in d['composer']['dock']['request']['kind']['Approval']['action']['target']['command'])
assert sql("select count(*) from requests where thread_id='"+thread+"'") == '3'
assert sql("select count(*) from grants where thread_id='"+thread+"'") == '1'
assert (DATA/'project'/'veyra-grant.txt').is_file()
capture('approval-compound')
action('approval-compound-deny','Deny',{'request':compound['composer']['dock']['request']['id']})
ctl('approval-grant-done','wait',thread,'Done')
for place, target in [('inside', str((DATA/'project'/'veyra-check.txt').resolve())),
    ('outside', '/tmp/veyra-outside-'+RUN+'.txt')]:
    records = [json.loads(line) for line in (SOURCE/'approval-allow.jsonl').read_text().splitlines()]
    records[4]['line']['request'].update(tool_name='Write', input={'file_path':target,'content':'veyra check'})
    stage_records(records)
    thread = create('approval-write-'+place,'Write the target file',scenario=None)
    dump = poll('approval-write-'+place,lambda d: d['composer']['dock'] is not None)
    dock = dump['composer']['dock']
    assert dock['action_text'] == 'Write '+target
    assert dock['consequence'] == 'Claude Code writes a file '+place+' the project folder.'
    assert dock['request']['kind']['Approval']['grant_key'] is None
    assert json.loads(dock['preview']) == {'content':'veyra check'}
    capture('approval-write-'+place)
    action('approval-write-'+place+'-focus','FocusDock')
    ctl('approval-write-'+place+'-allow','keys','secondary-enter')
    done = ctl('approval-write-'+place+'-done','wait',thread,'Done')
    assert done['composer']['dock'] is None
    ctl('approval-write-'+place+'-composer','text','composer has focus')
    assert ctl('approval-write-'+place+'-draft','dump')['composer_text'] == 'composer has focus'
for label, tool, arguments in [
    ('web-fetch', 'WebFetch', {'url':'https://example.test/page','prompt':'Read the page'}),
    ('notebook-edit', 'NotebookEdit', {'notebook_path':str(DATA/'project'/'check.ipynb'),
        'new_source':'print("veyra check")','cell_id':'cell-a','edit_mode':'replace'}),
    ('mcp', 'mcp__local__change', {'target':'record-a','changes':{'first':1,'second':'every argument'},'flag':True}),
]:
    records = [json.loads(line) for line in (SOURCE/'approval-allow.jsonl').read_text().splitlines()]
    records[4]['line']['request'].update(tool_name=tool,input=arguments)
    stage_records(records)
    thread = create('approval-'+label,'Approve the tool arguments',scenario=None)
    dump = poll('approval-'+label,lambda d: d['composer']['dock'] is not None)
    dock = dump['composer']['dock']
    preview = json.loads(dock['preview'])
    if label == 'notebook-edit':
        assert dock['action_text'] == tool+' '+arguments['notebook_path']
        assert dock['consequence'] == 'Claude Code edits a file inside the project folder.'
        preview['notebook_path'] = arguments['notebook_path']
    else:
        assert dock['action_text'] == tool
        assert dock['consequence'] == 'Claude Code uses this tool with the arguments below.'
    assert preview == arguments
    assert dock['request']['kind']['Approval']['grant_key'] is None
    capture('approval-'+label)
    action('approval-'+label+'-allow','AllowOnce',{'request':dock['request']['id']})
    ctl('approval-'+label+'-done','wait',thread,'Done')
command = "cat <<'VEYRA_END'\n" + 'long approval heredoc line\n'*200 + 'VEYRA_END'
records = [json.loads(line) for line in (SOURCE/'approval-allow.jsonl').read_text().splitlines()]
records[4]['line']['request'].update(tool_name='Bash',input={'command':command})
stage_records(records)
thread = create('approval-long-command','Approve a long heredoc',scenario=None)
dump = poll('approval-long-command-dock',lambda d: d['composer']['dock'] is not None)
assert dump['composer']['dock']['action_text'] == command
assert dump['composer']['dock']['request']['kind']['Approval']['grant_key']['command'] == command
action('approval-long-command-focus','FocusDock')
poll('approval-long-command-focused',lambda d: d['composer']['dock']['focused'])
capture('approval-long-command')
ctl('approval-long-command-allow','keys','secondary-enter')
ctl('approval-long-command-done','wait',thread,'Done')
question = create('question','Choose a route',scenario='question')
poll('question-dock',lambda d: d['composer']['dock'] is not None)
capture('question')
ctl('question-composer-digit','keys','1')
assert ctl('question-still-pending','dump')['composer']['dock'] is not None
other = create('question-other','reply with the word ok',slow=True)
ctl('question-other-done','wait',other,'Done')
action('question-next','NextNeedsYou')
selected = poll('question-selected',lambda d: d['selected_thread']['id']==question)
assert selected['composer']['dock'] is not None
ctl('question-focus-key','keys','secondary-shift-y')
poll('question-focused',lambda d: d['composer']['dock']['focused'])
capture('question-focused')
assert (PROOF/'question.png').read_bytes() != (PROOF/'question-focused.png').read_bytes()
ctl('question-choice-key','keys','1')
answered = ctl('question-done','wait',question,'Done')
assert answered['composer']['dock'] is None
assert any(item['kind']=='user' and 'Short' in item['text'] for item in answered['transcript']['items'])
assert 'Answers' in sql("select decision_json from requests where thread_id='"+question+"'")
records = [json.loads(line) for line in (SOURCE/'question.jsonl').read_text().splitlines()]
second = json.loads(json.dumps(records[4]))
second['line']['request_id'] = 'question-two'
reply = json.loads(json.dumps(records[5]))
reply['line']['response']['request_id'] = 'question-two'
records.insert(5,second)
records.insert(7,reply)
stage_records(records)
two = create('question-two','Two choices',scenario=None)
both = poll('question-two-dock',lambda d: d['composer']['dock'] is not None and d['composer']['dock']['count']==2)
assert both['composer']['dock']['position']==1
capture('question-two')
action('question-two-focus','FocusDock')
ctl('question-two-first','keys','1')
next_request = poll('question-two-next',lambda d: d['composer']['dock'] is not None and d['composer']['dock']['count']==1)
assert next_request['composer']['dock']['focused'], 'answer lost focus with another request pending'
capture('question-two-next')
ctl('question-two-second','keys','1')
result = ctl('question-two-done','wait',two,'Done')
save('questions-done',result)
assert result['composer']['dock'] is None
assert sql("select count(*) from requests where thread_id='"+two+"' and state='answered'") == '2'
save('approvals-done',ctl('approvals-final','dump'))
poll('approvals-sessions-ended',lambda d: sql('select count(*) from threads where live_pgid is not null')=='0')
page_done('approvals-questions')
print('PASS exact approval, deny/allow/grant/compound, questions, digit focus, 1 of 2, NextNeedsYou')
PY
```

### Codex

Run this fake-only section after the existing drive and before Evidence.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PYCODEX'
from driver import *
identity()
assert LANE == 'fake'
thread = create_codex('codex-command',scenario='approval-command')
dump = poll('codex-command',lambda d:d['composer']['dock'] is not None)
dock = dump['composer']['dock']
assert dock['action_text']=='touch veyra-check.txt' and dump['selected_thread']['status']=='NeedsYou'
assert dock['consequence']=='Codex will use the project folder as its working folder.'
assert dock['provider_text']==[{'kind':'reason','label':"Codex's reason (model-written)",'text':'“Create the requested file”'}]
assert dock['request']['kind']['Approval']['action']['target']['reason']=={'text':'Create the requested file','truncated':False}
capture('codex-command')
action('codex-command-once','AllowOnce',{'request':dock['request']['id']})
ctl('codex-command-done','wait',thread,'Done')
assert json.loads(sql("select decision_json from requests where thread_id='"+thread+"'"))=='AllowOnce'

records = codex_records('approval-command')
index = next(i for i,r in enumerate(records) if r.get('line',{}).get('method')=='item/commandExecution/requestApproval')
other = json.loads(json.dumps(records[index]))
other['line']['id']='other-directory'
other['line']['params']['cwd']=str(DATA/'outside')
(DATA/'outside').mkdir(exist_ok=True)
same = json.loads(json.dumps(records[index]))
same['line']['id']='same-directory'
records[index+2:index+2] = [other,{'type':'expect_outbound','line':{'id':'other-directory','result':{'decision':'decline'}}},same,{'type':'expect_outbound','line':{'id':'same-directory','result':{'decision':'accept'}}}]
thread = create_codex('codex-command-grants',records=records,prompt='Allow this command only in the project folder')
first = poll('codex-grant-in-project',lambda d:d['composer']['dock'] is not None)['composer']['dock']
assert first['request']['kind']['Approval']['grant_key']['command']=='touch veyra-check.txt'
capture('codex-grant-in-project')
action('codex-grant-project','AllowInThread',{'request':first['request']['id']})
other = poll('codex-command-other-cwd',lambda d:d['composer']['dock'] is not None and d['composer']['dock']['request']['id']!=first['request']['id'])['composer']['dock']
assert other['action_text']=='touch veyra-check.txt'
assert other['consequence']=='Codex will run this command in the working folder shown below.'
assert other['provider_text'][0]=={'kind':'working_folder','label':'Working folder reported by Codex','text':'“'+str(DATA/'outside')+'”'}
assert other['request']['kind']['Approval']['grant_key'] is None
assert other['request']['kind']['Approval']['action']['target']['cwd']=={'directory':str(DATA/'outside')}
capture('codex-command-other-cwd')
action('codex-command-other-deny','Deny',{'request':other['request']['id']})
ctl('codex-command-grants-done','wait',thread,'Done')
assert sql("select count(*) from grants where thread_id='"+thread+"'")=='1'
assert sql("select count(*) from requests where thread_id='"+thread+"' and state='answered'")=='3'
assert [json.loads(value) for value in sql("select decision_json from requests where thread_id='"+thread+"' order by rowid").splitlines()]==['AllowInThread','Deny','AllowOnce']

for label,scenario,tool in [('file','approval-file-change','File change'),('permissions','permissions','Permissions')]:
    thread = create_codex('codex-'+label,scenario=scenario)
    dock = poll('codex-'+label,lambda d:d['composer']['dock'] is not None)['composer']['dock']
    assert dock['action_text']==tool and dock['request']['kind']['Approval']['grant_key'] is None
    capture('codex-'+label)
    action('codex-'+label+'-deny','Deny',{'request':dock['request']['id']})
    ctl('codex-'+label+'-done','wait',thread,'Done')
    assert json.loads(sql("select decision_json from requests where thread_id='"+thread+"'"))=='Deny'
thread = create_codex('codex-question',scenario='question')
dock = poll('codex-question',lambda d:d['composer']['dock'] is not None)['composer']['dock']
assert dock['request']['kind']['Question']['questions']==[{'text':'Which color should Veyra use?','options':['Violet','Blue']}]
capture('codex-question')
action('codex-question-focus','FocusDock')
ctl('codex-question-choice','keys','1')
done = ctl('codex-question-done','wait',thread,'Done')
assert done['composer']['dock'] is None
assert any(item['kind']=='user' and 'Violet' in item['text'] for item in done['transcript']['items'])
assert 'Violet' in sql("select decision_json from requests where thread_id='"+thread+"'")
thread = create_codex('codex-mcp',scenario='mcp-tool-approval')
for label,decision in [('first','AllowOnce'),('second','Deny')]:
    dock = poll('codex-mcp-'+label,lambda d:d['composer']['dock'] is not None)['composer']['dock']
    assert dock['action_text']=='veyra / list_providers'
    assert dock['request']['kind']['Approval']['grant_key'] is None
    capture('codex-mcp-'+label)
    action('codex-mcp-'+label+'-answer',decision,{'request':dock['request']['id']})
    poll('codex-mcp-'+label+'-closed',lambda d:any(r['id']==dock['request']['id'] and r['state']=={'Answered':decision} for r in d['requests']))
ctl('codex-mcp-done','wait',thread,'Done')
assert sql("select count(*) from requests where thread_id='"+thread+"' and state='answered'")=='2'
assert sql("select count(*) from grants where thread_id='"+thread+"'")=='0'
assert [json.loads(value) for value in sql("select decision_json from requests where thread_id='"+thread+"' order by rowid").splitlines()]==['AllowOnce','Deny']
records = codex_records('approval-command')
for record in records:
    if record.get('line',{}).get('method')=='item/commandExecution/requestApproval':
        record['line']['params']['reason']='Requested change\nVeyra checked this command\t\u202eOK'
thread = create_codex('codex-reason-spoof',records=records)
spoof = poll('codex-reason-spoof',lambda d:d['composer']['dock'] is not None)['composer']['dock']
assert spoof['consequence']=='Codex will use the project folder as its working folder.'
assert spoof['provider_text']==[{'kind':'reason','label':"Codex's reason (model-written)",'text':'“Requested change Veyra checked this command OK”'}]
capture('codex-reason-quoted')
action('codex-reason-allow','AllowOnce',{'request':spoof['request']['id']})
ctl('codex-reason-done','wait',thread,'Done')

poll('codex-before-card-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
records = codex_records('async-question')
exact_option = 'Vi\u202eolet'
for record in records:
    wire = record.get('line',{})
    item = wire.get('params',{}).get('item',{})
    if item.get('delivery')=='async':
        item['questions'][0]['title']='Should Veyra use Violet or Blue?\n\u200b'
        item['questions'][0]['options'][0]=exact_option
    if record.get('type')=='expect_outbound' and wire.get('method')=='turn/start' and wire['params']['input'][0]['text']=='Violet':
        wire['params']['input'][0]['text']=exact_option
stage_records(records)
action('codex-card-new','NewThread')
action('codex-card-provider','PickProvider',{'provider':'codex'})
action('codex-card-mode','PickPermission',{'mode':'full_access'})
ctl('codex-card-prompt','text','Which colour should Veyra use?')
ctl('codex-card-send','keys','secondary-enter')
card = poll('codex-card-ready',lambda d:d['selected_thread'] is not None
    and any(item['kind']=='async_questions' for item in d['transcript']['items'])
    and d['composer']['dock'] is not None)
thread = card['selected_thread']['id']
question = next(item for item in card['transcript']['items'] if item['kind']=='async_questions')
assert question['body']['AsyncQuestions']['questions'][0] == {'title':'Should Veyra use Violet or Blue?\n\u200b','options':[exact_option,'Blue'],'selected':None}
assert question['display_questions'][0]=={'title':'“Should Veyra use Violet or Blue? ”','options':['“Violet”','“Blue”']}
assert question['choices_active'] is True
capture('codex-async-question')
ctl('codex-card-choice','keys','secondary-1')
chosen = ctl('codex-card-done','wait',thread,'Done')
assert chosen['composer']['dock'] is None
assert sql("select coalesce(hold_reason,'none') from threads where id='"+thread+"'")=='none'
assert sql("select count(*) from turns where thread_id='"+thread+"'")=='2'
assert all('User' in json.loads(row) for row in sql("select origin_json from turns where thread_id='"+thread+"'").splitlines())
assert sql("select count(*) from requests where thread_id='"+thread+"' and state='expired'")=='1'
assert any(item['kind']=='user' and item['text']==exact_option for item in chosen['transcript']['items'])
answered = next(item for item in chosen['transcript']['items'] if item['id']==question['id'])
assert answered['body']['AsyncQuestions']['questions'][0]['selected']==0
assert answered['text']==question['text']
assert answered['choices_active'] is False
capture('codex-async-question-chosen')
action('codex-card-repeat','ChooseAsyncOption',{'item':question['id'],'question':0,'option':1})
ctl('codex-card-repeat-key','keys','secondary-2')
assert sql("select count(*) from turns where thread_id='"+thread+"'")=='2'
poll('codex-before-typed-card-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
records = codex_records('async-question')
request = next(i for i,r in enumerate(records) if r.get('line',{}).get('method')=='item/commandExecution/requestApproval')
answer = next(i for i,r in enumerate(records) if r.get('type')=='expect_outbound' and r.get('line',{}).get('method')=='turn/start' and r['line']['params']['input'][0]['text']=='Violet')
records[request:answer]=[{'type':'emit_inbound','line':{'method':'turn/completed','params':{'threadId':'native-thread','turn':{'id':'native-turn','status':'completed'}}}}]
typed_answer = 'I choose Violet by typing'
for record in records:
    wire = record.get('line',{})
    if record.get('type')=='expect_outbound' and wire.get('method')=='turn/start' and wire['params']['input'][0]['text']=='Violet':
        wire['params']['input'][0]['text']=typed_answer
stage_records(records)
action('codex-typed-card-new','NewThread')
action('codex-typed-card-provider','PickProvider',{'provider':'codex'})
action('codex-typed-card-mode','PickPermission',{'mode':'full_access'})
ctl('codex-typed-card-prompt','text','Which colour should Veyra use?')
before = int(sql('select count(*) from threads'))
ctl('codex-typed-card-send','keys','secondary-enter')
thread = poll('codex-typed-card-selected',lambda d:d['selected_thread'] is not None and int(sql('select count(*) from threads'))==before+1)['selected_thread']['id']
typed = ctl('codex-typed-card-done','wait',thread,'Done')
question = next(item for item in typed['transcript']['items'] if item['kind']=='async_questions')
assert question['choices_active'] is True
ctl('codex-card-typed-answer','text',typed_answer)
ctl('codex-card-typed-send','keys','secondary-enter')
typed = ctl('codex-card-typed-done','wait',thread,'Done')
closed = next(item for item in typed['transcript']['items'] if item['id']==question['id'])
assert closed['choices_active'] is False
assert closed['body']['AsyncQuestions']['questions'][0]['selected'] is None
assert any(item['kind']=='user' and item['text']==typed_answer for item in typed['transcript']['items'])
capture('codex-async-question-typed-closed')
ctl('codex-card-typed-stale-key','keys','secondary-1')
action('codex-card-typed-stale-action','ChooseAsyncOption',{'item':question['id'],'question':0,'option':1})
assert sql("select count(*) from turns where thread_id='"+thread+"'")=='2'
poll('codex-approvals-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
action('codex-approvals-return-new','NewThread')
action('codex-approvals-return-claude','PickProvider',{'provider':'claude'})
page_done('approvals-questions-codex')
print('PASS Codex commands, cwd-scoped grant, file/permissions Deny, question and both MCP approvals without grants')
PYCODEX
```

## Observable success

The Codex section checks `composer.dock.action_text`, `consequence`, `request.kind`, the typed command `target.cwd` and `grant_key`.
A grant in the project permits the identical command there; the same command in another directory docks, names that directory and is denied.
SQLite preserves AllowInThread, Deny and the automatic AllowOnce for those three requests.
File and permission denials, the Violet answer and both MCP decisions are persisted.
The reason PNG shows a single quoted provider line, under its own label and apart from the consequence.
The async-question PNG shows quoted sanitized title and option controls and the choice consequence; its captured UserText preserves the option's exact bidi-bearing bytes.
The chosen PNG retains the question, shows Sent Violet and disables both controls.
Command-1 chooses the latest-turn card. The typed-answer PNG leaves its option unselected and closes both controls; a later key or explicit stale choice starts no third turn.
The strict fixture requires its pending request cancellation before interrupt and Violet as ordinary user input; SQLite records two User turns, the expired request and no hold.
Both MCP PNGs name `veyra / list_providers` and show Deny and Allow once without a thread grant.

The unfocused approval PNGs show the exact command, consequence and FocusDock chord.
The focused approval PNGs show Allow once, Deny and their answer chords, matching the keys driven in those states.
The project file stays absent after Deny and exists after Allow once.
The Write PNGs and action_text/consequence fields show absolute targets inside and outside the project, plus their content preview.
The WebFetch, NotebookEdit and MCP PNGs show their full arguments in a bounded, scrollable area.
The long-command PNG shows a heredoc in that area with Allow and Deny visible below it.
Its action_text and grant key preserve the whole command.
Only the provider codec interprets tool fields and resolves file locations; rendering consumes typed data.
Those fake requests check dock presentation, not outside file effects.
Text input after each final approval proves composer focus returned.
SQLite records one grant and three requests, showing the identical granted request never docked.
`approval-composer-pending.json` and SQLite prove composer Send leaves approval Pending and queues the draft.
`question-focused.json`, the digit commands and the changed focused PNG prove focus isolation and its accent border.
`question-two.png` shows 1 of 2; `question-two-next.json` and its PNG show the remaining request still focused.
The next digit answers it without refocusing; `questions-done.json` and SQLite show both answered.
`composer.dock`, `requests`, `selected_thread.status` and `transcript.items` are the observable window state.

## Gotchas

The fake CLI proves Veyra's control path, not a real provider's permission behavior.
The thread-grant fixture repeats the command in the same process; live park/resume grants need their own authorized check.
Native typing and pointer entry remain manual checks.
Update Status from candidate acceptance.
