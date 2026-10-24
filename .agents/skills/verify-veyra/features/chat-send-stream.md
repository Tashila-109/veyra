# Chat send and stream

Status: passed, 2026-10-24, 90327b9, live lane not rerun since RUN 8d22e59d at 2b45067 (fake CLI RUN b4c41aec), 2e8165b37dfd99a4437e779503095b001fc46ee6e8c61b86a71fb561462801f5
Covers: crates/veyra/src/ui/provider_text.rs crates/engine/src/provider/** crates/engine/tests/fixtures/claude/** crates/engine/tests/fixtures/codex/** crates/engine/tests/fixtures/grok/**

## Sub-features

- `chat-create`: sending the new-thread composer creates one Claude root thread.
- `chat-stream`: the textarea clears, a user bubble and streamed assistant text appear, with Working then Done.
- `chat-failure`: mid-turn exit shows Failed, Veyra's crash cause and the last stderr lines.
- `chat-retry`: Retry probes again and completes the failed prompt on a new turn.
- `chat-wait`: `ctl wait` returns the state it waited for and times out with exit 1 and its named stderr line.
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
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
prepare()
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
records = [json.loads(line) for line in (SOURCE/'ok-reply.jsonl').read_text().splitlines()]
initialize_request = next(record for record in records if record.get('type')=='expect_outbound'
    and record.get('line',{}).get('type')=='control_request'
    and record['line']['request'].get('subtype')=='initialize')
initialize_response = next(record for record in records if record.get('type')=='emit_inbound'
    and record.get('line',{}).get('type')=='control_response'
    and record['line']['response'].get('request_id')==initialize_request['line']['request_id'])
initialize_response['after_ms']=3000
stage_records(records)
starting_thread = create('chat-starting','Wait for the CLI to start',scenario=None)
starting = poll('chat-starting',lambda d:d['selected_thread']['wait_reason']=='Waits for the CLI to start')
assert starting['selected_thread']['header_status']=='Starting Claude Code'
capture('chat-starting')
ctl('chat-starting-done','wait',starting_thread,'Done')
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
ctl('chat-wait-timeout','wait',thread,'Working','0',expected=1,stderr_contains='veyra ctl wait timed out:')
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
capture('chat-follow-first-bottom')
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
expanded = poll('chat-failed-tool-expanded',lambda d: next(item for item in d['transcript']['items'] if item['id']==failed_row['id'])['expanded']
    and d['transcript']['item_tops'][index] is not None)
assert abs(expanded['transcript']['item_tops'][index]-old_top)<1
capture('chat-failed-tool-expanded')
signed = create('chat-signed-out','Sign-in failure',scenario='assistant-error-signed-out')
signed_dump = ctl('chat-signed-out-failed','wait',signed,'Failed')
assert 'Claude is signed out. Sign in from a terminal' in signed_dump['selected_thread']['failure']
assert [item['outcome'] for item in signed_dump['transcript']['items'] if item['outcome']] == ['Failed']
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
    if scenario=='notices':
        assert any('The reset time has passed.' in item['text'] and item['body']['Notice'].get('reset_at')==12345 for item in dump['transcript']['items'] if item['kind']=='notice')
    capture('chat-'+scenario)
poll('chat-sessions-ended',lambda d: sql('select count(*) from threads where live_pgid is not null') == '0')
def retained(records, provider):
    records = json.loads(json.dumps(records))
    assert records[-1]['type'] == 'exit'
    records.insert(-1, {'type':'emit_inbound','after_ms':60000,'line':
        {'type':'system','subtype':'idle'} if provider=='claude' else {'jsonrpc':'2.0','method':'unknown/idle','params':{}}})
    return records

def send_next(label, thread, turns):
    ctl(label+'-draft','text','Continue on this process')
    action(label+'-send','Send')
    poll(label+'-admitted',lambda d:int(sql("select count(*) from turns where thread_id='"+thread+"'"))==turns)
    return ctl(label+'-done','wait',thread,'Done')

for scenario, words in [
    ('result-403','Your organization cannot use this model. Choose another model and send again.'),
    ('prompt-skipped','Claude took this message but never started a turn for it. Send it again.'),
]:
    label = 'chat-claude-'+scenario
    records = [json.loads(line) for line in (SOURCE/(scenario+'.jsonl')).read_text().splitlines()]
    stage_records(retained(records,'claude'))
    action(label+'-new','NewThread')
    action(label+'-provider','PickProvider',{'provider':'claude'})
    action(label+'-mode','PickPermission',{'mode':'ask_first'})
    ctl(label+'-draft','text','Check this message')
    action(label+'-send','Send')
    failed = poll(label+'-failed',lambda d:d['selected_thread'] is not None and d['selected_thread']['status']=='Failed' and words in json.dumps(d))
    assert [item['outcome'] for item in failed['transcript']['items'] if item['outcome']] == ['Failed']
    thread = failed['selected_thread']['id']
    pid = sql("select live_pid from threads where id='"+thread+"'")
    assert pid and pid != '0'
    capture(label+'-failed')
    send_next(label+'-next',thread,2)
    assert sql("select live_pid from threads where id='"+thread+"'") == pid
    assert sql("select count(*) from turns where thread_id='"+thread+"' and state='ended'") == '2'
    save(label+'-same-process',{'thread':thread,'pid':int(pid),'turns':2})
    capture(label+'-next-done')
    action(label+'-close','PickPermission',{'mode':'ask_first'})
    poll(label+'-closed',lambda d:sql("select live_pid from threads where id='"+thread+"'")=='' )


records = [json.loads(line) for line in (SOURCE/'bash-echo.jsonl').read_text().splitlines()]
position = next(i for i,r in enumerate(records) if r.get('type')=='emit_inbound'
    and r.get('line',{}).get('type')=='result')
records[position]['after_ms']=60000
records.insert(position,{'type':'emit_inbound','line':{'type':'assistant','session_id':'<id:session>',
    'parent_tool_use_id':None,'message':{'content':[{'type':'tool_use','id':'still-open-tool',
        'name':'Bash','input':{'command':'second command'}}]}}})
stage_records(records)
thread = create('chat-result-stop','Keep the first result when stopping the second command',scenario=None)
before_stop = poll('chat-result-before-stop',lambda d:len([row for row in d['transcript']['items'] if row['kind']=='tool'])==2)
assert [row['tool_status'] for row in before_stop['transcript']['items'] if row['kind']=='tool']==['Done','Working']
action('chat-result-stop','Stop')
after_stop = ctl('chat-result-stopped','wait',thread,'Interrupted')
assert [row['tool_status'] for row in after_stop['transcript']['items'] if row['kind']=='tool']==['Done','Stopped']
assert next(row for row in after_stop['transcript']['items'] if row['kind']=='tool')['text']=='veyra-probe'
capture('chat-result-stopped')

page_done('chat-send-stream')
print('PASS chat streamed o → ok, Working → Done; crash cause/tail; Retry; wait exits 0/1/2/3')
PY
```

DebugScrollTranscript scrolls the same public gpui list state used by keys and the wheel; its pixels payload tests the 24 px boundary.
ScrollTranscriptUp, ScrollTranscriptDown and ScrollTranscriptEnd use Page Up, Page Down and Command+End.
The debug transcript also reports gap, top_item, top_offset and item_tops for scroll and expansion assertions.

### Codex

Run this fake-only section after the existing drive and before Evidence.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PYCODEX'
from driver import *
prepare()
assert LANE == 'fake'
thread = create_codex('codex-chat',slow=True)
stream = poll('codex-chat-working',lambda d:any(item['kind']=='assistant' and item['text']=='o' and not item['done'] for item in d['transcript']['items']))
assert stream['selected_thread']['status']=='Working'
assert stream['composer_text']=='' and stream['selected_thread']['native_id']
capture('codex-chat-working')
done = ctl('codex-chat-wait','wait',thread,'Done')
save('codex-chat-done',done)
assert [(item['text'],item['done']) for item in done['transcript']['items'] if item['kind']=='assistant']==[('ok',True)]
assert next(item for item in done['transcript']['items'] if item['kind']=='assistant')['delta_count']==2
assert sql("select json_extract(body_json,'$.AssistantText.text')||'|'||done from items where thread_id='"+thread+"' and kind='assistant_text'")=='ok|1'
assert json.loads(sql("select outcome_json from turns where thread_id='"+thread+"'"))=='Completed'
capture('codex-chat-done')
thread = create_codex('codex-other-thread',scenario='subagent-other-thread')
done = ctl('codex-other-thread-wait','wait',thread,'Done')
save('codex-other-thread-done',done)
assert sql("select count(*) from turns where thread_id='"+thread+"'")=='1'
assert 'User' in json.loads(sql("select origin_json from turns where thread_id='"+thread+"'"))
assert [item['text'] for item in done['transcript']['items'] if item['kind']=='assistant']==['ok']
assert sql("select json_extract(body_json,'$.AssistantText.text') from items where thread_id='"+thread+"' and kind='assistant_text'")=='ok'
capture('codex-other-thread')
records = codex_records('retry-and-limits')
reset = int(time.time())+65
for record in records:
    if record.get('line',{}).get('method')=='account/rateLimits/updated':
        record['line']['params']['rateLimits']['primary']['resetsAt']=reset
thread = create_codex('codex-notices',records=records)
done = ctl('codex-notices-wait','wait',thread,'Done')
save('codex-notices-done',done)
expected = ['Codex is retrying after a provider error.','Codex is near its usage limit: primary window, 95% used.']
notices = [item for item in done['transcript']['items'] if item['kind']=='notice']
assert notices[0]['text']==expected[0]
assert notices[1]['text']==expected[1]+' Resets in 2 min.'
assert notices[1]['body']['Notice']['reset_at']==reset
stored = sql("select json_extract(body_json,'$.Notice.text') from items where thread_id='"+thread+"' and kind='notice' order by seq").splitlines()
assert stored==expected
frame = done['perf']['repaints']
advanced = poll('codex-notice-clock',lambda d:any(item['kind']=='notice'
    and item['text']==expected[1]+' Resets in less than a minute.' for item in d['transcript']['items'])
    and d['perf']['repaints']>frame)
assert next(item for item in advanced['transcript']['items'] if item['body'].get('Notice',{}).get('reset_at'))['body']['Notice']['reset_at']==reset
capture('codex-notices')
records = codex_records('shell-echo')
for record in records:
    wire = record.get('line',{})
    if record['type']=='expect_outbound' and wire.get('method') in ['thread/start','turn/start']:
        wire['params']['approvalPolicy']='untrusted'
        if wire['method']=='thread/start': wire['params']['sandbox']='read-only'
        else: wire['params']['sandboxPolicy']={'type':'readOnly','networkAccess':True}
thread = create_codex('codex-command-row',records=records)
done = ctl('codex-command-row-done','wait',thread,'Done')
tool = next(item for item in done['transcript']['items'] if item['kind']=='tool')
assert tool['tool_label']=='Command · '+tool['body']['Tool']['call']['input_summary'] and tool['body']['Tool']['call']['veyra_label'] is True
capture('codex-command-row')
action('codex-command-row-expand','ToggleItem',{'item':tool['id']})
capture('codex-command-row-expanded')
records = codex_records('ok-reply')
position = next(i for i,r in enumerate(records) if r.get('line',{}).get('method')=='turn/completed')
records.insert(position,{'type':'emit_inbound','line':{'method':'item/completed','params':{'threadId':'<id:session>','turnId':'native-turn','item':{'type':'mcpToolCall','id':'provider-tool','server':'example','tool':'lookup','arguments':{},'status':'completed'}}}})
thread = create_codex('codex-provider-tool-row',records=records)
done = ctl('codex-provider-tool-row-done','wait',thread,'Done')
tool = next(item for item in done['transcript']['items'] if item['kind']=='tool')
assert tool['tool_label']=='example / lookup' and tool['body']['Tool']['call']['veyra_label'] is False
capture('codex-provider-tool-row')
records = codex_records('ok-reply')
position = next(i for i,r in enumerate(records) if r.get('type')=='emit_inbound'
    and r.get('line',{}).get('method')=='turn/completed')
second = next(json.loads(json.dumps(r)) for r in records if r.get('type')=='emit_inbound'
    and r.get('line',{}).get('method')=='item/completed'
    and r['line']['params']['item'].get('type')=='agentMessage')
second['line']['params']['item'].update(id='paragraph-two',text='A second paragraph: a'+'\u0301'*6)
records.insert(position,second)
thread = create_codex('codex-separated-parts',records=records)
parts = ctl('codex-separated-parts-done','wait',thread,'Done')
replies = [row for row in parts['transcript']['items'] if row['kind']=='assistant']
assert len(replies)==2 and replies[0]['text']=='ok'
assert replies[1]['text']=='A second paragraph: a'+'\u0301'*3
assert replies[1]['body']['AssistantText']['text'].endswith('\u0301'*6)
capture('codex-separated-parts')
action('codex-chat-return-new','NewThread')
action('codex-chat-return-claude','PickProvider',{'provider':'claude'})
page_done('chat-send-stream-codex')
print('PASS Codex streams ok; foreign thread creates no turn/text; two stored notices')
PYCODEX
```

### Grok

Run this fake-only section after Codex.
The signed-out fixture advertises only grok.com and fails session opening without any authenticate call.
The background fixture clears its task snapshot and ends the Grok-started report only on its own turn_completed notification.
The launch-argument receipt records only the fake CLI's own arguments and must contain neither optional selection flag.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PYGROK'
from driver import *
prepare()
assert LANE == 'fake'
records = grok_records('ok-reply',slow=True)
records.insert(1,{'type':'record_argv','path':'grok-launch-argv.json'})
thread = create_grok('grok-chat',records=records)
stream = poll('grok-chat-working',lambda d:d['selected_thread']['id']==thread
    and d['selected_thread']['status']=='Working'
    and any(item['kind']=='assistant' and item['text']=='o' and not item['done'] for item in d['transcript']['items']))
assert stream['composer']['provider']=='grok' and stream['composer']['stop_visible']
assert stream['selected_thread']['config']['model'] is None and stream['selected_thread']['config']['effort'] is None
capture('grok-chat-working')
done = ctl('grok-chat-done','wait',thread,'Done')
assert [(item['text'],item['done']) for item in done['transcript']['items'] if item['kind']=='assistant']==[('ok',True)]
assert next(item for item in done['transcript']['items'] if item['kind']=='assistant')['delta_count']==2
assert json.loads(sql("select outcome_json from turns where thread_id='"+thread+"'"))=='Completed'
assert sql("select json_extract(body_json,'$.AssistantText.text')||'|'||done from items where thread_id='"+thread+"' and kind='assistant_text'")=='ok|1'
arguments = json.loads((DATA/'project/grok-launch-argv.json').read_text())
assert pathlib.Path(arguments[0]).resolve()==pathlib.Path('target/debug/veyra-fake-cli').resolve()
assert arguments[1:]==['--no-auto-update','--permission-mode','default','agent','--no-leader','stdio']
save('grok-launch-argv',arguments[1:])
capture('grok-chat-done')
thread = create_grok('grok-signed-out',scenario='signed-out',prompt='Show an authentication failure')
failed = ctl('grok-signed-out','wait',thread,'Failed')
assert failed['view']=='failed_thread'
assert 'Sign in with `grok login`' in failed['selected_thread']['failure']
assert [item['outcome'] for item in failed['transcript']['items'] if item['outcome']] == ['Failed']
assert 'grok login' in sql("select outcome_json from turns where thread_id='"+thread+"'")
capture('grok-signed-out')
records = grok_records('background-work')
for record in records:
    if record.get('line',{}).get('method')=='_x.ai/task_completed': record['after_ms']=3000
thread = create_grok('grok-background',records=records,prompt='Start background work and report its result')
foreground_ended('grok-background-foreground-ended',thread)
background = poll('grok-background-working',lambda d:d['selected_thread']['id']==thread and d['selected_thread']['status']=='Working' and not d['composer']['stop_visible'])
assert background['composer']['send_enabled']
capture('grok-background-working')
background = ctl('grok-background-done','wait',thread,'Done')
background = poll('grok-background-done-rendered',lambda d:d['selected_thread']['id']==thread
    and d['selected_thread']['status']=='Done' and not d['composer']['stop_visible'])
assert not background['composer']['stop_visible']
assert sql("select count(*) from turns where thread_id='"+thread+"' and outcome_json='\"Completed\"'")=='2'
assert sql("select count(*) from turns where thread_id='"+thread+"' and origin_json='\"Provider\"'")=='1'
assert [item['text'] for item in background['transcript']['items'] if item['kind']=='assistant']==['foreground','background reply']
capture('grok-background-done')
records = grok_records('tool-title')
position = next(i for i,r in enumerate(records) if r.get('type')=='emit_inbound'
    and r.get('line',{}).get('params',{}).get('update',{}).get('sessionUpdate')=='tool_call')
message = next(json.loads(json.dumps(r)) for r in records if r.get('type')=='emit_inbound'
    and r.get('line',{}).get('params',{}).get('update',{}).get('sessionUpdate')=='agent_message_chunk')
message['line']['params']['update']['content']['text']='Before the tool.'
records.insert(position,message)
thread = create_grok('grok-tool-title',records=records,prompt='Run pwd')
titled = ctl('grok-tool-title-done','wait',thread,'Done')
tools = [item for item in titled['transcript']['items'] if item['kind']=='tool']
assert len(tools)==1 and tools[0]['body']['Tool']['call']['name']=='Execute `pwd`'
assert len([item for item in titled['transcript']['items'] if item['kind']=='assistant'])==2
assert sql("select count(*) from items where thread_id='"+thread+"' and kind='tool'")=='1'
capture('grok-tool-title')
poll('grok-chat-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
action('grok-chat-return-new','NewThread')
action('grok-chat-return-claude','PickProvider',{'provider':'claude'})
records = grok_records('turn-completed-only')
records.insert(-1, {'type':'emit_inbound','after_ms':60000,'line':{'jsonrpc':'2.0','method':'unknown/idle','params':{}}})
started = time.monotonic()
thread = create_grok('grok-completion-grok-guard', records=records)
working = poll('grok-completion-grok-guard-working',lambda d:d['selected_thread']['id']==thread and d['selected_thread']['status']=='Working')
pid = sql("select live_pid from threads where id='"+thread+"'")
capture('grok-completion-grok-guard-working')
ctl('grok-completion-grok-guard-first-done','wait',thread,'Done')
elapsed = time.monotonic()-started
assert elapsed >= 1.8, elapsed
capture('grok-completion-grok-guard-first-done')
ctl('grok-completion-second-draft','text','Continue on this process')
action('grok-completion-second-send','Send')
poll('grok-completion-second-admitted',lambda d:int(sql("select count(*) from turns where thread_id='"+thread+"'"))==2)
ctl('grok-completion-second-done','wait',thread,'Done')
assert sql("select live_pid from threads where id='"+thread+"'") == pid
ctl('grok-completion-third-draft','text','Continue on this process')
action('grok-completion-third-send','Send')
poll('grok-completion-third-admitted',lambda d:int(sql("select count(*) from turns where thread_id='"+thread+"'"))==3)
ctl('grok-completion-third-done','wait',thread,'Done')
assert sql("select live_pid from threads where id='"+thread+"'") == pid
assert sql("select count(*) from turns where thread_id='"+thread+"' and state='ended'") == '3'
final = ctl('grok-completion-final','dump')
assert all(part in json.dumps(final) for part in ['First reply.','Second reply.','Third reply.'])
save('grok-completion-grok-same-process',{'thread':thread,'pid':int(pid),'turns':3,'first_elapsed_seconds':elapsed})
capture('grok-completion-grok-guard-third-done')

action('grok-completion-close','PickPermission',{'mode':'ask_first'})
poll('grok-completion-closed',lambda d:sql("select live_pid from threads where id='"+thread+"'")=='' )

page_done('chat-send-stream-grok')
print('PASS Grok streams ok; Default leaves both flags out; signed-out failure; background task and Provider turn end')
PYGROK
```

## Observable success

The Claude 403 and completed-without-started fixtures show their own failure cause with no sign-in instruction.
Their next turn completes on the same saved process pid, with two ended turns in SQLite.
The sign-in failure view shows the command already supplied by its cause only once.
The Grok completion fixture waits for the own-prompt two-second guard, then completes three replies on one process despite reordered untagged tails.
Inspect those failure, next-turn and guarded-completion PNGs.

`grok-chat-working.png` shows the first streamed character and Stop; `grok-chat-done.png`, its dump and SQLite show one completed ok reply.
`grok-launch-argv.json` proves neither model nor effort flag was sent to the fake CLI.
`grok-signed-out.png`, `selected_thread.failure` and SQLite name `grok login` in the existing failed view.
`grok-background-working.png` shows Working with Send after the foreground ends, and `grok-background-done.png` shows the report completed.
SQLite retains one User turn and one Provider turn, both Completed, after task_snapshot completion and the provider prompt's turn_completed notification.
`grok-tool-title.png` and its dump retain one tool row titled Execute `pwd` after title-only updates, with its completed output preserved.

The Codex section saves streamed `o`, completed `ok` with two `delta_count` events, its stored item and Completed turn.
The foreign-thread fixture leaves one User turn and only `ok`; the retry/limit fixture stores exactly two Notice items.
The Codex PNGs show Working, Done and both notices.
`codex-command-row.png` shows the unquoted Command title followed by its command; `codex-provider-tool-row.png` shows the provider-named MCP tool without quotes.
`codex-notice-clock.json` keeps the stored reset instant while the rendered notice advances from two minutes to less than a minute and the repaint counter advances.
`codex-separated-parts.png` separates two item replies and bounds the visible combining marks while the stored body keeps all six.
`grok-tool-title.png` separates the replies on each side of the tool.
`chat-starting.png` shows Starting Claude Code from the pending spawn wait reason.
`chat-result-stopped.png` retains the completed result and marks only the second, open command Stopped.
Both names share the bounded display projection.

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
- `drive.log` records wait exit 0 for each state and 1 with `veyra ctl wait timed out:` for the zero-second timeout.
  Exit 2 for bad usage and 3 for a missing or archived thread are proven by `ctl_wait_exit_codes` (`crates/veyra/src/control.rs`), so this page no longer drives them.
- `chat-tool-done.json` has a completed collapsed tool with captured output `veyra-probe`; the action and key assert its expanded state, with collapsed/expanded PNGs.
- Thread selection and transcript loading use the snapshot fence, not a UI database read.

`chat-follow-25.json` remains detached at 25 px and `chat-follow-24.json` reattaches within one read line.
The failed-tool PNG shows the last diagnostic while collapsed; its expansion assertion compares the same row top.
The signed-out, retirement and notice dumps cover account and notice window behavior.

### Dump fields

`header_branch` is the branch text used by the header and is empty after leaving Chat.
`view` is `new_thread`, `chat`, `failed_thread` or `settings`.
`transcript.items` contains the stored item rows; item_tops includes the family card first when present.
`selected_thread` includes its native id, configuration, failure cause and `header_status`, the rendered status with elapsed seconds.
For stored-mode compatibility, see [stored Grok modes](permission-matrix.md#stored-grok-modes); dump retains the stored configuration beside the effective composer mode.
`transcript.items` carries the rendered items and `transcript.follow` is true while attached to the bottom.
`transcript.gap`, `top_item`, `top_offset` and `item_tops` expose the rendered ListState geometry for the follow and expansion checks.
Items include `outcome`, queued `wait_reason`, a tool's `failed_last_line` and its rendered `tool_label`.
The failed banner's own turn has `outcome` `["Failed"]`; historical failed turns retain their cause in the caption.
Each transcript item carries `delta_count`, the actual text-stream deltas observed during this app run.
Engine commands await space in the window's gpui task, so Stop is retained when that capacity is zero.

### Evidence contract

```json
{
  "required": {
    "chat-send-stream": [
      "chat-working.json",
      "chat-working.png",
      "chat-done.json",
      "chat-done.png",
      "chat-failed.json",
      "chat-failed.png",
      "chat-provider-done.json",
      "chat-background-working.json",
      "chat-follow-first-bottom.png",
      "chat-starting.png",
      "chat-starting.json",
      "chat-result-stopped.png"
    ],
    "chat-send-stream-codex": [
      "codex-chat-working.json",
      "codex-chat-working.png",
      "codex-chat-done.json",
      "codex-chat-done.png",
      "codex-other-thread-done.json",
      "codex-other-thread.png",
      "codex-notices-done.json",
      "codex-notices.png",
      "codex-separated-parts.png",
      "codex-separated-parts-done.json"
    ],
    "chat-send-stream-grok": [
      "grok-chat-working.json",
      "grok-chat-working.png",
      "grok-chat-done.json",
      "grok-chat-done.png",
      "grok-launch-argv.json",
      "grok-signed-out.json",
      "grok-signed-out.png"
    ],
    "chat-send-stream-live": [
      "chat-working.json",
      "chat-working.png",
      "chat-done.json",
      "chat-done.png",
      "rec/probe.jsonl"
    ]
  },
  "pairs": {
    "chat-send-stream": [
      [
        "chat-working",
        "chat-done"
      ],
      [
        "chat-tool-collapsed",
        "chat-tool-expanded"
      ]
    ],
    "chat-send-stream-live": [
      [
        "chat-working",
        "chat-done"
      ]
    ],
    "chat-send-stream-codex": [
      [
        "codex-chat-working",
        "codex-chat-done"
      ]
    ],
    "chat-send-stream-grok": [
      [
        "grok-chat-working",
        "grok-chat-done"
      ]
    ]
  }
}
```

## Gotchas

A Codex command approval that arrives without an item frame has no transcript tool row.
The codec consumes typed item frames and reads no raw events to invent one.

Replay is not a live permission or tool proof.
Approvals, questions and Stop are covered by their feature pages.
A streamed row may complete before the turn ends; the header remains Working until the result.
Open Terminal and native mouse/keyboard entry points remain manual checks.
Update Status from candidate acceptance.

A Codex command that runs as a later call in the same turn can arrive with no `item/*` frames.
Such a command has no tool row; Veyra renders the frames Codex supplies.
