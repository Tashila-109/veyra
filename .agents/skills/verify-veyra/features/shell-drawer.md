# Shell drawer

Status: passed, 2026-10-23, 0fca537, human login-shell lane not run (fake CLI RUN 613a6eb9), d344b3b1ec533db2832f8c343c6eaa3232e4c12f21a66598e016b1f37af0fded
Covers: crates/veyra/src/ui/drawer.rs crates/veyra/src/ui/terminal_view.rs crates/veyra/src/ui/header.rs crates/veyra/src/ui/actions.rs crates/engine/src/terminal.rs crates/engine/src/lib.rs crates/engine/src/platform/** crates/engine/src/provider/env.rs crates/engine/tests/fixtures/tui/**

## Sub-features

- Each thread keeps its own drawer, height and up to six tabs in memory; no shell row or actor event is written.
- `veyra::ToggleShellDrawer` opens or hides with ⌘J.
  An empty composer gives focus to the drawer; a draft keeps composer focus, text and caret.
  Hiding returns focus to the composer.
- `veyra::NewShellTab`, `veyra::CloseShellTab`, `veyra::NextShellTab` and `veyra::PreviousShellTab` use ⌘T, ⌘W, ⌘⇧] and ⌘⇧[ only with drawer focus.
- A bare prompt closes immediately; a foreground, background or stopped job asks.
  `veyra::ConfirmCloseShellTab` confirms with ⌘↵; `veyra::CancelCloseShellTab` cancels with Escape.
- Output and exits retain focus.
  Hidden output keeps parsing without scheduling paints.
  `veyra::RestartShellTab`, Return, replaces an ended shell in the same tab; a missing folder starts nothing.
- `veyra::PasteShell` and `veyra::CopyShell` bind ⌘V and ⌘C.
  Paste removes ESC, DEL and C0 controls except tab, newline and carriage return in both modes, and adds brackets only when requested.
  `veyra::ScrollShellHistoryUp` and `veyra::ScrollShellHistoryDown` bind ⇧PageUp and ⇧PageDown; the next input returns to the bottom.
  Drag selection and copy use the emulator selection.
- Archive confirms running jobs and closes the reached drawers; Stop thread leaves shells alone.
  Quit names running tabs and their commands.
  A crash hangs up the run's shell groups.
- The active tab supplies `perf.flood` and one 50,000,020-byte smoke measurement.

## How to get to it (user POV)

Open a thread and press ⌘J, or click the header's ⌘J button.
Click the terminal to focus it, then type or use the tab keys.
The top rail resizes the drawer within the space left by chat and composition.
Click the composer to return to the draft.
A running tab's close button or ⌘W opens its confirmation sheet; Return restarts an exited tab.
The shell uses the thread folder and the user's login shell with `-l`, without Veyra credentials or host-terminal identity variables.

## Driving it with veyra ctl

Use the fake lane only.
`VEYRA_DEBUG_SHELL_FIXTURE` points to an owned fixture and starts the adjacent fake CLI in shell mode.
The scripted shell used to load PATH remains owned too; no human shell or dotfile is used.
`veyra::DebugFocusShellDrawer` applies the same focus target as the terminal click, for setting a drive's focus precondition; synthetic window tests separately exercise the click.
It does not prove native pointer delivery.
`veyra::DebugDragShellDrawer` dispatches mouse-down on the rendered top rail, a held-left move and mouse-up through the window; its signed pixel amount grows the drawer upwards.
It exercises the pointer handlers without establishing native pointer delivery.
Every key below uses `keys`, with `focus` asserted before and after by `drawer_keys`.
Draft checks also retain `composer_caret`.
The focus table is: hidden + empty composer opens into drawer; hidden + draft opens while composer stays focused; either shown focus hides into composer; tab keys retain drawer focus, except closing its last tab hides into composer.
Output and restart keep the existing surface focus.
A sheet reports `focus` as `none` because neither terminal nor composer holds focus; Cancel restores its originating surface.
After archive opens the new-thread screen, its composer receives focus; the recipe asserts that settled screen and focus together.
The orderly lifecycle case uses `ctl quit` to exercise shutdown with any owned idle structured sessions too, then verifies their groups and every owned shell pid are gone before the guarded relaunch.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
prepare()
assert LANE == 'fake'
for row in ctl('drawer-prior-work','dump')['sidebar']['rows']:
    if row['kind']=='thread' and row['status']=='Working':
        action('drawer-stop-prior-'+row['id'],'StopThread',{'thread':row['id']})
drawer_fixture()
thread = create('drawer-thread','reply with the word ok')
ctl('drawer-chat-done','wait',thread,'Done')
before = ctl('drawer-chat-baseline','dump')
assert before['focus']=='composer' and before['composer_text']==''
opened = drawer_keys('drawer-open-key','cmd-j','composer','drawer',draft='',caret=0)
opened = poll('drawer-open',lambda d:d['drawer']['terminal'] is not None
    and 'VEYRA_TUI_READY' in ''.join(d['drawer']['terminal']['grid']))
assert opened['view']=='chat' and opened['selected_thread']['native_id']==before['selected_thread']['native_id']
assert len(opened['drawer']['tabs'])==1 and not opened['drawer']['tabs'][0]['running']
first_pid = opened['drawer']['tabs'][0]['pid']
owned_shells = set()
def remember(d):
    owned_shells.update(tab['pid'] for tab in d['drawer']['tabs'] if tab['pid'])
    (PROOF/'shell-pids.txt').write_text('\n'.join(map(str,sorted(owned_shells)))+'\n')
remember(opened)
capture('drawer-open')
capture('drawer-header')
height = opened['drawer']['height']
action('drawer-resize-drag','DebugDragShellDrawer',{'pixels':48})
resized = poll('drawer-resize',lambda d:d['drawer']['height']==height+48)
assert resized['focus']=='drawer'
capture('drawer-resize')
action('drawer-resize-restore','DebugDragShellDrawer',{'pixels':-48})
poll('drawer-resize-restored',lambda d:d['drawer']['height']==height)
for index in range(7):
    drawer_keys('drawer-new-'+str(index),'cmd-t','drawer','drawer')
full = poll('drawer-six-tabs',lambda d:len(d['drawer']['tabs'])==6 and all(tab['pid'] for tab in d['drawer']['tabs']))
remember(full)
active = full['drawer']['active_tab']
previous = drawer_keys('drawer-previous','cmd-shift-[','drawer','drawer')
assert previous['drawer']['active_tab'] != active
following = drawer_keys('drawer-next','cmd-shift-]','drawer','drawer')
assert following['drawer']['active_tab'] == active
for index in range(5): drawer_keys('drawer-close-bare-'+str(index),'cmd-w','drawer','drawer')
poll('drawer-one-tab',lambda d:len(d['drawer']['tabs'])==1)
for name,key,expected in [('return','enter',['0d']),('control','ctrl-c',['03']),('line-start','cmd-left',['01']),
    ('line-end','cmd-right',['05']),('word','alt-left',['1b','62'])]:
    drawer_keys('drawer-byte-'+name,key,'drawer','drawer')
    poll('drawer-byte-'+name+'-echo',lambda d:[line.strip() for line in d['drawer']['terminal']['grid']
        if re.fullmatch('[0-9a-f]{2}',line.strip())][-len(expected):] == expected)
assert ctl('drawer-live-return-pid','dump')['drawer']['tabs'][0]['pid']==first_pid
paste_records = [dict(type='transcript_start',provider='shell',protocol='terminal',cli_version='fixture',
    scenario='shell-paste-record',source='scripted'),dict(type='emit_raw',base64='G1s/MjAwNGg=')]
link_fixture('drawer.jsonl',fixture_source(paste_records))
drawer_keys('drawer-paste-tab','cmd-t','drawer','drawer')
remember(poll('drawer-paste-ready',lambda d:'VEYRA_TUI_READY' in ''.join(d['drawer']['terminal']['grid'])))
pasteboard_helper = PROOF/'pasteboard'
assert pasteboard_helper.is_file()
pasteboard = subprocess.Popen([str(pasteboard_helper),'--plant'],stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
save('drawer-pasteboard-helper',{'pid':pasteboard.pid,'pgid':pasteboard.pid})
try:
    ready = json.loads(pasteboard.stdout.readline())
    assert ready['phase']=='ready', ready
    drawer_keys('drawer-paste','cmd-v','drawer','drawer')
    expected = b'\x1b[200~Q[201~[200~\x1b[201~'.hex()
    poll('drawer-paste-echo',lambda d:''.join(line.strip() for line in d['drawer']['terminal']['grid'])
        .split('PASTE:')[1:] == [expected])
    capture('drawer-paste')
finally:
    pasteboard.stdin.close()
    pasteboard.stdin = None
    output, error = pasteboard.communicate(timeout=10)
    restored = json.loads(output)
    save('drawer-pasteboard-restored',restored)
    assert pasteboard.returncode==0 and restored['phase']=='restored' and restored['matches'], restored
    assert restored['original_restored'] and restored['before']==restored['after'], restored
drawer_keys('drawer-paste-close','cmd-w','drawer','drawer')
drawer_fixture()
save('drawer-keys',ctl('drawer-key-state','dump'))
drawer_keys('drawer-hide','cmd-j','drawer','composer',draft='',caret=0)
ctl('drawer-draft-text','text','keep this draft')
draft = drawer_keys('drawer-caret','left left','composer','composer')
caret = draft['composer_caret']
drawer_keys('drawer-show-draft','cmd-j','composer','composer',draft='keep this draft',caret=caret)
count = len(ctl('drawer-composer-before-tab','dump')['drawer']['tabs'])
drawer_keys('drawer-composer-tab','cmd-t','composer','composer',draft='keep this draft',caret=caret)
assert len(ctl('drawer-composer-after-tab','dump')['drawer']['tabs'])==count
action('drawer-focus-surface','DebugFocusShellDrawer')
poll('drawer-click-focus',lambda d:d['focus']=='drawer')
drawer_keys('drawer-hide-draft','cmd-j','drawer','composer',draft='keep this draft',caret=caret)
drawer_keys('drawer-show-draft-again','cmd-j','composer','composer',draft='keep this draft',caret=caret)
drawer_keys('drawer-hide-composer','cmd-j','composer','composer',draft='keep this draft',caret=caret)
save('drawer-draft',ctl('drawer-draft-kept','dump'))
drawer_keys('drawer-clear-draft','cmd-a backspace','composer','composer',draft='',caret=0)
drawer_keys('drawer-show-empty','cmd-j','composer','drawer',draft='',caret=0)
records = [dict(type='spin',ms=1500),dict(type='emit_raw',base64='SElEREVOX1BST0dSRVNTDQo=')]
link_fixture('drawer.jsonl',fixture_source(records))
drawer_keys('drawer-hidden-tab','cmd-t','drawer','drawer')
remember(poll('drawer-hidden-started',lambda d:all(tab['pid'] for tab in d['drawer']['tabs'])))
drawer_keys('drawer-hide-output','cmd-j','drawer','composer')
time.sleep(.2)
quiet = ctl('drawer-hidden-before','dump')
active = quiet['drawer']['active_tab']
def frame_counts(d):
    tab = next(tab for tab in d['drawer']['tabs'] if tab['number']==active)
    return tab['frame_requests'],tab['repaints']
time.sleep(1.7)
hidden = ctl('drawer-hidden','dump')
assert hidden['focus']=='composer' and frame_counts(hidden)==frame_counts(quiet)
shown = drawer_keys('drawer-show-output','cmd-j','composer','drawer')
poll('drawer-hidden-parsed',lambda d:'HIDDEN_PROGRESS' in ''.join(d['drawer']['terminal']['grid']))
save('drawer-hidden',{'before_frame_counts':frame_counts(quiet),'hidden':hidden})
drawer_keys('drawer-close-hidden-tab','cmd-w','drawer','drawer')
drawer_fixture(job=True)
drawer_keys('drawer-job-tab','cmd-t','drawer','drawer')
job = poll('drawer-running',lambda d:any(tab['running'] for tab in d['drawer']['tabs']))
remember(job)
action('drawer-stop-thread-keeps-shell','StopThread',{'thread':thread})
kept = ctl('drawer-after-stop-thread','dump')
assert any(tab['running'] for tab in kept['drawer']['tabs']) and kept['focus']=='drawer'
drawer_keys('drawer-job-close','cmd-w','drawer','none')
poll('drawer-close-sheet',lambda d:d['drawer']['close_sheet'] is not None)
capture('drawer-running-close')
drawer_keys('drawer-cancel-close','escape','none','drawer')
assert any(tab['running'] for tab in ctl('drawer-job-kept','dump')['drawer']['tabs'])
drawer_keys('drawer-job-close-again','cmd-w','drawer','none')
drawer_keys('drawer-confirm-close','cmd-enter','none','drawer')
poll('drawer-job-closed',lambda d:len(d['drawer']['tabs'])==1)
drawer_fixture()
other = create('drawer-other-thread','reply with the word ok')
ctl('drawer-other-done','wait',other,'Done')
assert not ctl('drawer-other-hidden','dump')['drawer'].get('open',False)
drawer_keys('drawer-other-open','cmd-j','composer','drawer')
remember(poll('drawer-other-ready',lambda d:all(tab['pid'] for tab in d['drawer']['tabs'])))
action('drawer-return-thread','SelectThread',{'thread':thread})
returned = poll('drawer-thread-state',lambda d:d['selected_thread']['id']==thread)
assert returned['drawer']['open'] and returned['drawer']['tabs'][0]['pid']==first_pid
assert returned['focus']=='composer'
action('drawer-focus-again','DebugFocusShellDrawer')
poll('drawer-focus-again-state',lambda d:d['focus']=='drawer')
link_fixture('drawer.jsonl',fixture_source([dict(type='spin',ms=500),dict(type='exit',code=7)]))
drawer_keys('drawer-ending-tab','cmd-t','drawer','drawer')
remember(poll('drawer-ending-started',lambda d:all(tab['pid'] for tab in d['drawer']['tabs'])))
ended = poll('drawer-ended',lambda d:any(tab['exited'] for tab in d['drawer']['tabs']))
number = ended['drawer']['active_tab']
assert ended['focus']=='drawer' and any(tab['exit_code']==7 for tab in ended['drawer']['tabs'])
capture('drawer-ended')
drawer_fixture()
drawer_keys('drawer-restart','enter','drawer','drawer')
restarted = poll('drawer-restarted',lambda d:d['drawer']['active_tab']==number
    and all(not tab['exited'] for tab in d['drawer']['tabs']) and 'VEYRA_TUI_READY' in ''.join(d['drawer']['terminal']['grid']))
remember(restarted)
assert next(tab['pid'] for tab in restarted['drawer']['tabs'] if tab['number']==number) != next(
    tab['pid'] for tab in ended['drawer']['tabs'] if tab['number']==number)
capture('drawer-restarted')
save('drawer-restarted',restarted)
drawer_keys('drawer-close-restarted','cmd-w','drawer','drawer')
drawer_fixture(job=True)
drawer_keys('drawer-archive-job','cmd-t','drawer','drawer')
remember(poll('drawer-archive-running',lambda d:any(tab['running'] for tab in d['drawer']['tabs'])))
action('drawer-archive','Archive',{'thread':thread})
poll('drawer-archive-sheet',lambda d:d['drawer']['close_sheet'] is not None)
capture('drawer-archive')
archived_focus = drawer_keys('drawer-archive-confirm','cmd-enter','none','composer')
assert archived_focus['view']=='new_thread' and archived_focus['selected_thread'] is None
poll('drawer-archived',lambda d:all(row.get('id')!=thread for row in d['sidebar']['rows']))
action('drawer-select-other','SelectThread',{'thread':other})
poll('drawer-other-selected',lambda d:d['selected_thread'] is not None and d['selected_thread']['id']==other)
action('drawer-other-focus','DebugFocusShellDrawer')
poll('drawer-other-focus-state',lambda d:d['focus']=='drawer')
drawer_keys('drawer-quit-job','cmd-t','drawer','drawer')
remember(poll('drawer-quit-running',lambda d:any(tab['running'] for tab in d['drawer']['tabs'])))
drawer_keys('drawer-quit','cmd-q','drawer','none')
quit_state = poll('drawer-quit-sheet',lambda d:d['quit_sheet']['open'])
project_name = next(row['name'] for row in quit_state['sidebar']['rows'] if row['kind']=='project')
assert any('sleep' in line and project_name in line and quit_state['selected_thread']['title'] in line
    for line in quit_state['quit_sheet']['threads'])
capture('drawer-quit-sheet')
drawer_keys('drawer-cancel-quit','escape','none','drawer')
app = identity()
provider_groups = [int(value) for value in sql('select live_pgid from threads where live_pgid is not null').split()]
ctl('drawer-orderly-quit','quit')
wait_gone(app)
for pid in owned_shells: wait_gone(pid)
for group in provider_groups:
    try: os.killpg(group,0)
    except ProcessLookupError: pass
    else: raise AssertionError('owned provider group survived drawer quit: '+str(group))
save('drawer-orderly',{'app_pid':app,'provider_groups':provider_groups,'shell_pids':sorted(owned_shells),'gone':True})
drawer_fixture()
start('drawer-after-quit')
poll('drawer-after-quit-path',lambda d:d['login_path']!='pending')
action('drawer-after-quit-select','SelectThread',{'thread':other})
poll('drawer-after-quit-selected',lambda d:d['selected_thread'] is not None and d['selected_thread']['id']==other)
assert not ctl('drawer-after-quit-empty','dump')['drawer'].get('open',False)
folder = (DATA/'drawer-project').resolve()
folder.mkdir()
action('drawer-project','AddProject',{'path':str(folder)})
projects = poll('drawer-project-rows',lambda d:any(row['kind']=='project' and row['path']==str(folder)
    for row in d['sidebar']['rows']))
project = next(row['id'] for row in projects['sidebar']['rows'] if row['kind']=='project' and row['path']==str(folder))
action('drawer-project-select','SelectProject',{'project':project})
missing = create('drawer-missing-thread','reply with the word ok')
ctl('drawer-missing-done','wait',missing,'Done')
folder.rmdir()
drawer_keys('drawer-missing-open','cmd-j','composer','drawer')
missing_state = ctl('drawer-missing-folder','dump')
assert missing_state['drawer']['tabs'][0]['pid'] is None and 'folder' in missing_state['drawer']['tabs'][0]['error']
capture('drawer-missing-folder')
for index in range(2):
    retry = drawer_keys('drawer-folder-still-missing-'+str(index),'enter','drawer','drawer')
    assert retry['drawer']['tabs'][0]['pid'] is None and 'folder' in retry['drawer']['tabs'][0]['error']
folder.mkdir()
drawer_keys('drawer-folder-retry','enter','drawer','drawer')
remember(poll('drawer-folder-restarted',lambda d:all(tab['pid'] for tab in d['drawer']['tabs'])))
drawer_fixture(job=True)
drawer_keys('drawer-crash-job','cmd-t','drawer','drawer')
crash_state = poll('drawer-crash-running',lambda d:any(tab['running'] for tab in d['drawer']['tabs']))
remember(crash_state)
crash_pids = [tab['pid'] for tab in crash_state['drawer']['tabs']]
pid = identity()
os.kill(pid,signal.SIGKILL)
wait_gone(pid)
for child in crash_pids: wait_gone(child)
save('drawer-crash',{'app_pid':pid,'shell_pids':crash_pids,'gone':True})
drawer_fixture('flood-50mb')
start('drawer-after-crash')
poll('drawer-after-crash-path',lambda d:d['login_path']!='pending')
action('drawer-flood-select','SelectThread',{'thread':missing})
poll('drawer-flood-selected',lambda d:d['selected_thread'] is not None and d['selected_thread']['id']==missing)
drawer_keys('drawer-flood-open','cmd-j','composer','drawer')
flood = poll('drawer-flood-ready',lambda d:d['perf']['flood'] is not None)
assert flood['perf']['flood']['pty_bytes']==50000020
remember(flood)
save('drawer-flood',dict(flood['perf']['flood'],columns=flood['drawer']['terminal']['columns'],rows=flood['drawer']['terminal']['rows']))
capture('drawer-flood')
drawer_keys('drawer-close-last','cmd-w','drawer','composer')
page_done('shell-drawer')
print('PASS drawer focus table, draft/caret, six tabs, keys, bracketed paste, hidden output, restart, missing folder, archive, quit, crash and flood')
PY
```

## Observable success

`drawer` reports open, height, active_tab, tab pids, running process paths and exit codes; its nested terminal reports grid, focused, columns and rows.
`focus` and `composer_caret` accompany every key receipt.
The paste fixture plants ESC, DEL and C0 controls and records all received bytes as one compact hexadecimal record, so all 23 filtered bracketed-paste bytes are checked exactly and in order at layout sizing, including a nine-row drawer.
No grid override is needed.
The native recipe checks exact fake byte echoes, both close decisions, independent thread state, a same-tab restart, missing-folder refusal, archive confirmation, orderly shutdown and crash hangup.
`drawer-hidden.json` checks that hidden output adds no terminal frame_requests or repaints, then verifies the parsed text on show.
The global window counter includes composer caret blinks and is not a terminal wake counter.
`drawer-flood.json` includes the active grid and `perf.flood` fields pty_bytes, elapsed_ms, repaints, median_repaint_gap_ms and max_repaint_gap_ms.
Read the drawer, resize, paste, header, close-sheet, ended-tab, restarted-tab, missing-folder, archive, quit-sheet and flood PNGs at original size.

### Dump fields

The read-only dump reports `login_path`, `drawer`, `focus`, `composer_caret`, `tasks`, `pid`, `build_rev`, `data_dir`, `schema_version`, `appearance`, `view`, `header_branch`, `sidebar`, `selected_thread`, `transcript`, `composer_text`, `composer`, `providers`, `settings`, `perf`, `awake`, `processes`, `window_active`, `notifications`, `quit_sheet`, `requests` and `notice`.
`drawer` describes the selected thread's open state, height, active_tab and tabs, with each tab's number, label, pid, running process paths, exit_code, exited flag and error.
`focus` reports composer, drawer or none from their focus observations.
`composer_caret` is the input's byte offset and verifies that toggling the drawer preserves a draft's caret.
`VEYRA_DEBUG_SHELL_FIXTURE` runs only the adjacent fake CLI's shell mode, using a run-owned fixture; the fake lane always sets it and never opens the user's real shell.
The shell environment strips all VEYRA_* variables, so the fake fixture path is injected only after that filter for the debug fake.
`perf.flood` measures the active drawer tab after its FLOOD_DONE marker.
`perf.flood` is null before FLOOD_DONE, then holds `pty_bytes`, `elapsed_ms`, `repaints`, `median_repaint_gap_ms` and `max_repaint_gap_ms` between the reader markers.

Its nested `terminal` carries grid, focused, columns, rows and repaint count for the active tab.

Sheets report none and remember their originating surface only for Cancel.

The bytes exclude both OSC markers; the repaint gaps use only painted frames inside those timestamps.

### Evidence contract

```json
{
  "required": {
    "shell-drawer": [
      "drawer-open.png",
      "drawer-header.png",
      "drawer-resize.png",
      "drawer-paste.png",
      "drawer-draft.json",
      "drawer-keys.json",
      "drawer-hidden.json",
      "drawer-running-close.png",
      "drawer-restarted.json",
      "drawer-missing-folder.png",
      "drawer-archive.png",
      "drawer-quit-sheet.png",
      "drawer-crash.json",
      "drawer-flood.json",
      "drawer-flood.png"
    ]
  },
  "pairs": {
    "shell-drawer": [
      [
        "drawer-open",
        "drawer-running-close"
      ]
    ]
  }
}
```

## Gotchas

Native pointer delivery, selection dragging and IME are not established by the control driver.
Window tests cover the click focus and emulator selection wiring; IME remains unsupported.
The pasteboard helper snapshots every item and every declared type into memory before staging the fixture text.
It runs in its own session and restores on pipe EOF, including when the driving process exits.
It refuses an incomplete or changing snapshot before touching the board.
The planted check uses two items containing PNG, binary, rich text and plain text; only their types and hashes are recorded.
The original clipboard's contents and hashes are never logged or stored.
The shell fixture's background-job scenario starts only an owned sleep child; Control-C ends it, and normal group hangup reaches it.
The shell and terminal paths never launch an installed agent CLI.
Never replace the owned fixture with the human's shell during verification.
