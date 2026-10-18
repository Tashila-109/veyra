# Restart and recovery

Status: passed, 2026-10-18, ec5469f, none

## Sub-features

- Quit opens a sheet naming busy threads; CancelQuit keeps their turns running.
- ConfirmQuit awaits bounded shutdown; ctl quit dispatches ConfirmQuit directly.
- Orderly quit persists Interrupted and Restarted; a relaunch shows Held and Continue.
- A crash expires pending requests and holds queued work without resending it.
- An idle Quit exits immediately.
- A crashed child shows Interrupted with Continue, settles its task on Continue, and leaves a held parent wake until Continue there.

## How to get to it (user POV)

During a turn, press ⌘Q or choose Quit Veyra.
Read the thread names; Escape cancels and ⌘↵ confirms.
On relaunch, read Held because Veyra closed while this ran, then Continue or Discard queued.
ConfirmQuit waits for engine shutdown, and the app quit hook calls Engine::on_app_quit.
No busy thread means Quit runs ConfirmQuit immediately.

After ConfirmQuit begins, Escape keeps the quit sheet visible until the app exits.

## Driving it with veyra ctl

Run Launch, Doctor and new-thread first, with fake CLI only.
The final section adds a child task through the production relay, then crashes and relaunches the owned app.
Its post-relaunch marker check uses Cleanup's exiting-process retry and chooses a separate thirty-second window to let session-end hooks drain; remaining unreadable or marked pids fail the check without signalling unrecorded groups.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
assert LANE == 'fake'
def owned_groups_gone(groups):
    end = time.monotonic()+5
    while time.monotonic()<end:
        alive = []
        for group in groups:
            try: os.killpg(group,0); alive.append(group)
            except ProcessLookupError: pass
        if not alive: return
        time.sleep(.05)
    raise AssertionError(('owned groups remain',alive))
thread = create('restart-running','Long reply',scenario='long-reply')
ctl('restart-working','wait',thread,'Working')
ctl('restart-queue-text','text','after restart')
action('restart-queue-send','Send')
capture('restart-before-sheet')
ctl('restart-quit-key','keys','secondary-q')
sheet = poll('restart-sheet',lambda d: d['quit_sheet']['open'])
assert sheet['quit_sheet']['threads'] and sheet['selected_thread']['status']=='Working'
assert sheet['selected_thread']['title'] in sheet['quit_sheet']['threads']
capture('restart-quit-sheet')
# Compare a sidebar pixel outside the sheet, using the platform's PNG reader.
checker = PROOF/'quit-overlay.swift'
checker.write_text("""import AppKit
import Foundation
let paths = Array(CommandLine.arguments.dropFirst())
let before = NSBitmapImageRep(data: try Data(contentsOf: URL(fileURLWithPath: paths[0])))!
let after = NSBitmapImageRep(data: try Data(contentsOf: URL(fileURLWithPath: paths[1])))!
let x = before.pixelsWide / 64
let y = before.pixelsHigh / 2
if !before.colorAt(x: x, y: y)!.isEqual(after.colorAt(x: x, y: y)!) {
    print("FAIL quit sheet hid the sidebar")
    exit(1)
}
print("PASS quit sheet preserves the sidebar")
""")
checked = subprocess.run(['swift',str(checker),str(PROOF/'restart-before-sheet.png'),
    str(PROOF/'restart-quit-sheet.png')])
assert checked.returncode == 0, 'quit sheet hid the sidebar'

ctl('restart-cancel-key','keys','escape')
kept = poll('restart-cancelled',lambda d: not d['quit_sheet']['open'])
assert kept['selected_thread']['status']=='Working'
action('restart-quit-action','Quit')
poll('restart-sheet-again',lambda d: d['quit_sheet']['open'])
pid = identity()
groups = [int(x) for x in sql('select live_pgid from threads where live_pgid is not null').splitlines()]
ctl('restart-confirm-key','keys','secondary-enter')
wait_gone(pid)
owned_groups_gone(groups)
start('restart-orderly')
poll('restart-ready',lambda d: d['providers'][0]['readiness']=='ready')
action('restart-select','SelectThread',{'thread':thread})
held = poll('restart-held',lambda d: d['selected_thread']['id']==thread
    and d['selected_thread']['status']=='Held')
assert held['selected_thread']['wait_reason']=='Held because Veyra closed while this ran'
assert any(item['outcome'] and 'Veyra closed while this turn ran' in item['outcome'] for item in held['transcript']['items'])
assert sql("select count(*) from turns where thread_id='"+thread+"' and json_type(outcome_json,'$.Interrupted') is not null")=="1"
assert sql("select count(*) from turns where thread_id='"+thread+"' and state='queued'")=='1'
capture('restart-held')
stage('ok-reply')
action('restart-continue','Continue')
ctl('restart-continued','wait',thread,'Done')
assert sql("select count(*) from turns where thread_id='"+thread+"' and state='queued'")=='0'
pending = create('restart-pending','touch pending',scenario='approval-allow')
poll('restart-pending-dock',lambda d: d['composer']['dock'] is not None)
pid = identity()
groups = [int(x) for x in sql('select live_pgid from threads where live_pgid is not null').splitlines()]
ctl('restart-direct-ctl','quit')
wait_gone(pid)
owned_groups_gone(groups)
assert sql("select json_extract(decision_json,'$') from requests where thread_id='"+pending+"'")=='Veyra is closing'
start('restart-after-ctl')
poll('restart-ready-again',lambda d: d['providers'][0]['readiness']=='ready')
action('restart-select-pending','SelectThread',{'thread':pending})
expired = poll('restart-expired',lambda d: d['selected_thread']['id']==pending
    and d['selected_thread']['status']=='Held')
assert expired['composer']['dock'] is None
capture('restart-expired-initial')
capture('restart-expired')
crashed = create('restart-crash','Long reply',scenario='long-reply')
ctl('restart-crash-working','wait',crashed,'Working')
ctl('restart-crash-queue-text','text','queued before crash')
action('restart-crash-queue-send','Send')
pid = identity()
groups = [int(x) for x in sql('select live_pgid from threads where live_pgid is not null').splitlines()]
os.kill(pid,signal.SIGKILL)
wait_gone(pid)
start('restart-after-crash')
owned_groups_gone(groups)
action('restart-select-crashed','SelectThread',{'thread':crashed})
recovered = poll('restart-crash-held',lambda d: d['selected_thread']['id']==crashed
    and d['selected_thread']['status']=='Held')
assert recovered['selected_thread']['wait_reason']=='Held because Veyra closed while this ran'
assert sql("select count(*) from turns where thread_id='"+crashed+"' and state='queued'")=='1'
capture('restart-crash-held')
assert sql("select count(*) from threads where live_pid is not null or live_pgid is not null") == '0'
action('restart-discard','DiscardQueued')
pid = identity()
action('restart-idle-quit','Quit')
wait_gone(pid)
start('restart-final-idle')
poll('restart-family-ready',lambda d:d['providers'][0]['readiness']=='ready')
for row in ctl('restart-earlier-rows','dump')['sidebar']['rows']:
    if row['kind']=='thread': action('restart-archive-earlier-'+row['id'],'Archive',{'thread':row['id']})
poll('restart-earlier-hidden',lambda d:all(row['kind']!='thread' for row in d['sidebar']['rows']))
root,child,task = create_family('child recovery',child='running')
poll('restart-child-running',lambda d:d['selected_thread']['status']=='Done'
    and d['transcript']['family_card']['rows'][0]['status']=='Working')
select('restart-child-before-crash',child)
pid = identity()
groups = [int(x) for x in sql('select live_pgid from threads where live_pgid is not null').splitlines()]
os.kill(pid,signal.SIGKILL)
wait_gone(pid)
link_fixture('t1-s1.jsonl',fixture_source([json.loads(line) for line in (SOURCE/'parent-wake.jsonl').read_text().splitlines()]))
start('restart-child-crash')
owned_groups_gone(groups)
started = time.monotonic()
while True:
    markers = marked_processes({pid}, retry_exiting=True)
    if not markers or time.monotonic()-started >= 30: break
    time.sleep(.05)
save('restart-child-marker-drain', {'remaining':len(markers),
    'elapsed_seconds':time.monotonic()-started})
assert not markers, f'{len(markers)} marked processes remain after the thirty-second restart drain: {markers}'
assert not list((DATA/'run').glob('*/mcp.json'))
select('restart-child-interrupted',child)
interrupted = poll('restart-child-interrupted',lambda d:d['selected_thread']['status']=='Interrupted')
assert interrupted['selected_thread']['hold']=='restarted'
assert sql("select state from tasks where id='"+task+"'")=='running'
capture('restart-child-interrupted')
action('restart-child-continue','Continue')
select('restart-child-parent-held',root)
held = poll('restart-child-parent-held',lambda d:d['selected_thread']['status']=='Held' and pending_wake(root))
assert held['selected_thread']['hold']=='child_report'
assert sql("select state from tasks where id='"+task+"'")=='settled'
assert sql("select json_type(outcome_json,'$.Interrupted') from tasks where id='"+task+"'")=='object'
capture('restart-child-parent-held')
action('restart-child-parent-continue','Continue')
family_delivered('restart-child-delivered',root,task)
assert sql("select delivery from tasks where id='"+task+"'")=='delivered'
capture('restart-child-delivered')
poll('restart-child-idle',lambda d:sql('select count(*) from threads where live_pgid is not null')=='0')
action('restart-archive-recovered','Archive',{'thread':root})
poll('restart-archive-hidden',lambda d:all(row.get('id') not in [root,child] for row in d['sidebar']['rows']))
restart('restart-final-family-idle')
page_done('restart-recovery')
print('PASS quit sheet, Cancel, ConfirmQuit, ctl quit, Interrupted/Restarted holds, crash and idle quit')
PY
```

## Observable success

`restart-sheet.json` and its PNG show quit_sheet.open and the busy thread title.
`restart-cancelled.json` remains Working.
The process checks show every owned provider group gone before orderly relaunch.
`restart-held.json`, its PNG and SQLite show Interrupted, Held with Restarted and one queued turn.
`restart-expired.json` has no dock; SQLite retains Veyra is closing.
`restart-crash-held.json` and its PNG show the same hold after a crash, without starting the queued turn.
`restart-child-interrupted.png` shows the recovered child and Continue, `restart-child-parent-held.png` shows its pending held wake, and `restart-child-delivered.json` agrees with SQLite delivered.
Both Archive and ArchiveKeepChildren and the top-level tasks key are named here; the archive choices are exercised on multi-agent.

## Gotchas

Normal startup sweeps same-user marked orphans across data folders.
Verification's debug-only VEYRA_REAP_PIDS filter admits only provider pids saved by this run.
The harness only signals the pids and groups it started.
The child sequence also checks stale run configs are removed and the old app's marker count is zero.
Archive and ArchiveKeepChildren are the two actions behind Archive and Archive, keep child threads; the multi-agent page drives both.
The top-level tasks dump key carries child task state and Pending or Delivered, while sidebar, selected_thread and transcript carry lineage, breadcrumbs and family controls.
`scripts/bundle-app.sh`, process inspection and `pmset` run only on macOS.
Update Status from candidate acceptance.
