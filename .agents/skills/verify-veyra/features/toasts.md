# Toasts

Status: not run, 2026-10-31, 75a17de, native checks await GO, none
Covers: crates/veyra/src/ui/toasts.rs crates/veyra/src/ui/notify.rs crates/veyra/src/ui/actions.rs crates/veyra/src/ui/composer.rs crates/veyra/src/ui/theme.rs crates/veyra/src/ui/sidebar.rs crates/veyra/src/ui/icons.rs crates/veyra/assets/marks/**

## Sub-features

- One replaceable notice has Copy and DismissNotice; it replaces the composer's danger line.
- One attention toast per unselected family follows the same Needs you, Done and Failed transitions as banners.
- New attention toasts appear only for the active window. Existing toasts stay when it goes inactive; later changes post banners. Open selects the target; selecting any family member removes its toast.
- Nothing auto-hides. At most three cards show, and a resting stack arms no clock.
- A storage-blocked notice stays at the top until the block clears, keeps Copy and has no close control.

## How to get to it (user POV)

A refused command or an empty Send shows a notice at the top right under the header.
Copy copies its words. The close button or Escape dismisses an ordinary notice.
A family's new attention status shows its title, a status mark and word, and Open when another family is selected.
Consent stays in the dock: a toast never offers Allow or Deny.
The storage-blocked card uses the same 360-point frame and 16-point inset as kit toasts, reserves one of the three shown slots, and cannot be dismissed.

## Driving it with veyra ctl

Copy comparisons run inside the helper's snapshot/restore session. Only the known fixture expectation is written; clipboard contents, lengths and hashes are never printed or stored.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
prepare()
action('toast-empty-send','Send')
notice=poll('toast-notice',lambda d:len(d['toasts'])==1 and d['toasts'][0]['kind']=='notice')
assert notice['notice']==notice['toasts'][0]['text']=='Write a message before sending'
capture_content('toast-notice')
expected=PROOF/'toast-copy.expected'
expected.write_text('Write a message before sending')
ready=PROOF/'toast-copy.ready'
if ready.exists():
    from orchestration import SafetyStop
    save('safety-stop',{'kind':'clipboard','reason':'PASTEBOARD READY FILE EXISTS'})
    raise SafetyStop('PASTEBOARD READY FILE EXISTS')
helper=subprocess.Popen([str(PROOF/'pasteboard'),'--audit','--expect-text',str(expected),'--ready-file',str(ready)],
    stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,start_new_session=True)
save('toast-copy-helper',{'pid':helper.pid,'pgid':helper.pid})
try:
    poll('toast-copy-session-ready',lambda d:helper.poll() is not None or ready.exists())
    if helper.poll() is None:
        action('toast-copy','CopyNotice')
        ctl('toast-copy-dispatched','dump')
finally:
    helper.stdin.close()
    helper.stdin=None
    output,error=helper.communicate(timeout=10)
    if helper.returncode==2:
        from orchestration import SafetyStop
        reason={
            'PASTEBOARD RESTORE FAILED\n':'PASTEBOARD RESTORE FAILED',
            'PASTEBOARD EXPECTED FILE UNREADABLE\n':'PASTEBOARD EXPECTED FILE UNREADABLE',
            'PASTEBOARD READY FILE EXISTS\n':'PASTEBOARD READY FILE EXISTS',
        }.get(error,'Pasteboard comparison failed')
        save('safety-stop',{'kind':'clipboard','reason':reason})
        raise SafetyStop(reason)
assert helper.returncode==0 and output=='match\n' and not error, 'Clipboard comparison did not match the fixture'
save('toast-copy-result',{'match':True})
action('toast-dismiss','DismissNotice')
poll('toast-dismissed',lambda d:not d['toasts'] and d['notice'] is None)
first=create('toast-background','Finish in the background',slow=True)
ctl('toast-background-working','wait',first,'Working')
second=create('toast-foreground','Keep this thread selected')
ctl('toast-foreground-done','wait',second,'Done')
attention=poll('toast-attention',lambda d:any(t['kind']=='attention' and t['root']==first for t in d['toasts']))
assert attention['selected_thread']['id']==second
assert len([t for t in attention['toasts'] if t.get('root')==first])==1
capture_content('toast-attention')
entry=next(t for t in attention['toasts'] if t.get('root')==first)
action('toast-open','SelectThread',{'thread':entry['thread']})
poll('toast-selected',lambda d:d['selected_thread']['id']==entry['thread'] and not any(t.get('root')==first for t in d['toasts']))
prepare()
page_done('toasts')
PY
```

### Native smoke

The notice remains at rest while two fresh dumps and the capture are taken. The lead reads the capture; class B owns mark shapes and the storage/kit frame match.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
from frame_scene import idle_variant
prepare()
action('toasts-smoke-send','Send')
state=poll('toasts-smoke-ready',lambda d:bool(d['toasts']))
variant=idle_variant(state,'6d')
assert state['notice']==state['toasts'][0]['text']
capture_content('toasts-smoke')
idle_variant(ctl('toasts-smoke-after','dump'),'6d',variant)
page_done('toasts-smoke')
action('toasts-smoke-dismiss','DismissNotice')
prepare()
PY
```

## Observable success

`dump.toasts` is the current list of models. Every entry has `kind` and `text`; attention entries add `root`, `thread` and `status`, and a notice adds `storage_blocked`.
`notice` reads the notice toast's text, including the persistent storage card. `palette.open` is false in the idle scene.
The five original toast tests cover notice replacement, family identity, retention of existing inactive toasts with later banner-only changes, family selection, and no clock. `viewed_family_never_gets_attention_toast` adds the selected-root/child rule.
`toast_removals_leave_no_painted_controls` checks headless dismissal and selection; removal checks also assert painted Copy/Open controls in the window tests. The no-clock test covers the storage card too.
The storage test attempts Escape, DismissNotice and the kit close-button location, then clears the block through the event path.
The class B tests compare status rasters, the kit/storage frames in both appearances, and the three-shown cap including the storage card.

### Evidence contract

```json
{"required":{"toasts":["toast-notice.png","toast-attention.png","toast-copy-result.json"],"toasts-smoke":["toasts-smoke.png","toasts-smoke-native-read.json","toasts-smoke-freshness.json"]}}
```

## Gotchas

The pinned kit cannot suppress its close control. The lead and arch-plan approved the storage-only non-dismissible card as an implementation deviation for errata; all other notices and attention toasts remain on the kit notification list. The card is never remounted after a close.
Inactive banner delivery and native banner clicks retain the existing notification page's operator limits.
DismissToast closes an attention toast through its root identity.
A model dump establishes state, while the smoke capture establishes placement. A resting toast has no timeout.
