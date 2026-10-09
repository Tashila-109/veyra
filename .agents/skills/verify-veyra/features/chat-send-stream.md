# Chat send and stream

Status: STATE, DATE, COMMIT, MISSING

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
    reply = [item for item in done['transcript'] if item['kind']=='assistant']
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
    and not item['done'] for item in d['transcript']))
assert stream['selected_thread']['status'] == 'Working'
assert stream['selected_thread']['header_status'].startswith('Working · ')
assert next(item for item in stream['transcript'] if item['kind']=='assistant')['delta_count'] >= 1
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
assert [item['text'] for item in done['transcript'] if item['kind']=='user'] == ['reply with the word ok']
assert [(item['text'],item['done']) for item in done['transcript'] if item['kind']=='assistant'] == [('ok',True)]
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
assert [(item['text'],item['done']) for item in retry['transcript'] if item['kind']=='assistant'][-1] == ('ok',True)
tool_thread = create('chat-tool','Run echo veyra-probe',scenario='bash-echo')
tools = ctl('chat-tool-done','wait',tool_thread,'Done')
tool = next(item for item in tools['transcript'] if item['kind']=='tool')
assert tool['done'] and tool['text'] == 'veyra-probe' and not tool['expanded']
capture('chat-tool-collapsed')
action('chat-tool-expand','ToggleItem',{'item':tool['id']})
assert next(item for item in ctl('chat-tool-expanded','dump')['transcript'] if item['id']==tool['id'])['expanded']
capture('chat-tool-expanded')
action('chat-tool-collapse','ToggleItem',{'item':tool['id']})
collapsed = ctl('chat-tool-collapsed-action','dump')['transcript']
assert not next(item for item in collapsed if item['id']==tool['id'])['expanded']
latest = next(item['id'] for item in reversed(tools['transcript']) if item['kind'] in ['tool','reasoning'])
action('chat-detail-default-expand','ToggleItem')
assert next(item for item in ctl('chat-detail-default-expanded','dump')['transcript'] if item['id']==latest)['expanded']
action('chat-detail-default-collapse','ToggleItem')
for expanded in [True,False]:
    ctl('chat-detail-key-'+str(expanded),'keys','secondary-shift-i')
    items = ctl('chat-detail-key-after-'+str(expanded),'dump')['transcript']
    assert next(item for item in items if item['id']==latest)['expanded'] == expanded
background = create('chat-background','reply with the word ok',scenario='background-tasks')
busy = background_working('chat-background-working',background)
assert busy['selected_thread']['header_status'].startswith('Working · ')
ambient = ctl('chat-background-done','wait',background,'Done','3')
assert ambient['selected_thread']['status']=='Done'
assert sql("select live_pgid is not null from threads where id='"+background+"'")=='1'
assert sql("select count(*) from turns where thread_id='"+background+"'")=='1'
provider = create('chat-provider','reply with the word ok',scenario='provider-turn-after-background')
provider_done = poll('chat-provider-done',lambda d: d['selected_thread']['id']==provider
    and d['selected_thread']['status']=='Done'
    and sql("select count(*) from turns where thread_id='"+provider+"' and state='ended'")=='2')
assert sql("select group_concat(json_extract(outcome_json,'$')) from turns where thread_id='"
    +provider+"'") == 'Completed,Completed'
poll('chat-sessions-ended',lambda d: sql('select count(*) from threads where live_pgid is not null') == '0')
page_done('chat-send-stream')
print('PASS chat streamed o → ok, Working → Done; crash cause/tail; Retry; wait exits 0/1/2/3')
PY
```

## Observable success

- `chat-working.json` has Working, assistant `o` with `done: false`, a native id and cleared composer; inspect its PNG for the caret and Working header.
- `chat-done.json` has one user prompt and exactly one completed assistant `ok`, Done in the selected thread and sidebar, and an increased applied-batch counter.
- `sql.log` keeps the SQLite `outcome_json` Completed and stored assistant `ok|1` checks.
- `chat-timer-advanced.json` proves Working time advances above zero; Done also has positive elapsed seconds.
- Streamed assistant items have a positive `delta_count`.
- `chat-background-working.json` stays Working after its foreground turn ends and `wait Done 1` times out.
- The ambient-only interval shows Done while the session remains alive; `chat-provider-done.json` shows Done after the separate provider turn completes.
- `chat-failed.json` has `view: failed_thread`, Failed and the crash cause with both fixture stderr lines; its PNG shows the cause, next step, Retry and Open Terminal.
- `chat-retried.json` has Done with the retried completed `ok`.
- `drive.log` records wait exit 0 for each state, 1 for the zero-second timeout, 2 for bad usage and 3 for a missing thread.
- `chat-tool-done.json` has a completed collapsed tool with captured output `veyra-probe`; the action and key assert its expanded state, with collapsed/expanded PNGs.
- Thread selection and transcript loading use the snapshot fence, not a UI database read.

## Gotchas

Replay is not a live permission or tool proof.
No approvals, questions or Stop UI are implemented.
A streamed row may complete before the turn ends; the header remains Working until the result.
Open Terminal and native mouse/keyboard entry points remain manual checks.
The lead fills the Status values after its gate.
