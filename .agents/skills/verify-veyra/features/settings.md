# Settings

Status: STATE, DATE, COMMIT, MISSING

## Sub-features

- `settings-entry`: ⌘, and the menu action open one in-window Settings view.
- `settings-modes`: Full access is the fresh stored default; all three choices publish SettingsChanged.
- `settings-new-only`: Ask first applies to a new thread while the existing Full access thread stays unchanged.
- `settings-persistence`: SQLite stores Ask first and a relaunch keeps it, its new-thread chip and both thread configurations.
- `settings-exit`: Escape, SelectThread and SelectProject leave Settings through actions.
- `settings-focus-transition`: NewThread immediately after Escape reaches the rendered composer through both its action and key.

## How to get to it (user POV)

Choose Settings… in the Veyra menu or press ⌘, (`veyra::OpenSettings`).
The main column shows Default mode for new threads, Ask first, Accept edits and Full access.
Choose a mode (`veyra::SetDefaultMode`) to read its permission consequence and store it.
The hint says Applies to threads you create from now on.
Escape or selecting a thread leaves Settings.
It does not reconfigure existing threads or open a second window.
⌘⌥D cycles the default mode; ⌘⌥1 opens the first active thread and ⌘⌥⇧1 selects the first project.

## Driving it with veyra ctl

Run Launch, Doctor and new-thread first.
This page can run alone on a fresh DATA after those prerequisites, or after chat-send-stream on the same initially fresh DATA.
The initial stored default must still be Full access.
It creates a Full access thread, chooses Ask first, creates an Ask first thread and relaunches.
Use ctl wait for both completed turns.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
assert sql('select default_mode from settings') == 'full_access'
action('settings-initial-new','NewThread')
assert ctl('settings-initial-chip','dump')['composer']['mode'] == 'full_access'
first = create('settings-full','reply with the word ok')
ctl('settings-full-done','wait',first,'Done')
ctl('settings-open-key','keys','secondary-,')
initial = poll('settings-first',lambda d: d['view']=='settings')
assert initial['view'] == 'settings' and initial['settings']['default_mode'] == 'full_access'
assert initial['selected_thread']['config']['mode'] == 'full_access'
capture('settings-full-access')
action('settings-invalid-mode','SetDefaultMode',{'mode':'invalid'})
assert ctl('settings-command-error','dump')['notice'] == 'Unknown permission mode'
capture('settings-command-error')
for mode in ['ask_first','accept_edits','full_access']:
    ctl('settings-mode-key-'+mode,'keys','secondary-alt-d')
    assert poll('settings-key-'+mode+'-after',lambda d: d['settings']['default_mode']==mode)['view'] == 'settings'
for mode in ['accept_edits','full_access','ask_first']:
    action('settings-mode-'+mode,'SetDefaultMode',{'mode':mode})
    changed = poll('settings-mode-'+mode+'-after',lambda d: d['settings']['default_mode'] == mode)
    assert changed['view'] == 'settings'
    assert sql('select default_mode from settings') == mode
    assert changed['selected_thread']['config']['mode'] == 'full_access'
save('settings-ask-first',changed)
capture('settings')
ctl('settings-no-input','text','not a message',expected=1)
ctl('settings-escape','keys','escape')
existing = poll('settings-existing',lambda d: d['view']=='chat' and d['selected_thread']['id']==first)
assert existing['view'] == 'chat' and existing['selected_thread']['id'] == first
assert existing['composer']['mode'] == 'full_access'
assert existing['settings']['default_mode'] == 'ask_first'
action('settings-next-new','NewThread')
assert poll('settings-next-chip',lambda d: d['view']=='new_thread')['composer']['mode'] == 'ask_first'
second = create('settings-ask','reply with the word ok')
new = ctl('settings-new-wait','wait',second,'Done')
save('settings-new',new)
assert new['composer']['mode'] == 'ask_first'
assert sql("select json_extract(config_json,'$.mode') from threads where id='"+first+"'") == 'full_access'
assert sql("select json_extract(config_json,'$.mode') from threads where id='"+second+"'") == 'ask_first'
poll('settings-sessions-ended',lambda d: sql('select count(*) from threads where live_pgid is not null') == '0')
restart('settings-restart')
reloaded = poll('settings-relaunch',lambda d: d['settings']['default_mode'] == 'ask_first')
assert reloaded['composer']['mode'] == 'ask_first'
assert {row['id']:row['mode'] for row in reloaded['sidebar']['rows'] if row['kind']=='thread'}[first] == 'full_access'
assert {row['id']:row['mode'] for row in reloaded['sidebar']['rows'] if row['kind']=='thread'}[second] == 'ask_first'
database('settings-persisted','ask_first')
action('settings-open-action','OpenSettings')
assert poll('settings-action-view',lambda d: d['view']=='settings')['view'] == 'settings'
action('settings-select-existing','SelectThread',{'thread':first})
selected = poll('settings-selected-existing',lambda d: d['view']=='chat' and d['selected_thread']['id']==first)
assert selected['composer']['mode'] == 'full_access'
action('settings-open-again','OpenSettings')
ctl('settings-select-first-key','keys','secondary-alt-1')
assert poll('settings-selected-first-key',lambda d: d['view']=='chat')['view'] == 'chat'
action('settings-reopen-before-project','OpenSettings')
project = next(row['id'] for row in selected['sidebar']['rows'] if row['kind']=='project')
action('settings-select-project','SelectProject',{'project':project})
assert poll('settings-project-exit',lambda d: d['view']=='new_thread')['view'] == 'new_thread'
assert ctl('settings-project-chip','dump')['composer']['mode'] == 'ask_first'
action('settings-reopen-before-project-key','OpenSettings')
ctl('settings-select-project-key','keys','secondary-alt-shift-1')
assert poll('settings-project-key-exit',lambda d: d['view']=='new_thread')['view'] == 'new_thread'
for command,argument in [('action','veyra::NewThread'),('keys','secondary-n')]:
    action('settings-transition-select-'+command,'SelectThread',{'thread':first})
    action('settings-transition-open-'+command,'OpenSettings')
    poll('settings-transition-opened-'+command,lambda d: d['view']=='settings')
    ctl('settings-transition-escape-'+command,'keys','escape')
    poll('settings-transition-chat-'+command,lambda d: d['view']=='chat')
    ctl('settings-transition-new-'+command,command,argument)
    transition = poll('settings-transition-'+command,lambda d: d['view']=='new_thread')
    assert transition['view'] == 'new_thread' and transition['selected_thread'] is None
    assert transition['composer']['mode'] == 'ask_first'
action('settings-final-open','OpenSettings')
assert poll('settings-final',lambda d: d['view']=='settings')['view'] == 'settings'
capture('settings-relaunch')
print('PASS criterion 19: fresh Full access; stored Ask first; old thread Full access; new thread Ask first; relaunch keeps all three')
PY
```

## Observable success

- `settings-first.json` has `view: settings`, `settings.default_mode: full_access` and the first thread's Full access configuration; inspect `settings-full-access.png`.
- Every mode change is asserted in both dump and `select default_mode from settings`; the direct SetDefaultMode key cycles all three.
- `settings-command-error.json` and its PNG show a rejected mode as a visible notice without changing the setting.
- `settings-ask-first.json` and `settings.png` show Ask first, its full consequence copy and the applies-from-now hint.
- `settings-existing.json` keeps the old Full access composer; `settings-new.json` has the new Ask first thread.
- Both thread configuration rows are checked directly in SQLite.
- `settings-relaunch.json` and `settings-persisted-database.txt` retain Ask first after a new pid; both thread rows retain their own modes.
- Escape, SelectThread and SelectProject each assert the resulting view, and OpenSettings is driven by both its key and named action.
- `settings-transition-action.json` and `settings-transition-keys.json` show NewThread immediately after Escape, with no selected thread and the Ask first composer.
- `drive.log` records commands, replies and exits; the text request while Settings is focused exits 1 instead of editing a hidden composer.

## Gotchas

Settings is not a second native window.
The driver dispatches through the same OpenSettings action as the menu, but does not prove a native menu click.
The helper waits for provider groups to finish before relaunch; it does not add provider reaping.
The default is stored, not a process-local preference.
The lead fills the Status values after its gate.
