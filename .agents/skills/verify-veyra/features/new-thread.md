# New thread

Status: STATE, DATE, COMMIT, MISSING

## Sub-features

- `provider-readiness`: missing Claude disables Send and names the installation command.
- `provider-recheck`: Check again discovers the newly linked fake binary without a relaunch or thread.
- `provider-no-account`: an initialize reply without account stays Ready, not Signed out.
- `composer-pickers`: provider, model, effort and permission choices change the live composer through actions and keys.
- `new-thread-layout`: question, pickers, textarea and hooks/MCP hint in the upper third.

## How to get to it (user POV)

Add a project with ⌘⇧O or the sidebar button.
New thread or ⌘N opens the composer.
Choose the provider, model, effort and permission chips.
The provider menu marks Claude Code, Codex and Grok Ready, Not installed or Signed out.
A missing Claude names `npm install -g @anthropic-ai/claude-code`; a signed-out Claude names `claude auth login`.
Check again reaches `veyra::RecheckProviders`.
Codex and Grok stay Ready until their codecs supply a recorded readiness signal.
Type in the textarea, then Send, Enter or ⌘↵ to create a thread.
Shift+Enter inserts a newline.
This page stops before Send so its readiness proofs must leave zero threads.

## Driving it with veyra ctl

Launch and Doctor first.
The default fake branch covers installation, Check again and choices.
The authorized live branch covers startup readiness and leaves zero threads without replay overrides.
The fixture binary path must still be missing and the database must contain zero threads.
app-launch's project row is safe to reuse.
Each section recovers RUN and proof records in a fresh shell.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH="$PROOF" python3 - <<'PY'
from driver import *
identity()
if LANE == 'live':
    if sql('select count(*) from projects') == '0':
        action('live-project','AddProject',{'path':str(DATA/'project')})
        poll('live-project-added',lambda d: any(row['kind']=='project' for row in d['sidebar']['rows']))
    action('live-new-thread','NewThread')
    ready = poll('new-thread-live',lambda d: d['providers'][0]['readiness']=='ready'
        and bool(d['providers'][0]['models']))
    assert ready['composer']['send_enabled']
    assert sql('select count(*) from threads') == '0'
    capture('new-thread-live')
    page_done('new-thread-live')
    print('PASS live provider ready; models present; threads=0')
    sys.exit(0)
assert sql('select count(*) from threads') == '0'
assert not (DATA/'fake-cli').exists()
if sql('select count(*) from projects') == '0':
    action('new-thread-project','AddProject',{'path':str(DATA/'project')})
    poll('new-thread-project-added',lambda d: any(row['kind']=='project' for row in d['sidebar']['rows']))
action('new-thread-open','NewThread')
missing = poll('new-thread-missing',lambda d: d['providers'][0]['readiness'] == 'not_installed')
assert not missing['composer']['send_enabled']
assert 'npm install -g @anthropic-ai/claude-code' in missing['composer']['disabled_reason']
assert missing['view'] == 'new_thread'
capture('new-thread-missing')
action('new-thread-provider-menu-action','OpenProviderPicker')
poll('new-thread-provider-menu-open',lambda d: d['composer']['provider_picker_open'])
capture('new-thread-provider-menu')
ctl('new-thread-provider-menu-dismiss','keys','escape')
poll('new-thread-provider-menu-closed',lambda d: not d['composer']['provider_picker_open'])
ctl('new-thread-provider-menu-key','keys','secondary-alt-shift-p')
poll('new-thread-provider-menu-key-open',lambda d: d['composer']['provider_picker_open'])
capture('new-thread-provider-menu-key')
action('new-thread-provider-menu-close','PickProvider',{'provider':'claude'})

(DATA/'fake-cli').symlink_to(pathlib.Path('target/debug/veyra-fake-cli').resolve())
action('new-thread-recheck','RecheckProviders')
ready = poll('new-thread-ready',lambda d: d['providers'][0]['readiness'] == 'ready')
assert ready['composer']['send_enabled'] and ready['providers'][0]['models']
assert ready['providers'][0]['account'] == 'known'
assert sql('select count(*) from threads') == '0'
shutil.copy2(SOURCE/'probe-no-account.jsonl',FIXTURES/'probe.jsonl')
ctl('new-thread-recheck-key','keys','secondary-alt-r')
no_account = poll('new-thread-no-account',lambda d: d['providers'][0]['readiness'] == 'ready')
assert no_account['composer']['send_enabled']
assert no_account['providers'][0]['account'] == 'unknown'
assert no_account['providers'][0]['models']
assert sql('select count(*) from threads') == '0'
for provider in ['codex','grok','claude']:
    ctl('new-thread-provider-key-'+provider,'keys','secondary-alt-p')
    assert ctl('new-thread-provider-'+provider,'dump')['composer']['provider'] == provider
action('new-thread-provider-action','PickProvider',{'provider':'claude'})
assert ctl('new-thread-provider-action-after','dump')['composer']['provider'] == 'claude'
ctl('new-thread-model-key','keys','secondary-alt-m')
model = no_account['providers'][0]['models'][0]
assert ctl('new-thread-model','dump')['composer']['model'] == model['value']
ctl('new-thread-effort-key','keys','secondary-alt-e')
assert ctl('new-thread-effort','dump')['composer']['effort'] == model['supportedEffortLevels'][0]
choice = no_account['providers'][0]['models'][1]
action('new-thread-model-action','PickModel',{'model':choice['value']})
assert ctl('new-thread-model-action-after','dump')['composer']['model'] == choice['value']
effort = choice['supportedEffortLevels'][1]
action('new-thread-effort-action','PickEffort',{'effort':effort})
assert ctl('new-thread-effort-action-after','dump')['composer']['effort'] == effort
for mode in ['ask_first','accept_edits','full_access']:
    action('new-thread-mode-action-'+mode,'PickPermission',{'mode':mode})
    assert ctl('new-thread-mode-'+mode,'dump')['composer']['mode'] == mode
ctl('new-thread-mode-key','keys','secondary-alt-a')
assert ctl('new-thread-mode-key-after','dump')['composer']['mode'] == 'ask_first'
action('new-thread-default-reset','NewThread')
assert ctl('new-thread-default','dump')['composer']['mode'] == 'full_access'
assert ctl('new-thread-text','text','A draft, not a thread') == {'ok':True}
assert ctl('new-thread-draft','dump')['composer_text'] == 'A draft, not a thread'
action('new-thread-clear','NewThread')
assert ctl('new-thread-after','dump')['composer_text'] == ''
assert sql('select count(*) from threads') == '0'
capture('new-thread')
page_done('new-thread')
print('PASS new-thread missing → ready → no-account Ready; choices live; threads=0')
PY
```

## Observable success

- `new-thread-missing.json`: Claude `not_installed`, Send disabled, installation command visible; inspect its PNG.
- `new-thread-ready.json`: Claude `ready`, models present, Send enabled after RecheckProviders.
- `new-thread-no-account.json`: account presence changes from `known` to `unknown`, models stay non-empty and Claude stays `ready` on the account-free probe; every readiness phase asserts `select count(*) from threads` is 0.
- `composer.provider`, `model`, `effort` and `mode` assert every choice; `composer_text` asserts typing and NewThread's reset.
- `new-thread-provider-menu.png` shows the actual Not installed mark after OpenProviderPicker; its key opens the same menu.
- `new-thread.png`: upper-third question and composer, hooks/MCP hint, left-aligned Add project and Geist.
- `drive.log`: named `PickProvider`, `PickModel`, `PickEffort`, `PickPermission`, `RecheckProviders` and their keys, with exit codes.

## Gotchas

A missing account is unknown, not proof of sign-out.
Do not invent a signed-out fixture before a live signal is recorded.
Check again may briefly show Checking; the helper polls that state at the same 50 ms cadence as ctl wait.
The checks do not run a Claude turn or mutate any committed fixture.
The native folder picker and external keyboard delivery are manual entry points.
The lead fills the Status values after its gate.
