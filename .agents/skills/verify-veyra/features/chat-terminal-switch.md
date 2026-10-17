# Chat and terminal switch

Status: not run, DATE, COMMIT, all

## Sub-features

- Chat and Terminal share one native conversation and the header names a refused switch.
- The terminal fills the thread width above its 30 px hint row, with a still muted open arc in the sidebar.
- The window-level `veyra::SwitchView` binding is ⌘⇧V and wins while the terminal has focus.
- `text` sends character key events to the focused terminal; Shift+Enter, Option+Left and Ctrl+L reach the PTY.
- Returning to chat imports each typed message once, preserves the native id and advances `native_cursor` once per import.
- Quitting in terminal view recovers to chat and imports before the next structured turn.
- One debug flood reports exactly 50,000,020 payload bytes and its repaint measurements.

## How to get to it (user POV)

Open an idle Claude thread that already has a conversation.
Use the header's Chat and Terminal buttons or press ⌘⇧V.
The hint names that shortcut and says switching ends the CLI session.
Type in the TUI, then press ⌘⇧V to return to chat with the terminal span.
A running turn, pending request, background work or missing native conversation refuses the switch with a reason in the header.
The optional `SwitchView` action payload `{"terminal":true}` selects Terminal; false selects Chat.

## Driving it with veyra ctl

Run Launch, Doctor and new-thread first in the fake lane.
The SDK anchor written by `terminal_seed` is Veyra-authored fixture setup for the already-visible chat turn; all later entries come from the fake TUI's typed input.
The fake drives use `CLAUDE_CONFIG_DIR=$DATA/claude-home`; live drives never set it.
Every switch below uses `keys cmd-shift-v`, including the return while Terminal owns focus.
After the flood return, one structured reply finishes the resumed fake fixture before later pages require all sessions to be parked.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
assert LANE == 'fake'
action('terminal-provider','NewThread')
action('terminal-claude','PickProvider',{'provider':'claude'})
thread = create('terminal-chat','reply with the word ok')
ctl('terminal-chat-done','wait',thread,'Done')
native,path = terminal_seed(thread)
initial_cursor = sql("select native_cursor from threads where id='"+thread+"'")
sql('CREATE TABLE terminal_cursor_audit (thread TEXT, old_cursor TEXT, new_cursor TEXT, importing INTEGER); CREATE TRIGGER terminal_cursor_changed AFTER UPDATE OF native_cursor ON threads WHEN OLD.native_cursor IS NOT NEW.native_cursor BEGIN INSERT INTO terminal_cursor_audit VALUES (NEW.id,OLD.native_cursor,NEW.native_cursor,OLD.import_pending=1 AND NEW.import_pending=0); END;')
opened = terminal_enter('terminal-open')
assert opened['selected_thread']['native_id']==native
assert opened['selected_thread']['status']=='InTerminal'
assert sql("select view from threads where id='"+thread+"'")=='terminal'
capture('terminal-open')
first = 'terminal window heron'
ctl('terminal-type','text',first)
ctl('terminal-newline','keys','shift-enter')
ctl('terminal-word-left','keys','alt-left')
ctl('terminal-clear','keys','ctrl-l')
def hex_seen(d):
    grid = ' '.join(d['terminal']['grid']) if d['terminal'] else ''
    return '1b 0d' in grid and '1b 62' in grid and '0c' in grid
hex_dump = poll('terminal-hex',hex_seen)
save('terminal-hex',hex_dump)
end = time.monotonic()+5
while not path.exists() or first not in path.read_text():
    assert time.monotonic()<end, 'typed text not saved in isolated history'
    time.sleep(.05)
grep = subprocess.run(['grep','-c',first,str(path)],capture_output=True,text=True,check=True)
(PROOF/'terminal-history-grep.txt').write_text(grep.stdout)
assert int(grep.stdout)>=1
missing = subprocess.run(['ls',str(pathlib.Path.home()/'.claude/projects/veyra-fake')],capture_output=True,text=True)
assert missing.returncode != 0
(PROOF/'terminal-real-home-absent.txt').write_text('PASS ls ~/.claude/projects/veyra-fake failed\n')
back = terminal_back('terminal-imported')
spans = [item for item in back['transcript']['items'] if item['kind']=='terminal_span']
assert sum(first in item['text'] for item in spans)==1
assert back['selected_thread']['native_id']==native
assert sql("select native_id from threads where id='"+thread+"'")==native
cursor = sql("select native_cursor from threads where id='"+thread+"'")
assert cursor != initial_cursor
assert sql("select count(*) from terminal_cursor_audit where thread='"+thread+"'")=='1'
assert sql("select count(*) from items where thread_id='"+thread+"' and body_json like '%"+first+"%'")=='1'
capture('terminal-imported')
terminal_enter('terminal-repeat')
repeat = terminal_back('terminal-empty-repeat')
assert sum(first in item['text'] for item in repeat['transcript']['items'] if item['kind']=='terminal_span')==1
assert sql("select native_cursor from threads where id='"+thread+"'")==cursor
assert sql("select count(*) from terminal_cursor_audit where thread='"+thread+"'")=='1'
terminal_enter('terminal-before-quit')
recovery = 'terminal recovery egret'
ctl('terminal-recovery-type','text',recovery)
ctl('terminal-recovery-enter','keys','enter')
end = time.monotonic()+5
while recovery not in path.read_text():
    assert time.monotonic()<end
    time.sleep(.05)
assert sql("select native_cursor from threads where id='"+thread+"'")==cursor
pid = identity()
groups = [int(value) for value in sql('select live_pgid from threads where live_pgid is not null').split()]
ctl('terminal-quit','quit')
wait_gone(pid)
for group in groups:
    try: os.killpg(group,0)
    except ProcessLookupError: pass
    else: raise AssertionError('terminal group survived quit: '+str(group))
stage('ok-reply')
start('terminal-relaunch')
action('terminal-reselect','SelectThread',{'thread':thread})
recovered = poll('terminal-relaunch-chat',lambda d:d['selected_thread'] is not None and d['selected_thread']['id']==thread)
assert recovered['view']=='chat' and recovered['selected_thread']['import_pending']
assert sql("select view||'|'||import_pending from threads where id='"+thread+"'")=='chat|1'
turns = int(sql("select count(*) from turns where thread_id='"+thread+"'"))
ctl('terminal-next-type','text','reply with the word ok')
ctl('terminal-next-send','keys','secondary-enter')
recovered = poll('terminal-recovered',lambda d:d['selected_thread']['status']=='Done'
    and int(sql("select count(*) from turns where thread_id='"+thread+"'"))>=turns+2
    and any(item['kind']=='terminal_span' and recovery in item['text'] for item in d['transcript']['items']))
assert sum(recovery in item['text'] for item in recovered['transcript']['items'] if item['kind']=='terminal_span')==1
assert recovered['selected_thread']['native_id']==native
assert sql("select count(*) from items where thread_id='"+thread+"' and body_json like '%"+recovery+"%'")=='1'
assert sql("select count(*) from terminal_cursor_audit where thread='"+thread+"' and importing=1")=='2'
save('terminal-cursor-updates',sql('select json_group_array(json_object(\'thread\',thread,\'old\',old_cursor,\'new\',new_cursor,\'importing\',importing)) from terminal_cursor_audit'))
sql('DROP TRIGGER terminal_cursor_changed; DROP TABLE terminal_cursor_audit;')
capture('terminal-recovered')
flood = create('terminal-flood-chat','reply with the word ok')
ctl('terminal-flood-chat-done','wait',flood,'Done')
terminal_seed(flood)
terminal_enter('terminal-flood-open','flood-50mb')
flood_dump = poll('terminal-flood',lambda d:d['perf']['flood'] is not None)
assert flood_dump['perf']['flood']['pty_bytes']==50000020, flood_dump['perf']['flood']
save('terminal-flood',flood_dump['perf']['flood'])
capture('terminal-flood')
terminal_back('terminal-flood-back')
turns = int(sql("select count(*) from turns where thread_id='"+flood+"'"))
ctl('terminal-flood-followup-type','text','reply with the word ok')
ctl('terminal-flood-followup-send','keys','secondary-enter')
poll('terminal-flood-followup-done',lambda d:d['selected_thread']['status']=='Done'
    and int(sql("select count(*) from turns where thread_id='"+flood+"'"))==turns+1
    and sql("select live_pgid from threads where id='"+flood+"'")=='')
page_done('chat-terminal-switch')
print('PASS terminal keys, imported once, empty repeat, quit recovery, isolated history, native cursor and one flood')
PY
```

## Observable success

`view` becomes terminal; top-level `terminal.grid`, `focused`, `columns`, `rows` and `repaints` describe the painted emulator.
Top-level `view_refusal` names the header refusal.
`selected_thread.native_id`, `native_cursor`, `view` and `import_pending` follow engine metadata.
`transcript.items` and SQLite `items.body_json` each hold one terminal span per typed message.
The SQLite audit records every cursor change and marks import completion separately from the next dispatched structured turn.
`terminal-hex.json` holds the fake byte echoes, and `terminal-history-grep.txt` holds grep's count.
`perf.flood` holds `pty_bytes`, `elapsed_ms`, `repaints`, `median_repaint_gap_ms` and `max_repaint_gap_ms` after FLOOD_DONE.
The one flood here checks measurement availability and payload size; the phase's performance lane owns its five-sample series and throughput budget.
Read terminal-open.png, terminal-imported.png, terminal-recovered.png and terminal-flood.png for the full-width grid, switch, hint, sidebar arc and imported text.

## Gotchas

Selection, paste and IME are deferred.
The driver proves gpui key dispatch, including the switch precedence, rather than external keyboard delivery.
Use a new run's DATA and the fake overrides; do not drive another instance or the user's own provider history.
The release build contains neither the driver nor flood/repaint instrumentation.
