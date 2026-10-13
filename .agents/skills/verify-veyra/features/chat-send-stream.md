# Chat send and stream

Status: passed, 2026-10-12, 950317c, none

## Sub-features

- `chat-create`: sending the new-thread composer creates one Claude root thread.
- `chat-stream`: the textarea clears, a user bubble and streamed assistant text appear, with Working then Done.
- `chat-failure`: mid-turn exit shows Failed, Veyra's crash cause and the last stderr lines.
- `chat-retry`: Retry probes again and completes the failed prompt on a new turn.
- `chat-wait`: client exit codes distinguish completion, timeout, bad usage and a missing thread.
- `chat-tools`: captured Bash wires render a collapsed tool row whose detail toggles through the action and key.

## How to get to it (user POV)

Use new-thread's Ready Claude and selected project.
Type `reply with the word ok`, then Send or ⌘↵.
The header shows the title, status with elapsed time, project and branch.
The sidebar has Working, Done, Failed and Idle glyphs.
The chat centres user bubbles and assistant markdown in one column, with a still caret while text streams.
Reasoning and tool rows start collapsed; `ToggleItem` opens their detail, retaining the last 10,000 characters of tool output.
⌘⇧I toggles the latest reasoning/tool detail.
On failure, read the plain cause and stderr tail, use Retry (`veyra::Retry`, ⌘R), or Open Terminal (`veyra::OpenTerminal`, ⌘⌥T).
Open Terminal's OS side effect remains a manual check; this page proves its rendered button, not an external terminal window.
Selecting a thread reaches `SelectThread`; selecting a project reaches `SelectProject`.

## Driving it with veyra ctl

Run Launch, Doctor and new-thread's drive first.
All fixture changes below are proof copies, never edits to committed fixtures.
The helper splits the captured `ok` into `o` then `k`, with design-derived 500 ms delays so the real window has an observable streaming phase.
Use ctl wait for thread states; do not replace it with sleeps.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
before = ctl('chat-before','dump')
if LANE == 'live':
    thread = create('live-chat','reply with the word ok')
    working = ctl('chat-working','dump')
    assert working['selected_thread']['status'] == 'Working'
    assert working['selected_thread']['header_status'].startswith('Working · ')
    assert working['selected_thread']['config']['mode'] == 'full_access'
    capture('chat-working')
    done = ctl('chat-live-done','wait',thread,'Done','120')
    save('chat-done',done)
    reply = [item for item in done['transcript']['items'] if item['kind']=='assistant']
    assert len(reply) == 1 and reply[0]['text'].strip().lower() == 'ok' and reply[0]['done']
    assert reply[0]['delta_count'] >= 1
    assert json.loads(sql("select outcome_json from turns where thread_id='"+thread+"'")) == 'Completed'
    assert sql("select json_extract(body_json,'$.AssistantText.text') from items where thread_id='"
        +thread+"' and kind='assistant_text'").strip().lower() == 'ok'
    assert sql("select native_id is not null and cli_version is not null and cli_version != '' "
        +"from threads where id='"+thread+"'") == '1'
    assert re.fullmatch(r'Done · [0-9]+s',done['selected_thread']['header_status'])
    assert done['selected_thread']['config']['mode'] == 'full_access'
    capture('chat-done')
    page_done('chat-send-stream-live')
    print('PASS live streamed reply; Working to Done; stored reply and native metadata')
    sys.exit(0)
thread = create('chat','reply with the word ok',slow=True)
working = ctl('chat-wait-working','wait',thread,'Working')
assert working['selected_thread']['status'] == 'Working'
stream = poll('chat-working',lambda d: any(item['kind']=='assistant' and item['text']=='o'
    and not item['done'] for item in d['transcript']['items']))
assert stream['selected_thread']['status'] == 'Working'
assert stream['selected_thread']['header_status'].startswith('Working · ')
assert next(item for item in stream['transcript']['items'] if item['kind']=='assistant')['delta_count'] >= 1
assert stream['composer_text'] == ''
assert stream['selected_thread']['native_id']
capture('chat-working')
advanced = poll('chat-timer-advanced',lambda d: d['selected_thread']['id']==thread
    and d['selected_thread']['status']=='Working'
    and re.fullmatch(r'Working · [1-9][0-9]*s',d['selected_thread']['header_status']))
assert advanced['selected_thread']['header_status'] != stream['selected_thread']['header_status']
done = ctl('chat-wait-done','wait',thread,'Done')
save('chat-done',done)
assert done['view'] == 'chat' and done['selected_thread']['status'] == 'Done'
assert [item['text'] for item in done['transcript']['items'] if item['kind']=='user'] == ['reply with the word ok']
assert [(item['text'],item['done']) for item in done['transcript']['items'] if item['kind']=='assistant'] == [('ok',True)]
assert done['perf']['applied_batches'] > before['perf']['applied_batches']
assert json.loads(sql("select outcome_json from turns where thread_id='"+thread
    +"' order by ordinal desc limit 1")) == 'Completed'
assert sql("select json_extract(body_json,'$.AssistantText.text')||'|'||done from items where thread_id='"
    +thread+"' and kind='assistant_text'") == 'ok|1'
assert re.fullmatch(r'Done · [1-9][0-9]*s',done['selected_thread']['header_status'])
capture('chat-done')
ctl('chat-wait-missing','wait','missing-thread','Done',expected=3)
ctl('chat-wait-timeout','wait',thread,'Working','0',expected=1,stderr_contains='veyra ctl wait timed out:')
ctl('chat-wait-bad-usage','wait',thread,'unknown-status',expected=2)
failed = create('chat-crash','reply with the word ok',scenario='crash-mid-turn')
crash = ctl('chat-wait-failed','wait',failed,'Failed')
save('chat-failed',crash)
assert crash['view'] == 'failed_thread'
message = crash['selected_thread']['failure']
assert 'claude exited unexpectedly (exit code 7)' in message
assert 'Veyra fixture error: stream closed' in message and 'last diagnostic line' in message
assert len(message.splitlines()) <= 9
capture('chat-failed')
stage('ok-reply',slow=True)
ctl('chat-retry-key','keys','secondary-r')
retry = ctl('chat-retry-wait','wait',failed,'Done')
save('chat-retried',retry)
assert retry['view'] == 'chat'
assert [(item['text'],item['done']) for item in retry['transcript']['items'] if item['kind']=='assistant'][-1] == ('ok',True)
tool_thread = create('chat-tool','Run echo veyra-probe',scenario='bash-echo')
tools = ctl('chat-tool-done','wait',tool_thread,'Done')
tool = next(item for item in tools['transcript']['items'] if item['kind']=='tool')
assert tool['done'] and tool['text'] == 'veyra-probe' and not tool['expanded']
capture('chat-tool-collapsed')
action('chat-tool-expand','ToggleItem',{'item':tool['id']})
assert next(item for item in ctl('chat-tool-expanded','dump')['transcript']['items'] if item['id']==tool['id'])['expanded']
capture('chat-tool-expanded')
action('chat-tool-collapse','ToggleItem',{'item':tool['id']})
collapsed = ctl('chat-tool-collapsed-action','dump')['transcript']['items']
assert not next(item for item in collapsed if item['id']==tool['id'])['expanded']
latest = next(item['id'] for item in reversed(tools['transcript']['items']) if item['kind'] in ['tool','reasoning'])
action('chat-detail-default-expand','ToggleItem')
assert next(item for item in ctl('chat-detail-default-expanded','dump')['transcript']['items'] if item['id']==latest)['expanded']
action('chat-detail-default-collapse','ToggleItem')
for expanded in [True,False]:
    ctl('chat-detail-key-'+str(expanded),'keys','secondary-shift-i')
    items = ctl('chat-detail-key-after-'+str(expanded),'dump')['transcript']['items']
    assert next(item for item in items if item['id']==latest)['expanded'] == expanded
background = create('chat-background','reply with the word ok',scenario='background-tasks')
busy = background_working('chat-background-working',background)
assert busy['selected_thread']['header_status'].startswith('Working · ')
ambient = ctl('chat-background-done','wait',background,'Done','3')
assert ambient['selected_thread']['status']=='Done'
assert sql("select live_pgid is not null from threads where id='"+background+"'")=='1'
assert sql("select count(*) from turns where thread_id='"+background+"'")=='1'
provider = create('chat-provider','reply with the word ok',scenario='provider-turn-after-background')
reply = 'The background `sleep 20` finished with exit code 0.'
provider_done = poll('chat-provider-done',lambda d: d['selected_thread']['id']==provider
    and d['selected_thread']['status']=='Done'
    and sql("select count(*) from turns where thread_id='"+provider+"' and state='ended'")=='2'
    and [(item['kind'],item['text'],item['done']) for item in d['transcript']['items']
        if item['kind'] in ['assistant','reasoning']] == [('assistant','started',True),('assistant',reply,True)])
assert sql("select group_concat(json_extract(outcome_json,'$')) from turns where thread_id='"
    +provider+"'") == 'Completed,Completed'
provider_turn = sql("select id from turns where thread_id='"+provider+"' and origin_json='\"Provider\"'")
assert provider_turn
assert sql("select count(*) from items where turn_id='"+provider_turn+"'") == '1'
assert sql("select kind||'|'||json_extract(body_json,'$.AssistantText.text')||'|'||done "
    +"from items where turn_id='"+provider_turn+"'") == 'assistant_text|'+reply+'|1'
assert [(item['kind'],item['text'],item['done']) for item in provider_done['transcript']['items']
    if item['kind'] in ['assistant','reasoning']] == [('assistant','started',True),('assistant',reply,True)]
# The failed turn's own prompt survives a newer queued message.
records = [json.loads(line) for line in (SOURCE/'crash-mid-turn.jsonl').read_text().splitlines()]
records[2]['line']['response']={'subtype':'error','request_id':'<id:initialize>',
    'error':'fixture first-turn failure'}
records[2]['after_ms']=3000
records = records[:3]+[records[-1]]
stage_records(records)
retry_thread = create('chat-retry-queued','failed prompt',scenario=None)
ctl('chat-retry-queued-working','wait',retry_thread,'Working')
ctl('chat-retry-newer-text','text','queued prompt')
action('chat-retry-newer-send','Send')
poll('chat-retry-original-failed',lambda d: sql("select count(*) from turns where thread_id='"+retry_thread
    +"' and json_type(outcome_json,'$.Failed') is not null")=='1')
stage('ok-reply',slow=True,turns=2)
action('chat-retry-own-prompt','Retry')
poll('chat-retry-both-done',lambda d: sql("select count(*) from turns where thread_id='"+retry_thread
    +"' and outcome_json='\"Completed\"'")=='2')
assert sql("select group_concat(json_extract(i.body_json,'$.UserText'),'|') from turns t "
    +"join items i on i.id=json_extract(t.origin_json,'$.User.item') where t.thread_id='"+retry_thread+"'") == 'failed prompt|queued prompt|failed prompt'
# Make a long streaming row from Veyra's own ok fixture, then drive the actual list scroll API.
records = [json.loads(line) for line in (SOURCE/'ok-reply.jsonl').read_text().splitlines()]
long_text = '\n\n'.join('Line '+str(i)+': Veyra follow probe' for i in range(200))
for record in records:
    wire = record.get('line',{})
    if wire.get('event',{}).get('delta',{}).get('text')=='ok':
        wire['event']['delta']['text']=long_text
    if wire.get('type')=='result': record['after_ms']=3000
delta = next(i for i,record in enumerate(records) if record.get('line',{}).get('event',{}).get('delta',{}).get('type')=='text_delta')
part = json.loads(json.dumps(records[delta]))
part['line']['event']['delta']['text']='\n\nAfter scroll'
part['after_ms']=500
records.insert(delta+1,part)
stage_records(records)
follow_thread = create('chat-follow','Show a long reply',scenario=None)
poll('chat-follow-streaming',lambda d: any(item['kind']=='assistant' and len(item['text'])>1000 for item in d['transcript']['items']))
poll('chat-follow-laid-out',lambda d:d['transcript']['top_offset']>200)
assert ctl('chat-follow-attached','dump')['transcript']['follow']
action('chat-follow-scroll','DebugScrollTranscript',{'pixels':-200})
detached = poll('chat-follow-detached',lambda d: not d['transcript']['follow'])
capture('chat-follow-detached')
ctl('chat-follow-ended','wait',follow_thread,'Done')
assert not ctl('chat-follow-after-new-text','dump')['transcript']['follow']
gap = ctl('chat-follow-gap','dump')['transcript']['gap']
action('chat-follow-outside-line','DebugScrollTranscript',{'pixels':gap-25})
outside = poll('chat-follow-25',lambda d: 24.5 < d['transcript']['gap'] < 25.5)
assert not outside['transcript']['follow']
action('chat-follow-within-line','DebugScrollTranscript',{'pixels':1})
poll('chat-follow-24',lambda d: d['transcript']['follow'])
capture('chat-follow-latest')
action('chat-follow-focus-up','ScrollTranscriptUp')
ctl('chat-follow-up-key','keys','pageup')
assert not ctl('chat-follow-up','dump')['transcript']['follow']
ctl('chat-follow-down-key','keys','pagedown')
ctl('chat-follow-end-key','keys','secondary-end')
assert ctl('chat-follow-end','dump')['transcript']['follow']
# Failed tool output is visible without expanding; expansion keeps its top edge.
records = [json.loads(line) for line in (SOURCE/'bash-echo.jsonl').read_text().splitlines()]
for record in records:
    wire = record.get('line',{})
    if wire.get('type')=='user':
        for block in wire.get('message',{}).get('content',[]):
            if isinstance(block,dict) and block.get('type')=='tool_result':
                block['is_error']=True
                block['content']='first diagnostic\nlast tool error\n'
stage_records(records)
failed_tool = create('chat-failed-tool','Show a failed tool',scenario=None)
tool_dump = ctl('chat-failed-tool-done','wait',failed_tool,'Done')
failed_row = next(item for item in tool_dump['transcript']['items'] if item['kind']=='tool')
assert failed_row['failed_last_line']=='last tool error' and not failed_row['expanded']
capture('chat-failed-tool')
index = next(i for i,item in enumerate(tool_dump['transcript']['items']) if item['id']==failed_row['id'])
old_top = poll('chat-failed-tool-measured',lambda d: d['transcript']['item_tops'][index] is not None)['transcript']['item_tops'][index]
action('chat-failed-tool-expand','ToggleItem',{'item':failed_row['id']})
expanded = poll('chat-failed-tool-expanded',lambda d: next(item for item in d['transcript']['items'] if item['id']==failed_row['id'])['expanded'])
assert abs(expanded['transcript']['item_tops'][index]-old_top)<1
capture('chat-failed-tool-expanded')
signed = create('chat-signed-out','Sign-in failure',scenario='assistant-error-signed-out')
signed_dump = ctl('chat-signed-out-failed','wait',signed,'Failed')
assert 'Claude is signed out. Sign in from a terminal' in signed_dump['selected_thread']['failure']
capture('chat-signed-out')
records = [json.loads(line) for line in (SOURCE/'assistant-error-signed-out.jsonl').read_text().splitlines()]
records[4]['after_ms']=3000
stage_records(records)
retire = create('chat-retire','Fail then queue',scenario=None)
old_pid = int(sql("select live_pid from threads where id='"+retire+"'"))
ctl('chat-retire-queue-text','text','after sign-in failure')
action('chat-retire-queue-send','Send')
stage('ok-reply',slow=True)
new_pid = poll('chat-retire-new-pid',lambda d: sql("select live_pid from threads where id='"+retire+"'") not in ['',str(old_pid)])
ctl('chat-retire-done','wait',retire,'Done')
assert sql("select count(*) from turns where thread_id='"+retire+"' and outcome_json='\"Completed\"'")=='1'
compact = create('chat-compact','Compact',scenario='compact-and-clear')
ctl('chat-compact-done','wait',compact,'Done')
ctl('chat-clear-text','text','Clear')
action('chat-clear-send','Send')
clear = poll('chat-clear-done',lambda d: d['selected_thread']['id']==compact
    and sql("select count(*) from turns where thread_id='"+compact+"' and state='ended'")=='2')
assert len([item for item in clear['transcript']['items'] if item['kind']=='notice'])==2
capture('chat-compact-clear')
action('chat-compact-close','StopThread',{'thread':compact})
for scenario, count in [('bad-lines',1),('notices',3)]:
    thread = create('chat-'+scenario,'reply with the word ok',scenario=scenario)
    dump = ctl('chat-'+scenario+'-done','wait',thread,'Done')
    assert len([item for item in dump['transcript']['items'] if item['kind']=='notice'])==count
    if scenario=='bad-lines': assert any(item['kind']=='assistant' and item['text']=='ok' for item in dump['transcript']['items'])
    capture('chat-'+scenario)
poll('chat-sessions-ended',lambda d: sql('select count(*) from threads where live_pgid is not null') == '0')
page_done('chat-send-stream')
print('PASS chat streamed o → ok, Working → Done; crash cause/tail; Retry; wait exits 0/1/2/3')
PY
```

DebugScrollTranscript scrolls the same public gpui list state used by keys and the wheel; its pixels payload tests the 24 px boundary.
ScrollTranscriptUp, ScrollTranscriptDown and ScrollTranscriptEnd use Page Up, Page Down and Command+End.
The debug transcript also reports gap, top_item, top_offset and item_tops for scroll and expansion assertions.

## Observable success

- `chat-working.json` has Working, assistant `o` with `done: false`, a native id and cleared composer; inspect its PNG for the caret and Working header.
- `chat-done.json` has one user prompt and exactly one completed assistant `ok`, Done in the selected thread and sidebar, and an increased applied-batch counter.
- `sql.log` keeps the SQLite `outcome_json` Completed and stored assistant `ok|1` checks.
- `chat-timer-advanced.json` proves Working time advances above zero; Done also has positive elapsed seconds.
- Streamed assistant items have a positive `delta_count`.
- `chat-background-working.json` stays Working after its foreground turn ends and `wait Done 1` times out.
- The ambient-only interval shows Done while the session remains alive; `chat-provider-done.json` shows Done after the separate provider turn completes.
- The CLI-started turn stores exactly one completed assistant reply and no empty reasoning; `sql.log` and the dump assert the exact items.
- `chat-failed.json` has `view: failed_thread`, Failed and the crash cause with both fixture stderr lines; its PNG shows the cause, next step, Retry and Open Terminal.
- `chat-retried.json` has Done with the retried completed `ok`.
- `drive.log` records wait exit 0 for each state, 1 for the zero-second timeout, 2 for bad usage and 3 for a missing thread.
- `chat-tool-done.json` has a completed collapsed tool with captured output `veyra-probe`; the action and key assert its expanded state, with collapsed/expanded PNGs.
- Thread selection and transcript loading use the snapshot fence, not a UI database read.

`chat-follow-25.json` remains detached at 25 px and `chat-follow-24.json` reattaches within one read line.
The failed-tool PNG shows the last diagnostic while collapsed; its expansion assertion compares the same row top.
The signed-out, retirement and notice dumps cover account and notice window behavior.

## Gotchas

Replay is not a live permission or tool proof.
Approvals, questions and Stop are covered by their feature pages.
A streamed row may complete before the turn ends; the header remains Working until the result.
Open Terminal and native mouse/keyboard entry points remain manual checks.
Update Status from candidate acceptance.
