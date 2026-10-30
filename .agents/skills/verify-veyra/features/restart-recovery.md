# Restart and recovery

Status: passed, 2026-10-26, bd8a083, live lane not rerun since RUN 8d22e59d at 2b45067 (fake CLI RUN 8577dad2), 04a7915fa1161aff46917715a373df5e42fd4cf96074c140cd91d718806bc615
Covers: crates/veyra/src/ui/drawer.rs crates/veyra/src/ui/header.rs crates/engine/src/data_dir.rs crates/engine/src/mcp.rs crates/engine/src/tools.rs crates/veyra/src/relay.rs crates/engine/src/provider/** crates/engine/tests/fixtures/claude/**

## Sub-features

- Quit opens a sheet naming busy threads; CancelQuit keeps their turns running.
- ConfirmQuit awaits bounded shutdown; ctl quit dispatches ConfirmQuit directly.
- Orderly quit persists Interrupted and Restarted; a relaunch shows Held and Continue.
- A crash expires pending requests and holds queued work without resending it.
- An idle Quit exits immediately.
- A stored Terminal view opens in chat without a view-specific hold and preserves its stored terminal span.
- A crashed child shows Interrupted with Continue, settles its task on Continue, and leaves a held parent wake until Continue there.
- The `ctl quit` and crash relaunches keep the project row, schema 4, WAL, the eight tables, the stored default mode and the database inode.

## How to get to it (user POV)

During a turn, press ⌘Q or choose Quit Veyra.
Read the thread names; Escape cancels and ⌘↵ confirms.
On relaunch, read Held because Veyra closed while this ran, then Continue or Discard queued.
ConfirmQuit waits for engine shutdown, and the app quit hook calls Engine::on_app_quit.
No busy thread means Quit runs ConfirmQuit immediately.

After ConfirmQuit begins, Escape keeps the quit sheet visible until the app exits.

## Driving it with veyra ctl

Every `start()` below installs a fresh uniquely labelled one-shot yabai rule when available, verifies that the owned window is unmanaged and floating, and restores the proved 960×740 geometry inside its display's visible frame. The run's rule ledger covers every orderly and crash relaunch; Cleanup removes any unconsumed label.

Run Launch and Doctor first, with fake CLI only; the section calls `prepare()`.
The final section adds a child task through the production relay, then crashes and relaunches the owned app.
The `ctl quit` and crash relaunches also run app-launch's database checks against this page's starting project row and inode.
Its post-relaunch marker check uses Cleanup's exiting-process retry and chooses a separate thirty-second window to let session-end hooks drain; remaining unreadable or marked pids fail the check without signalling unrecorded groups.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
capture = capture_content
prepare()
assert LANE == 'fake'
project_row = sql('select id,path,name from projects order by id')
inode = DB.stat().st_ino
mode = sql('select default_mode from settings')
def database_kept(label):
    database(label,mode)
    assert sql('select id,path,name from projects order by id') == project_row, label
    assert DB.stat().st_ino == inode, label
    return {'label':label,'mode':mode,'inode':inode,'project':project_row}
database_rows = []
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
assert any(sheet['selected_thread']['title'] in line for line in sheet['quit_sheet']['threads'])
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
let equal = before.colorAt(x: x, y: y)!.isEqual(after.colorAt(x: x, y: y)!)
if equal != (paths[2] == "translucent") {
    print("FAIL quit sheet sidebar scrim")
    exit(1)
}
print("PASS quit sheet sidebar scrim")
""")

ctl('restart-cancel-key','keys','escape')
kept = poll('restart-cancelled',lambda d: not d['quit_sheet']['open'])
assert kept['selected_thread']['status']=='Working'
action('restart-quit-action','Quit')
poll('restart-sheet-again',lambda d: d['quit_sheet']['open'])
pid = owned_instance_pid()
groups = [int(x) for x in sql('select live_pgid from threads where live_pgid is not null').splitlines()]
ctl('restart-confirm-key','keys','secondary-enter')
wait_gone(pid)
owned_groups_gone(groups)
checked = subprocess.run(['swift',str(checker),str(PROOF/'restart-before-sheet.png'),
    str(PROOF/'restart-quit-sheet.png'),sheet['look']['background']])
assert checked.returncode == 0, 'quit sheet hid the sidebar'

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
pid = owned_instance_pid()
groups = [int(x) for x in sql('select live_pgid from threads where live_pgid is not null').splitlines()]
ctl('restart-direct-ctl','quit')
wait_gone(pid)
owned_groups_gone(groups)
assert sql("select json_extract(decision_json,'$') from requests where thread_id='"+pending+"'")=='Veyra is closing'
start('restart-after-ctl')
database_rows.append(database_kept('restart-after-ctl'))
poll('restart-ready-again',lambda d: d['providers'][0]['readiness']=='ready')
action('restart-select-pending','SelectThread',{'thread':pending})
expired = poll('restart-expired',lambda d: d['selected_thread']['id']==pending
    and d['selected_thread']['status']=='Held')
assert expired['composer']['dock'] is None
capture('restart-expired-initial')
capture('restart-expired')
records = [json.loads(line) for line in (SOURCE/'bash-echo.jsonl').read_text().splitlines()]
for record in records:
    if record.get('line',{}).get('type')=='result': record['after_ms']=60000
stage_records(records)
crashed = create('restart-crash','Store the result before crashing',scenario=None)
result = poll('restart-stored-result-before-crash',lambda d:any(item['kind']=='tool' and item['done']
    and item['text']=='veyra-probe' for item in d['transcript']['items']))
stored_tool = next(item for item in result['transcript']['items'] if item['kind']=='tool')
assert stored_tool['tool_status']=='Done'
ctl('restart-crash-working','wait',crashed,'Working')
ctl('restart-crash-queue-text','text','queued before crash')
action('restart-crash-queue-send','Send')
pid = owned_instance_pid()
groups = [int(x) for x in sql('select live_pgid from threads where live_pgid is not null').splitlines()]
os.kill(pid,signal.SIGKILL)
wait_gone(pid)
start('restart-after-crash')
owned_groups_gone(groups)
database_rows.append(database_kept('restart-after-crash'))
action('restart-select-crashed','SelectThread',{'thread':crashed})
recovered = poll('restart-crash-held',lambda d: d['selected_thread']['id']==crashed
    and d['selected_thread']['status']=='Held')
assert recovered['selected_thread']['wait_reason']=='Held because Veyra closed while this ran'
assert sql("select count(*) from turns where thread_id='"+crashed+"' and state='queued'")=='1'
assert next(item for item in recovered['transcript']['items'] if item['id']==stored_tool['id'])['body']['Tool']['result']==stored_tool['body']['Tool']['result']
assert next(item for item in recovered['transcript']['items'] if item['id']==stored_tool['id'])['text']=='veyra-probe'
assert all(not re.search(r' · [0-9]+s$',item['outcome']) for item in recovered['transcript']['items'] if item['outcome'])
capture('restart-crash-held')
assert sql("select count(*) from threads where live_pid is not null or live_pgid is not null") == '0'
action('restart-discard','DiscardQueued')
pid = owned_instance_pid()
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
action('restart-family-quit','Quit')
family_sheet = poll('restart-family-quit-sheet',lambda d:d['quit_sheet']['open'])
project = sql("select name from projects where id=(select project_id from threads where id='"+child+"')")
root_title = sql("select title from threads where id='"+root+"'")
child_title = sql("select title from threads where id='"+child+"'")
assert any(project+' · '+root_title+' / '+child_title==line for line in family_sheet['quit_sheet']['threads'])
capture('restart-family-quit-sheet')
ctl('restart-family-quit-cancel','keys','escape')
select('restart-child-before-crash',child)
pid = owned_instance_pid()
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
save('restart-database-kept',database_rows)
stored = create('restart-stored-terminal-thread','reply with the word ok')
ctl('restart-stored-terminal-done','wait',stored,'Done')
stop('before-stored-terminal')
import uuid
with sqlite3.connect(DB) as db:
    turn = db.execute("select id from turns where thread_id=? and state='ended' order by ordinal desc limit 1",(stored,)).fetchone()[0]
    seq = db.execute('select coalesce(max(seq),0)+1 from items where thread_id=?',(stored,)).fetchone()[0]
    db.execute("update threads set view='terminal',import_pending=1,native_cursor='legacy' where id=?",(stored,))
    db.execute('insert into items(id,thread_id,turn_id,seq,kind,body_json,done) values(?,?,?,?,?,?,1)',
        (str(uuid.uuid4()),stored,turn,seq,'terminal_span',json.dumps({'TerminalSpan':{'text':'Stored shell transcript'}})))
start('after-stored-terminal')
action('restart-stored-terminal-select','SelectThread',{'thread':stored})
legacy = poll('restart-stored-terminal',lambda d:d['selected_thread'] is not None
    and d['selected_thread']['id']==stored and any(item.get('text')=='Stored shell transcript' for item in d['transcript']['items']))
assert legacy['view']=='chat' and legacy['selected_thread']['hold'] is None
assert sql("select count(*) from threads where id='"+stored+"' and live_pid is not null")=='0'
capture('restart-stored-terminal')
page_done('restart-recovery')
print('PASS quit sheet, Cancel, ConfirmQuit, ctl quit, Interrupted/Restarted holds, crash and idle quit')
PY
```

### Native smoke

This capture runs only in a selected-page smoke; it leaves the page's Status unchanged.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PYSMOKE'
from smoke import drive
drive('restart-recovery')
PYSMOKE
```

## Observable success

Admitted test owners for this page: `semantic_placeholders_and_input_clipping`, `composer_accent_and_unscrolled_header_rule`.
Their class and exact scope are in [proof ownership](../proof-ownership.md); every remaining ink, mark or clipping detail stays a native capture read.

On settled states, the separate `-placeholder` capture moves focus with `root::Tab`, then restores it with `FocusComposer`.
`semantic_placeholders_and_input_clipping` owns the admitted placeholder decisions and bounds.
`composer_accent_and_unscrolled_header_rule` owns the active accent case; `window_active` also gates that case.
The lead reads remaining placeholder ink, inactive borders and native clipping from the captures.

Each retained PNG and its exact dump queue a `*-native-read.json` for the lead.
Vision OCR contributes no verdict.
The admitted ownership and remaining native reads are listed in [proof ownership](../proof-ownership.md).
A queued read is pending until the lead records the capture hash and verdict.

`restart-sheet.json` and its PNG show quit_sheet.open and the busy thread title.
`restart-family-quit-sheet.json` and its PNG name the child's project and root title.
`restart-stored-result-before-crash.json` and `restart-crash-held.json` retain the same successful tool result, and the recovered outcome has no elapsed segment.
`restart-stored-terminal.png` shows the fixture span in its labelled terminal block.
`restart-cancelled.json` remains Working.
The process checks show every owned provider group gone before orderly relaunch.
`restart-held.json`, its PNG and SQLite show Interrupted, Held with Restarted and one queued turn.
`restart-expired.json` has no dock; SQLite retains Veyra is closing.
`restart-crash-held.json` and its PNG show the same hold after a crash, without starting the queued turn.
`restart-child-interrupted.png` shows the recovered child and Continue, `restart-child-parent-held.png` shows its pending held wake, and `restart-child-delivered.json` agrees with SQLite delivered.
`restart-after-ctl-database.txt`, `restart-after-crash-database.txt` and `restart-database-kept.json` hold schema 4, WAL, the eight tables and the stored mode after each relaunch, with the page's starting project row and database inode.
Both Archive and ArchiveKeepChildren and the top-level tasks key are named here; the archive choices are exercised on multi-agent.

### Evidence contract

```json
{
  "required": {
    "restart-recovery": [
      "restart-quit-sheet.png",
      "restart-held.png",
      "restart-crash-held.json",
      "restart-continued.json",
      "restart-child-interrupted.png",
      "restart-child-parent-held.png",
      "restart-child-delivered.json",
      "restart-after-ctl-database.txt",
      "restart-after-crash-database.txt",
      "restart-database-kept.json",
      "restart-stored-terminal.png",
      "restart-family-quit-sheet.png",
      "restart-family-quit-sheet.json",
      "restart-stored-result-before-crash.json"
    ],
    "restart-recovery-smoke": [
      "restart-recovery-smoke.png",
      "restart-recovery-smoke-native-read.json",
      "restart-recovery-smoke-freshness.json"
    ]
  }
}
```

## Gotchas

Normal startup sweeps same-user marked orphans across data folders.
Verification's debug-only VEYRA_REAP_PIDS filter admits only provider pids saved by this run.
The harness only signals the pids and groups it started.
The child sequence also checks stale run configs are removed and the old app's marker count is zero.
Archive and ArchiveKeepChildren are the two actions behind Archive and Archive, keep child threads; the multi-agent page drives both.
The top-level tasks dump key carries child task state and Pending or Delivered, while sidebar, selected_thread and transcript carry lineage, breadcrumbs and family controls.
`scripts/bundle-app.sh`, process inspection and `pmset` run only on macOS.
Update Status from candidate acceptance.
