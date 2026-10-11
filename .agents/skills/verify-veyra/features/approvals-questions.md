# Approvals and questions

Status: not run, 2026-10-11, pending, all

## Sub-features

- Exact command, Deny, AllowOnce and AllowInThread, including a compound command.
- AnswerQuestion choices, FocusDock and FocusComposer, digit focus isolation and two pending requests.
- NextNeedsYou selects the request's thread across the sidebar.
- The request closes as Answered and its answer appears in the transcript.

## How to get to it (user POV)

In Ask first, ask for `touch veyra-check.txt`.
The request docks above the input, with the command and its consequence.
Deny is ⌘., Allow once is ⌘↵ and Allow in this thread is ⌘⌥↵ while the dock has focus.
Press ⌘⇧Y to FocusDock; composer ⌘↵ sends its draft and leaves the request pending.
Every button's shortcut_chip reads the same bindings table as the keymap.
A question shows its options; click Use number keys or press ⌘⇧Y to FocusDock.
Keys 1 to 9 choose only while the dock has focus; Escape runs FocusComposer.
More than one pending request shows 1 of N.
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
assert dock['composer']['dock']['request']['kind']['Approval']['action']['command'] == 'touch veyra-check.txt'
assert dock['selected_thread']['status'] == 'NeedsYou'
capture('approval-deny')
action('approval-deny-focus','FocusDock')
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
ctl('approval-allow-key','keys','secondary-enter')
poll('approval-allow-done',lambda d: d['selected_thread']['status']=='Done'
    and sql("select count(*) from turns where thread_id='"+thread+"' and state='ended'") == '2')
assert check.is_file()
thread = create('approval-grant','Run touch veyra-grant.txt',scenario='approval-grant')
poll('approval-grant-dock',lambda d: d['composer']['dock'] is not None)
capture('approval-grant')
action('approval-grant-answer','AllowInThread')
compound = poll('approval-compound',lambda d: d['composer']['dock'] is not None
    and '&&' in d['composer']['dock']['request']['kind']['Approval']['action']['command'])
assert sql("select count(*) from requests where thread_id='"+thread+"'") == '3'
assert sql("select count(*) from grants where thread_id='"+thread+"'") == '1'
assert (DATA/'project'/'veyra-grant.txt').is_file()
capture('approval-compound')
action('approval-compound-deny','Deny')
ctl('approval-grant-done','wait',thread,'Done')
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
poll('question-two-next',lambda d: d['composer']['dock'] is not None and d['composer']['dock']['count']==1)
action('question-two-refocus','FocusDock')
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

## Observable success

The approval PNGs show the exact command, consequence and chord chips.
The project file stays absent after Deny and exists after Allow once.
SQLite records one grant and three requests, showing the identical granted request never docked.
`approval-composer-pending.json` and SQLite prove composer Send leaves approval Pending and queues the draft.
`question-focused.json`, the digit commands and the changed focused PNG prove focus isolation and its accent border.
`question-two.png` shows 1 of 2; `questions-done.json` and SQLite show both answered.
`composer.dock`, `requests`, `selected_thread.status` and `transcript.items` are the observable window state.

## Gotchas

The fake CLI proves Veyra's control path, not a real provider's permission behavior.
The thread-grant fixture repeats the command in the same process; live park/resume grants need their own authorized check.
Native typing and pointer entry remain manual checks.
Update Status from candidate acceptance.
