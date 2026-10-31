# Command palette

Status: not run, 2026-10-31, 75a17de, native checks await GO, none
Covers: crates/veyra/src/ui/palette.rs crates/veyra/src/ui/actions.rs crates/veyra/src/ui/sidebar.rs crates/veyra/src/ui/drawer.rs crates/veyra/src/ui/terminal_view.rs crates/veyra/src/ui/theme.rs crates/veyra/src/ui/icons.rs

## Sub-features

- OpenPalette, the app-menu item and ⌘K open the kit command palette on the opaque content plane.
- Actions retain their origin's key context. Request answers never appear; NewShellTab appears only from the drawer.
- Actions and non-archived Threads filter by title. A leading `>` leaves Actions only.
- Running an entry and Escape restore the originating focus, draft and caret.

## How to get to it (user POV)

Choose Command Palette… in the Veyra menu or press ⌘K from the composer, dock or drawer.
Type to filter, move with Up and Down, and run with Return. The first Escape closes the palette, including a filtered query or no matches.
Clicking outside closes it; the covered window takes no click.
Actions show their chord. Thread entries show their status mark and a child's root title.
AllowOnce, AllowInThread, Deny, AnswerQuestion, ChooseAsyncOption and AnswerFocusedOption never appear.

## Driving it with veyra ctl

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
prepare()
ctl('palette-draft','text','Keep this draft')
ctl('palette-caret','keys','left left')
before=ctl('palette-before','dump')
ctl('palette-open','keys','secondary-k')
opened=poll('palette-opened',lambda d:d['palette']['open'] and d['focus']=='palette')
assert not any(e['action']=='NewShellTab' for e in opened['palette']['actions'])
capture_content('palette-open')
ctl('palette-query','text','> Narrow sidebar')
filtered=poll('palette-filtered',lambda d:len(d['palette']['actions'])==1 and not d['palette']['threads'])
assert filtered['palette']['actions'][0]['action']=='NarrowSidebar'
ctl('palette-run','keys','enter')
returned=poll('palette-returned',lambda d:not d['palette']['open'] and d['focus']=='composer')
assert returned['composer_text']==before['composer_text'] and returned['composer_caret']==before['composer_caret']
action('palette-width-restore','ResetSidebar')
ctl('palette-escape-open','keys','secondary-k')
poll('palette-escape-opened',lambda d:d['palette']['open'])
ctl('palette-escape-query','text','No entry matches this query')
poll('palette-escape-no-matches',lambda d:not d['palette']['actions'] and not d['palette']['threads'])
ctl('palette-first-escape','keys','escape')
poll('palette-first-escape-closed',lambda d:not d['palette']['open'] and d['focus']=='composer')
prepare()
action('palette-mode','SetDefaultMode',{'mode':'ask_first'})
poll('palette-mode-ready',lambda d:d['settings']['default_mode']=='ask_first')
thread=create('palette-question','Choose a route',scenario='question')
try:
    pending=poll('palette-pending',lambda d:d['composer']['dock'] is not None)
    requests=pending['requests']
    action('palette-question-open','OpenPalette')
    docked=poll('palette-refusals',lambda d:d['palette']['open'])
    forbidden={'AllowOnce','AllowInThread','Deny','AnswerQuestion','ChooseAsyncOption','AnswerFocusedOption','NewShellTab'}
    assert not forbidden.intersection(e['action'] for e in docked['palette']['actions'])
    assert docked['requests']==requests
    capture_content('palette-request-refusals')
    ctl('palette-close','keys','escape')
    drawer_fixture()
    ctl('palette-drawer','keys','secondary-j')
    poll('palette-drawer-focus',lambda d:d['focus']=='drawer')
    ctl('palette-drawer-open','keys','secondary-k')
    drawer=poll('palette-drawer-actions',lambda d:d['palette']['open'])
    assert any(e['action']=='NewShellTab' for e in drawer['palette']['actions'])
    capture_content('palette-drawer')
    ctl('palette-drawer-close','keys','escape')
    poll('palette-drawer-returned',lambda d:d['focus']=='drawer' and not d['palette']['open'])
    ctl('palette-hide-drawer','keys','secondary-j')
finally:
    action('palette-stop','StopThread',{'thread':thread})
    poll('palette-drained',lambda d:sql("select live_pgid is null from threads where id='"+thread+"'")=='1')
    action('palette-mode-restore','SetDefaultMode',{'mode':'full_access'})
    prepare()
page_done('command-palette')
PY
```

### Native smoke

This bounded scene proves both refusal rules while a fake request is docked.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
prepare()
action('palette-smoke-mode','SetDefaultMode',{'mode':'ask_first'})
poll('palette-smoke-mode-ready',lambda d:d['settings']['default_mode']=='ask_first')
thread=create('palette-smoke-question','Choose a route',scenario='question')
try:
    pending=poll('palette-smoke-dock',lambda d:d['composer']['dock'] is not None)
    action('palette-smoke-open','OpenPalette')
    state=poll('palette-smoke-opened',lambda d:d['palette']['open'] and d['focus']=='palette')
    forbidden={'AllowOnce','AllowInThread','Deny','AnswerQuestion','ChooseAsyncOption','AnswerFocusedOption','NewShellTab'}
    assert not forbidden.intersection(e['action'] for e in state['palette']['actions'])
    assert state['requests']==pending['requests']
    capture_content('command-palette-smoke')
    page_done('command-palette-smoke')
finally:
    ctl('palette-smoke-close','keys','escape')
    action('palette-smoke-stop','StopThread',{'thread':thread})
    poll('palette-smoke-drained',lambda d:sql("select live_pgid is null from threads where id='"+thread+"'")=='1')
    action('palette-smoke-mode-restore','SetDefaultMode',{'mode':'full_access'})
    prepare()
PY
```

## Observable success

`dump.palette` has `open`, `query`, `actions` and `threads`. Each action has `action`, `label` and `chord`; each thread has `thread` and `title`.
`focus` reads `palette` while it is open. The returned `composer_text` and `composer_caret` equal the saved origin.
Ten palette window tests establish request-answer exclusion in question/approval contexts, drawer-only eligibility, chord behavior, drawer focus, draft/caret, archived-thread exclusion, menu-out timing, restored dock focus before dispatch, first-Escape closure for filtered/no-match queries, and card containment with keyboard scrolling to the final entry.
`palette_metadata_is_explicit_for_every_binding` and `palette_chords_keep_the_shared_shifted_paste_spelling` check the shared table and chord formatter.

### Evidence contract

```json
{"required":{"command-palette":["palette-open.png","palette-request-refusals.png","palette-drawer.png"],"command-palette-smoke":["command-palette-smoke.png","command-palette-smoke-native-read.json","command-palette-smoke-freshness.json"]}}
```

## Gotchas

The palette searches only titles already in window memory. It makes no database search and gives no consent.
The gpui availability list does not determine eligibility: the shared action table's original key context does.
Only the owned fake provider and drawer fixture are used.
Native keyboard, pointer delivery and compositor appearance remain for the armed smoke.
