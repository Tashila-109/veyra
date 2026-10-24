# Stop and interrupt

Status: passed, 2026-10-24, 90327b9, live lane not rerun since RUN 8d22e59d at 2b45067 (fake CLI RUN b4c41aec), 942c63ba87471b85b3edfa4dcbed3f6b0ee8257dea6968a9f242fe68ff353348
Covers: crates/veyra/src/ui/notify.rs crates/engine/src/provider/** crates/engine/tests/fixtures/claude/**

## Sub-features

- Stop interrupts a running turn and retains queued messages under Continue.
- DiscardQueued drops queued user turns and clears the hold.
- StopThread stops the selected sidebar thread.
- Ctrl+C in VeyraComposer keeps typed text; stopped requests expire.
- awake follows running and background work, with pmset evidence.
- window_active gates notifications.posted on family status changes.

## How to get to it (user POV)

An empty running composer shows Stop and its literal ⌃C chord.
Text sent during work queues and shows the engine's wait_reason.
Stop holds those messages with a reason, Continue and Discard queued.
Continue is ⌘⇧↵; Discard queued is ⌘⇧⌫.
Each sidebar row offers Stop thread through veyra::StopThread.
When Veyra is inactive, a Needs you, Done or Failed family posts a title-and-status banner.

## Driving it with veyra ctl

Run Launch and Doctor first, using the fake lane; the section calls `prepare()`.
The background proof lists work before result and keeps the agent task pending after result.
Those are proof-copy wires; the committed fixture is unchanged.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
prepare()
assert LANE == 'fake'
thread = create('stop','A long reply',scenario='long-reply')
working = poll('stop-working',lambda d: d['composer']['stop_visible'] and d['awake'])
capture('stop-working')
(PROOF/'awake-running-pmset.txt').write_text(subprocess.check_output(['pmset','-g','assertions'],text=True))
assert 'caffeinate' in (PROOF/'awake-running-pmset.txt').read_text()
start_time = time.monotonic()
action('stop-first','Stop')
stopped = ctl('stop-interrupted','wait',thread,'Interrupted','5')
assert time.monotonic()-start_time < 5
assert any('Stopped by you' in (item.get('outcome') or '') for item in stopped['transcript']['items'])
assert not stopped['composer']['discard_queued_visible']
poll('stop-idle-awake',lambda d: not d['awake'])
(PROOF/'awake-ended-pmset.txt').write_text(subprocess.check_output(['pmset','-g','assertions'],text=True))
thread = create('stop-queue','A later long reply',scenario='long-reply')
ctl('stop-queue-working','wait',thread,'Working')
for text in ['queued one','queued two']:
    ctl('stop-queue-text-'+text,'text',text)
    ctl('stop-queue-send-'+text,'keys','secondary-enter')
    poll('stop-queue-cleared-'+text,lambda d: d['composer_text']=='')
queued = ctl('stop-queue-before','dump')
assert [item['wait_reason'] for item in queued['transcript']['items'] if item['kind']=='user'][-2:] == [
    'Waits for the current turn','Waits for the current turn']
action('stop-queue-stop','Stop')
held = ctl('stop-queue-held','wait',thread,'Held')
assert held['selected_thread']['wait_reason']=='Held because you pressed Stop'
assert held['composer']['discard_queued_visible']
capture('stop-held')
assert sql("select count(*) from turns where thread_id='"+thread+"' and state='queued'")=='2'
stage('ok-reply',slow=True,turns=2)
ctl('stop-continue-key','keys','secondary-shift-enter')
continued = poll('stop-continued',lambda d: sql("select count(*) from turns where thread_id='"+thread+"' and state='ended'")=='3'
    and d['selected_thread']['status']=='Done')
assert continued['selected_thread']['hold'] is None
thread = create('stop-discard','Discard stale text',scenario='long-reply')
ctl('stop-discard-working','wait',thread,'Working')
for text in ['stale one','stale two']:
    ctl('stop-discard-text-'+text,'text',text)
    ctl('stop-discard-send-'+text,'keys','secondary-enter')
    poll('stop-discard-clear-'+text,lambda d: d['composer_text']=='')
ctl('stop-correction','text','corrected text')
ctl('stop-control-c','keys','ctrl-c')
held = ctl('stop-discard-held','wait',thread,'Held')
assert held['composer_text']=='corrected text'
ctl('stop-discard-key','keys','secondary-shift-backspace')
dropped = poll('stop-discarded',lambda d: d['selected_thread']['hold'] is None
    and sql("select count(*) from turns where thread_id='"+thread+"' and state='queued'")=='0')
assert dropped['composer_text']=='corrected text'
stage('ok-reply',slow=True)
ctl('stop-correction-send','keys','secondary-enter')
poll('stop-correction-done',lambda d: d['selected_thread']['status']=='Done'
    and sql("select count(*) from turns where thread_id='"+thread+"'")=='4')
assert sql("select count(*) from turns where thread_id='"+thread+"' and outcome_json='\"Completed\"'")=='1'
thread = create('stop-request','Stop a pending approval',scenario='stop-pending-approval')
poll('stop-request-open',lambda d: d['composer']['dock'] is not None and d['composer']['dock']['count']==2)
action('stop-request-stop','Stop')
expired = ctl('stop-expired','wait',thread,'Interrupted')
assert expired['composer']['dock'] is None
assert sql("select count(*) from requests where thread_id='"+thread+"' and state='expired'")=='2'
capture('stop-expired')
thread = create('stop-sidebar','Stop through the sidebar',scenario='long-reply')
ctl('stop-sidebar-working','wait',thread,'Working')
action('stop-sidebar-row','StopThread',{'thread':thread})
ctl('stop-sidebar-interrupted','wait',thread,'Interrupted')
poll('stop-sessions-ended',lambda d: not d['awake'] and sql('select count(*) from threads where live_pgid is not null')=='0')
# Fake approval banner decision while the owned backdrop is frontmost.
import plistlib
backdrop_bundle = DATA/'notifications-backdrop.app'
backdrop_executable = backdrop_bundle/'Contents/MacOS/backdrop'
backdrop_executable.parent.mkdir(parents=True)
shutil.copy2(PROOF/'backdrop',backdrop_executable)
backdrop_pid_file = DATA/'notifications-backdrop.pid'
with (backdrop_bundle/'Contents/Info.plist').open('wb') as file:
    plistlib.dump({'CFBundleIdentifier':'dev.veyra.verify.backdrop.'+RUN,
        'CFBundleName':'Veyra verification backdrop','CFBundleExecutable':'backdrop',
        'CFBundlePackageType':'APPL','LSUIElement':True,
        'VeyraBackdropPidFile':str(backdrop_pid_file)},file)
backdrop_output = PROOF/'notifications-backdrop.stdout'
def backdrop_command(label,command):
    result = subprocess.run(command,capture_output=True,text=True)
    (PROOF/(label+'.stdout')).write_text(result.stdout)
    (PROOF/(label+'.stderr')).write_text(result.stderr)
    save(label+'-command',{'command':command,'exit':result.returncode})
    assert result.returncode==0, f'owned backdrop helper failed; see {label}.stderr'
    return result
backdrop_command('notifications-backdrop-sign',['codesign','--force','--sign','-',str(backdrop_bundle)])
backdrop_command('notifications-backdrop-open',['open','-n','--stdout',str(backdrop_output),
    '--stderr',str(PROOF/'notifications-backdrop.stderr'),str(backdrop_bundle)])
end = time.monotonic()+10
while not backdrop_pid_file.exists():
    assert time.monotonic()<end, 'owned backdrop did not write its pid'
    time.sleep(.05)
backdrop_pid = int(backdrop_pid_file.read_text())
backdrop_path = verify_owned_backdrop(backdrop_pid,backdrop_executable)
save('notifications-backdrop-ownership',{'pid':backdrop_pid,'executable':str(backdrop_path),
    'bundle':str(backdrop_bundle.resolve()),'pid_file':str(backdrop_pid_file.resolve())})
(PROOF/'notifications-backdrop-pid').write_text(str(backdrop_pid)+'\n')
with (PROOF/'backdrop-owned-pids').open('a') as file: file.write(str(backdrop_pid)+'\n')
end = time.monotonic()+10
while not backdrop_output.exists() or 'backdrop ready ' not in backdrop_output.read_text():
    verify_owned_backdrop(backdrop_pid,backdrop_executable)
    assert time.monotonic()<end, 'owned backdrop was not ready'
    time.sleep(.05)
def activate_owned_backdrop(label):
    verify_owned_backdrop(backdrop_pid,backdrop_executable)
    reopen_owned_backdrop(backdrop_pid,backdrop_executable,
        lambda: backdrop_command(label+'-open',['open',str(backdrop_bundle)]))
    source = ('import AppKit\n'
        f'let pid: Int32 = {backdrop_pid}\n'
        'let end = Date().addingTimeInterval(5)\n'
        'while NSWorkspace.shared.frontmostApplication?.processIdentifier != pid && Date() < end { '
        'RunLoop.current.run(until: Date().addingTimeInterval(0.05)) }\n'
        'guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { exit(2) }\n'
        'print(pid)\n')
    result = backdrop_command(label,['swift','-e',source])
    assert int(result.stdout.strip())==backdrop_pid
    save(label,{'frontmost_pid':backdrop_pid,'owned_backdrop_pid':backdrop_pid})
thread = create('notifications-approval','A background approval',scenario='approval-deny')
poll('notifications-approval-open',lambda d: d['composer']['dock'] is not None)
action('notifications-approval-deny','Deny')
ctl('notifications-approval-done','wait',thread,'Done')
records = [json.loads(line) for line in (SOURCE/'approval-deny.jsonl').read_text().splitlines()]
records[4]['after_ms'] = 3000
stage_records(records)
thread = create('notifications-away','A delayed approval',scenario=None)
action('notifications-away-activate','ActivateWindow')
active = poll('notifications-active',lambda d: d['window_active'])
posted = active['notifications']['posted']
activate_owned_backdrop('notifications-approval-backdrop-frontmost')
poll('notifications-inactive',lambda d: not d['window_active'])
needs = poll('notifications-needs-you',lambda d: d['selected_thread']['status']=='NeedsYou')
assert needs['notifications']['posted']==posted+1
stage('ok-reply')
action('notifications-answer-away','Deny',{'request':needs['composer']['dock']['request']['id']})
done = ctl('notifications-done-away','wait',thread,'Done')
assert done['notifications']['posted']==posted+2
old_banner_tag = thread
action('notifications-front','ActivateWindow')
poll('notifications-front-active',lambda d: d['window_active'])
posted = ctl('notifications-front-before','dump')['notifications']['posted']
thread = create('notifications-front-reply','reply with the word ok',slow=True)
ctl('notifications-front-done','wait',thread,'Done')
assert ctl('notifications-front-after','dump')['notifications']['posted']==posted
# The older tag still routes through the native response handler.
action('notifications-older-response','DebugNotificationResponse',{'tag':old_banner_tag})
selected = poll('notifications-older-selected',lambda d: d['selected_thread'] is not None
    and d['selected_thread']['id']==old_banner_tag and d['window_active'])
capture('notifications-older-response')
records = [json.loads(line) for line in (SOURCE/'background-after-result.jsonl').read_text().splitlines()]
agent = records.pop(5)
agent['after_ms'] = 0
records.insert(4,agent)
records[5]['after_ms'] = 3000
records[6]['after_ms'] = 3000
stage_records(records)
thread = create('notifications-background','A turn with background work',scenario=None)
poll('notifications-background-working',lambda d: d['selected_thread']['status']=='Working' and d['awake'])
posted = ctl('notifications-background-before','dump')['notifications']['posted']
activate_owned_backdrop('notifications-background-backdrop-frontmost')
poll('notifications-background-inactive',lambda d: not d['window_active'])
foreground_ended('notifications-background',thread)
busy = ctl('notifications-background','dump')
assert busy['selected_thread']['status']=='Working' and busy['awake']
assert busy['notifications']['posted']==posted
complete = ctl('notifications-background-done','wait',thread,'Done')
assert not complete['awake'] and complete['notifications']['posted']==posted+1
action('notifications-final-front','ActivateWindow')
poll('notifications-final-active',lambda d: d['window_active'])
capture('notifications-background-done')
poll('stop-final-idle',lambda d: sql('select count(*) from threads where live_pgid is not null')=='0')
page_done('stop-interrupt')
print('PASS Stop, held queue, Continue, Discard queued, Ctrl+C text, expiry, StopThread, awake and banners')
PY
```

## Observable success

`stop-held.png` and its dump show the held reason and two queued messages.
`stop-discarded.json` clears the hold, drops both stale turns and keeps corrected text.
`stop-expired.json` and SQLite show both requests expired.
The two pmset files retain OS assertion evidence around awake.
`notifications-needs-you.json`, `notifications-done-away.json` and the frontmost check prove the banner decision.
`notifications-background.json` proves no Done banner at foreground end while work remains.
`processes`, `perf.commits`, `perf.actor_max_ms`, `perf.actor_busy_ms`, `perf.mailbox_free`, `perf.repaints` and `perf.max_batch_events` remain available to an authorized performance run.
The debug-only DebugStallUi action remains available for that lane.

### Dump fields

`selected_thread.hold` and `wait_reason` describe held work.
`composer.discard_queued_visible` reports the held queue control.

### Evidence contract

```json
{
  "required": {
    "stop-interrupt": [
      "stop-working.png",
      "stop-held.png",
      "stop-discarded.json",
      "stop-expired.json",
      "notifications-background.json",
      "notifications-older-selected.json",
      "notifications-older-response.png",
      "notifications-approval-backdrop-frontmost.json",
      "notifications-background-backdrop-frontmost.json",
      "notifications-inactive.json",
      "notifications-background-inactive.json"
    ]
  },
  "pairs": {}
}
```

## Gotchas

Stop keeps queued messages under a hold.
Stop thread discards its queued messages and stops a root’s children before stopping the root.

`pmset` and `scripts/bundle-app.sh` run only on macOS.
Posting counts gpui requests even outside an app bundle; OS delivery is a separate manual banner check.
The debug-only veyra::DebugNotificationResponse action takes {"tag":"ROOT_ID"} and calls the native banner response handler.
`notifications-older-selected.json` and its PNG prove an older family tag activates and selects that family.
OS banner delivery and the macOS permission setting require a manual check from target/Veyra.app.
scripts/bundle-app.sh signs the bundle ad hoc, because macOS refuses notification authorization to an unsigned bundle (UNErrorDomain error 1) and shows no permission prompt.
The first banner from a new signature asks the operator to allow Veyra's notifications.
Use only this run's recorded pids and groups during cleanup.
Update Status from candidate acceptance.
