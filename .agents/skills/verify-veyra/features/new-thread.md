# New thread

Status: passed, 2026-10-26, bd8a083, live lane not rerun since RUN 8d22e59d at 2b45067 (fake CLI RUN 8577dad2), cb67642fe611682bffa46ec31737358c99ca877139def61d0acf78695b1470bb
Covers: crates/veyra/src/ui/new_thread.rs crates/veyra/src/ui/settings.rs crates/engine/src/provider/** crates/engine/tests/fixtures/claude/** crates/engine/tests/fixtures/codex/** crates/engine/tests/fixtures/grok/**

## Sub-features

- `provider-readiness`: missing Claude disables Send and names the installation command.
- `provider-recheck`: Check again discovers the newly linked fake binary without a relaunch or thread.
- `provider-no-account`: an initialize reply without account stays Ready, not Signed out.
- `provider-signed-out`: a signed-out probe disables Send and names `claude auth login`.
- `composer-pickers`: provider, model, effort and permission choices change the live composer through actions and keys.
- `new-thread-layout`: question, pickers, textarea and hooks/MCP hint in the upper third.

## How to get to it (user POV)

Add a project with ⌘⇧O or the sidebar button.
New thread or ⌘N opens the composer.
Choose the provider, model, effort and permission chips.
Claude and Codex offer Ask first, Accept edits and Full access.
Grok offers Ask first and Full access, and its permission key cycle visits only those two.
A new thread uses its provider’s run mode for the Settings default, so Grok shows Ask first when that default is Accept edits.
The thread-creation and stored-mode checks live in [approvals-questions](approvals-questions.md).
Settings continues to offer all three modes.
See [stored Grok modes](permission-matrix.md#stored-grok-modes) for compatibility with existing threads.
The provider menu marks Claude Code, Codex and Grok Ready, Not installed or Signed out.
A missing Claude names `npm install -g @anthropic-ai/claude-code`; a signed-out Claude names `claude auth login`.
Check again reaches `veyra::RecheckProviders`.
Codex reads readiness, models and efforts from its app-server probe.
Codex uses its app-server account signal for the sign-in mark.
Grok reads initialize once for its bare version, cached-token or API-key sign-in signal, and per-model effort catalog.
Without modelState it offers only Default in both menus; Signed out names `grok login` and disables Send.
Type in the textarea, then Send, Enter or ⌘↵ to create a thread.
Shift+Enter inserts a newline.
This page stops before Send so its readiness proofs must leave zero threads.

## Driving it with veyra ctl

Launch and Doctor first.
Run this page before any other page that creates a thread or installs the fake CLI; its Codex and Grok sections call `prepare()` and can run alone.
The default fake branch covers installation, Check again and choices.
The authorized live branch covers startup readiness and leaves zero threads without replay overrides.
The fixture binary path must still be missing and the database must contain zero threads.
app-launch's project row is safe to reuse.
Each section recovers RUN and proof records in a fresh shell.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
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
action('new-thread-recheck-open-menu','OpenProviderPicker')
action('new-thread-recheck','RecheckProviders')
ready = poll('new-thread-ready',lambda d: d['providers'][0]['readiness'] == 'ready')
assert ready['composer']['send_enabled'] and ready['providers'][0]['models']
assert ready['composer']['provider_picker_open']
assert ready['composer']['provider_picker_labels'][0] == 'Claude Code · Ready'
capture('new-thread-provider-menu-ready')
action('new-thread-recheck-close-menu','PickProvider',{'provider':'claude'})
capture('new-thread-ready')
assert ready['providers'][0]['account'] == 'known'
assert sql('select count(*) from threads') == '0'
shutil.copy2(SOURCE/'probe-no-account.jsonl',FIXTURES/'probe.jsonl')
ctl('new-thread-recheck-key','keys','secondary-alt-r')
no_account = poll('new-thread-no-account',lambda d: d['providers'][0]['readiness'] == 'ready')
assert no_account['composer']['send_enabled']
assert no_account['providers'][0]['account'] == 'unknown'
assert no_account['providers'][0]['models']
for scenario, status in [('probe-signed-out','signed_out'),('probe-no-account','ready')]:
    shutil.copy2(SOURCE/(scenario+'.jsonl'),FIXTURES/'probe.jsonl')
    action('claude-source-'+scenario+'-recheck','RecheckProviders')
    probe = poll('claude-source-'+scenario,lambda d:d['providers'][0]['readiness']==status)
    assert probe['composer']['send_enabled'] == (status=='ready')
    if status=='signed_out':
        assert 'claude auth login' in probe['composer']['disabled_reason']
        assert probe['composer']['disabled_reason'].count('Sign in')==1
        capture('new-thread-signed-out-hint')
        action('new-thread-signed-out-menu','OpenProviderPicker')
        menu = poll('new-thread-signed-out-menu',lambda d:d['composer']['provider_picker_open'])
        assert menu['composer']['disabled_reason']==probe['composer']['disabled_reason']
        capture('new-thread-signed-out-menu')
        action('new-thread-signed-out-menu-close','PickProvider',{'provider':'claude'})
    assert sql('select count(*) from threads')=='0'
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
capture('new-thread-model-display-name')
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

### Codex

Run this fake-only section after the existing drive and before Evidence.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PYCODEX'
from driver import *
prepare()
assert LANE == 'fake'
assert sql('select count(*) from threads')=='0'
(DATA/'fake-cli').unlink()
action('codex-new-thread-open','NewThread')
action('codex-new-thread-provider','PickProvider',{'provider':'codex'})
action('codex-new-thread-check-missing','RecheckProviders')
missing = poll('codex-new-thread-missing',lambda d:d['providers'][1]['readiness']=='not_installed')
assert missing['composer']['provider']=='codex' and not missing['composer']['send_enabled']
assert 'Install the Codex CLI' in missing['composer']['disabled_reason']
capture('codex-new-thread-missing')
(DATA/'fake-cli').symlink_to(pathlib.Path('target/debug/veyra-fake-cli').resolve())
action('codex-new-thread-recheck','RecheckProviders')
ready = poll('codex-new-thread-ready',lambda d:d['providers'][1]['readiness']=='ready' and bool(d['providers'][1]['models']))
assert ready['composer']['send_enabled']
assert ready['providers'][1]['version']=='0.161.0'
assert ready['providers'][1]['models'][0]['value']=='veyra-model'
assert ready['providers'][1]['models'][0]['supportedEffortLevels']==['medium']
action('codex-provider-menu','OpenProviderPicker')
menu = poll('codex-provider-menu-ready',lambda d:d['composer']['provider_picker_open'])
assert menu['composer']['provider_picker_labels'][1]=='Codex · Ready'
capture('codex-provider-menu-ready')
action('codex-provider-menu-close','PickProvider',{'provider':'codex'})
action('codex-new-thread-model','PickModel',{'model':'veyra-model'})
action('codex-new-thread-effort','PickEffort',{'effort':'medium'})
choice = ctl('codex-new-thread-choices','dump')
assert choice['composer']['model']=='veyra-model' and choice['composer']['effort']=='medium'
assert sql('select count(*) from threads')=='0'
capture('codex-new-thread')
action('codex-new-thread-return-claude','PickProvider',{'provider':'claude'})
action('codex-new-thread-reset','NewThread')
page_done('new-thread-codex')
print('PASS Codex missing to Ready; CLI model and effort; no thread created')
PYCODEX
```

### Grok

Run this fake-only section after Codex, while the database still holds zero threads.
The catalog-free signed-in probe offers one Default entry in each model and effort menu.
The signed-out fixture advertises only grok.com; the catalog fixture supplies the recorded model and effort lists.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PYGROK'
from driver import *
prepare()
assert LANE == 'fake'
assert sql('select count(*) from threads')=='0'
action('grok-new-thread-open','NewThread')
action('grok-new-thread-provider','PickProvider',{'provider':'grok'})
ready = poll('grok-new-thread-ready',lambda d:d['providers'][2]['readiness']=='ready')
assert ready['composer']['send_enabled']
assert ready['providers'][2]['version']=='1.0.60'
assert ready['providers'][2]['models']==[] and ready['providers'][2]['account']=='unknown'
action('grok-provider-menu','OpenProviderPicker')
menu = poll('grok-provider-menu-ready',lambda d:d['composer']['provider_picker_open'])
assert menu['composer']['provider_picker_labels'][2]=='Grok · Ready'
capture('grok-provider-menu-ready')
action('grok-provider-menu-close','PickProvider',{'provider':'grok'})
for name,chord in [('model','secondary-alt-m'),('effort','secondary-alt-e')]:
    action('grok-default-'+name,'Pick'+name.title())
    ctl('grok-default-'+name+'-key','keys',chord)
defaults = ctl('grok-new-thread-defaults','dump')
assert defaults['composer']['model'] is None and defaults['composer']['effort'] is None
assert sql('select count(*) from threads')=='0'
capture('grok-new-thread')
shutil.copy2(GROK_SOURCE/'signed-out.jsonl',FIXTURES/'probe-grok.jsonl')
action('grok-signed-out-recheck','RecheckProviders')
signed_out = poll('grok-new-thread-signed-out',lambda d:d['providers'][2]['readiness']=='signed_out')
assert not signed_out['composer']['send_enabled']
assert 'grok login' in signed_out['composer']['disabled_reason']
action('grok-signed-out-menu','OpenProviderPicker')
menu = poll('grok-provider-menu-signed-out',lambda d:d['composer']['provider_picker_open'])
assert menu['composer']['provider_picker_labels'][2]=='Grok · Signed out'
capture('grok-provider-menu-signed-out')
action('grok-signed-out-menu-close','PickProvider',{'provider':'grok'})
capture('grok-new-thread-signed-out')
shutil.copy2(GROK_SOURCE/'probe-catalog.jsonl',FIXTURES/'probe-grok.jsonl')
action('grok-catalog-recheck','RecheckProviders')
catalog = poll('grok-new-thread-catalog-ready',lambda d:d['providers'][2]['readiness']=='ready' and len(d['providers'][2]['models'])==4)
models = catalog['providers'][2]['models']
assert models[0]['value']=='grok-4.7'
assert next(m for m in models if m['value']=='grok-4.5')['supportedEffortLevels']==['high','medium','low']
action('grok-catalog-model-wide','PickModel',{'model':'grok-4.7'})
action('grok-catalog-xhigh','PickEffort',{'effort':'xhigh'})
action('grok-catalog-model-narrow','PickModel',{'model':'grok-4.5'})
assert ctl('grok-catalog-effort-reset','dump')['composer']['effort'] is None
for index,effort in enumerate(['high','medium','low',None,'high']):
    ctl('grok-catalog-effort-'+str(index),'keys','secondary-alt-e')
    assert ctl('grok-catalog-effort-after-'+str(index),'dump')['composer']['effort']==effort
    if effort is None: capture('grok-effort-default')
capture('grok-new-thread-catalog')
position = next(index for index,entry in enumerate(models) if entry['value']=='grok-4.5')
for index,model in enumerate([entry['value'] for entry in models[position+1:]]+[None]):
    ctl('grok-catalog-model-cycle-'+str(index),'keys','secondary-alt-m')
    assert ctl('grok-catalog-model-cycle-after-'+str(index),'dump')['composer']['model']==model
capture('grok-model-default')
action('grok-default-settings','OpenSettings')
action('grok-default-accept-edits','SetDefaultMode',{'mode':'accept_edits'})
poll('grok-default-setting-stored',lambda d:d['settings']['default_mode']=='accept_edits')
action('grok-default-new','NewThread')
action('grok-default-provider','PickProvider',{'provider':'grok'})
mode = ctl('grok-mode-default','dump')
assert mode['composer']['mode']=='ask_first'
assert mode['composer']['offered_modes']==['ask_first','full_access']
for index in range(16):
    frame = ctl('grok-mode-frame-'+str(index),'dump')['perf']['repaints']
    ctl('grok-mode-focus-'+str(index),'action','root::Tab')
    poll('grok-mode-repaint-'+str(index),lambda d:d['perf']['repaints']>frame)
    ctl('grok-mode-open-'+str(index),'keys','space')
    if ctl('grok-mode-state-'+str(index),'dump')['composer']['permission_picker_open']: break
    ctl('grok-mode-dismiss-'+str(index),'keys','escape')
else:
    raise AssertionError('permission chip could not be reached through window keys')
capture('grok-mode-offered-menu')
ctl('grok-mode-close','keys','escape')
poll('grok-mode-closed',lambda d:not d['composer']['permission_picker_open'])
for expected in ['full_access','ask_first']:
    ctl('grok-mode-cycle-'+expected,'keys','secondary-alt-a')
    assert ctl('grok-mode-cycle-after-'+expected,'dump')['composer']['mode']==expected
action('grok-mode-settings-restore','OpenSettings')
action('grok-mode-default-restore','SetDefaultMode',{'mode':'full_access'})
poll('grok-mode-default-restored',lambda d:d['settings']['default_mode']=='full_access')
action('grok-mode-new-restore','NewThread')
assert sql('select count(*) from threads')=='0'
shutil.copy2(GROK_SOURCE/'probe-grok.jsonl',FIXTURES/'probe-grok.jsonl')
action('grok-default-probe-restore','RecheckProviders')
poll('grok-default-probe-restored',lambda d:d['providers'][2]['readiness']=='ready' and d['providers'][2]['models']==[])
action('grok-new-thread-return-claude','PickProvider',{'provider':'claude'})
action('grok-new-thread-reset','NewThread')
page_done('new-thread-grok')
print('PASS Grok initialize readiness, Signed out mark, per-model catalog/defaults and Default-only fallback; no thread')
PYGROK
```

## Observable success

The permission chip’s visible menu is exposed by `composer.permission_picker_open`.
`composer.offered_modes` reports the same provider list used to build that menu.
In a Grok composer, open the permission chip through its focused button and capture the two entries, Ask first and Full access.

The Grok section preserves Ready and Signed out probes, bare version 1.0.60, advertised model names and each model's effort list, plus the catalog-free Default fallback.
`grok-provider-menu-signed-out.png` shows the sign-in mark and `grok-new-thread-signed-out.png` shows Send disabled with `grok login`.
`grok-new-thread-catalog.png` shows Grok 4.5 with high effort; changing from Grok 4.7 clears xhigh, and the effort key cycles high, medium, low and Default.
`grok-effort-default.png` and `grok-model-default.png` show both key cycles returning to Default after a pick.
Every Grok readiness and catalog step leaves zero threads.

The Codex section preserves missing and Ready `providers[1].readiness`, `version` and `models`, the picker label, `composer.model` and `composer.effort`.
Its SQLite thread count remains zero, and its PNGs show the disabled reason and selected CLI choices.

- `new-thread-missing.json`: Claude `not_installed`, Send disabled, installation command visible; inspect its PNG.
- `new-thread-ready.json`: Claude `ready`, models present, Send enabled after RecheckProviders.
- `new-thread-no-account.json`: account presence changes from `known` to `unknown`, models stay non-empty and Claude stays `ready` on the account-free probe; every readiness phase asserts `select count(*) from threads` is 0.
- `claude-source-probe-signed-out.json`: the signed-out probe reads `signed_out`, disables Send and names `claude auth login`.
  The API-key, third-party and silent-account probes read Ready in `claude_probe_api_key_reads_ready`, `claude_probe_third_party_reads_ready` and `claude_probe_silent_account_reads_ready` (`crates/engine/tests/replay_claude.rs`), which replay the same fixtures through the real probe code, so this page no longer drives them.
- `composer.provider`, `model`, `effort` and `mode` assert every choice; `composer_text` asserts typing and NewThread's reset.
- `new-thread-provider-menu.png` shows Not installed; `new-thread-provider-menu-ready.png` shows Ready after a probe completes while the menu remains open.
- `composer.provider_picker_labels` holds the labels used to build the open menu; its key opens the same menu.
- `new-thread.png`: upper-third question and composer, hooks/MCP hint, left-aligned Add project and Geist.
- `drive.log`: named `PickProvider`, `PickModel`, `PickEffort`, `PickPermission`, `RecheckProviders` and their keys, with exit codes.

The open provider picker keeps the Not installed repair hint above the input, where the popover cannot cover it.
Inspect the provider-menu PNG for that text before accepting the page.

### Dump fields

`composer` includes the chosen provider and effective permission mode, plus Send's readiness and disabled reason.
`composer.offered_modes` is the provider's menu and key-cycle list: three modes for Claude and Codex, Ask first and Full access for Grok.
`composer.permission_picker_open` reports the visible permission menu.
Settings keeps all three defaults; a new Grok composer applies Accept edits as Ask first.
`providers` reports each probe's path, version, models, readiness and `account`, either `known` or `unknown`, with no account value.
`composer.provider_picker_open` reports the picker opened by `OpenProviderPicker` or ⌘⌥⇧P.

### Evidence contract

```json
{
  "required": {
    "new-thread": [
      "new-thread-missing.json",
      "new-thread-ready.json",
      "new-thread-no-account.json",
      "new-thread-provider-menu.png",
      "new-thread-provider-menu-ready.png",
      "new-thread-ready.png",
      "new-thread.png",
      "new-thread-signed-out-hint.png",
      "new-thread-signed-out-menu.png",
      "new-thread-model-display-name.png"
    ],
    "new-thread-codex": [
      "codex-new-thread-missing.json",
      "codex-new-thread-missing.png",
      "codex-new-thread-ready.json",
      "codex-provider-menu-ready.png",
      "codex-new-thread-choices.json",
      "codex-new-thread.png"
    ],
    "new-thread-grok": [
      "grok-new-thread-ready.json",
      "grok-provider-menu-ready.png",
      "grok-new-thread-defaults.json",
      "grok-new-thread.png"
    ],
    "new-thread-live": [
      "new-thread-live.json",
      "new-thread-live.png"
    ]
  },
  "pairs": {
    "new-thread": [
      [
        "new-thread-missing",
        "new-thread-ready"
      ],
      [
        "new-thread-provider-menu",
        "new-thread-provider-menu-ready"
      ]
    ],
    "new-thread-codex": [
      [
        "codex-new-thread-missing",
        "codex-new-thread"
      ]
    ]
  }
}
```

## Gotchas

A missing account is unknown, not proof of sign-out.
Grok's signed-out fixture uses the recorded absence of cached_token, while its catalog fixture keeps the recorded modelState values masked.
Check again may briefly show Checking; the helper polls that state at the same 50 ms cadence as ctl wait.
The checks do not run a Claude turn or mutate any committed fixture.
The native folder picker and external keyboard delivery are manual entry points.
Update Status from candidate acceptance.
