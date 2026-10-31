---
name: verify-veyra
description: Verify Veyra's native macOS window and CLI after UI, launch, logging, data-folder, or debug-driver changes. Launch an isolated instance, drive gpui actions through veyra ctl, capture the window, and preserve evidence after cleanup.
---

# Verify Veyra

Read [the feature map](features/README.md) before choosing a recipe.
Run every section in a fresh `bash -c` from this checkout, or let [`bin/run`](#run) run them.
Export one fresh `RUN` of eight lowercase hex characters into Launch and every later shell, as `bin/run` does for each attempt; by hand, `export RUN=$(uuidgen | cut -c1-8 | tr A-Z a-z)`.
Each section recovers its records from `target/verify-veyra/$RUN`.
Never drive an instance this run did not start.
Screen Recording must be granted to the host running `screencapture`.
External Accessibility input is not granted here; driver keys prove gpui dispatch, not native keyboard delivery.

## Run

Tier S of any verify-veyra change runs `--check-guards`.

Start with the offline receipt, then let the lead arm the native window for its exact identity.
`bin/run --tier-s-write DIR` builds debug and release artifacts and capture helpers, runs the workspace tests (classes A and B included), debug clippy, fmt, release test compilation, asserts the named boundary/text/map and nine headless results in that workspace output, then runs `--check-only` and `--check-guards` once each.
It writes `DIR/receipt.json`, the command outputs and their hashes, helper hashes, and HEAD/diff/binary identity.
A native mode requires `--tier-s DIR/receipt.json`; missing commands, failed commands, changed outputs/helpers/binaries, another identity or a receipt older than 24 hours refuse Launch.
Launch copies the already compiled helpers and does no build.
Every relaunch checks the receipt again.

Native modes own quiet-machine admission; no external wrapper is needed.
Before native work, the runner and each guard probe call the existing gate: one-minute load below 5.0, sampled every 15 seconds, for at most 20 minutes and never beyond the exact GO cutoff.
After waiting they revalidate their admitted inputs; a held, unreadable or expired admission launches nothing.

`bin/run --smoke --base BASE` runs Launch, Doctor, app-launch's first section, each selected fake page's `Native smoke`, Doctor, Evidence and Cleanup.
Provider tokens select that page's smoke scene; live permission cells are reported due and remain in their separately consented lane.
A production hunk in a glass file selects `settings/Glass smoke` as well.
A test hunk never selects a page, release, glass or a guard.
Smoke uses the debug quiet clock and never writes Status lines or status proposals.
A selected release token still runs the release section.
`--then-guard --baseline DIR --part PART` runs the slice-6 frame-cost guard **before** that smoke in the same away window.
A failed guard cannot enter smoke.
The guard uses the fixed 24,60,960,740 placement on both sides, records stream `perf.repaints`, runs five pairs with the ten-pair extension rule, both idle focus states and normal/reduced Working scenes, with recording off.
A cover trip before the first baseline measurement discards the entire set, waits 60 seconds and starts the set once more.
A candidate trip in pair 1, any trip after a measurement starts, or a second pre-pair trip stops the set.
No measured baseline or candidate is reused across that restart; the full unfiltered cover guard runs again.
Its `guard-done.json` is the lead's signal to release the arch-plan pause.
The only baseline is the primary checkout's retained `.tstack/tasks/v001/evidence/perf/bin/5397b7b936f0e69c7c24c892f3f585328681b800/`, code identity `b2357c36d7a81924d41f97d94b17dc54a0983d7785ffd5895c549c328032e708`.
`bin/frame_guard.py` pins its approved manifest SHA-256 and all three binary hashes, and refuses another directory or a self-consistent impostor.
`--part` is required for the guard and has no default.
The current legacy candidate dump requires `--part 6c`; palette/toast fields require `--part 6d`, and a 6d request without those fields stops.
The 6d candidate idle variant closes the palette, creates an empty-Send notice toast and requires that toast to remain at rest in `dump` before, throughout and after both ten-second focus measurements.
The candidate exposes `dump.palette.open` and `dump.toasts`; an older candidate without them stops with `6d idle scene unavailable`; it never falls back to the 6c candidate scene.
The retained slice-5 baseline uses its earlier surface without palette/toasts, with the same drawer/composer focus and Translucent background.

`bin/run --slice-end` and `bin/run --gate` run the full fake map, including release.
Only `--gate` permits `--real-quiet`.
Full runs emit `status-proposals.json`; after reading every queued native image, the lead applies it with `bin/stamp-status DIR --reads READS.json` in the docs-only status change.
No fake run stamps permission-matrix's live cells.
A full run's native read manifest records each capture path, SHA-256 and verdict.
Status stamping uses the candidate commit's fingerprint, so the candidate must be committed before that full run.

Every native launch mode also requires `--go GO.json` with `launch_authorized: true`, the exact `identity` from Tier S, `issued_epoch` and `cutoff_epoch`.
A guard additionally requires `frame_cost_guard_authorized: true` and `arch_plan_paused: true`.
The lead writes that file only after the human's GO; the runner never creates approval.
`--deadline EPOCH` cannot exceed that cutoff.
HEAD, `git diff --binary HEAD`'s SHA-256, untracked helper hashes and the debug/fake/release binaries' SHA-256 form the identity.
A helper edit therefore invalidates READY, Tier S and GO.
`--expect-head SHA` refuses another HEAD.

Each section runs in a fresh `bash -c` with exported RUN and the admission paths.
The runner records `sections.json`, the transcript, the plan and required receipts under `DIR/attempt-1/`, and preserves proof as `DIR/attempt-1/run` after Cleanup.
A capture has at most three attempts, discarding the image after every guard failure.
Three tripped attempts block that section; the run then health-checks the owned process, restores a known composer and continues through independent sections.
Ordinary assertion failures are collected too.
Only an exhausted capture observation blocks a section; a recovered retry clears that flag, so a later assertion is FAILED.
Use `--sections PAGE/HEADING ...` in the next armed window to run only the blocked sections, with Launch, Doctor, app-launch's first section, Evidence and Cleanup.
Passed sections are not repeated.
The outcome lists each section as passed, failed or blocked.
Every section failure records fresh HID idle and screen-lock observations, including ordinary poll timeouts and assertion failures.
A generic section-exit observation is labelled as such; unknown HID or lock state cannot authorize a failure capture.
Each failure capture has a unique section/ordinal label, a freshness record and its own retained path and hash in the outcome.

Smoke drains are scoped to the current scene's thread IDs.
The multi-agent return path proves the child task Interrupted and its provider exited, with the parent Held/stopped and its report wake pending; that valid idle parent may remain until Cleanup.
The shell drawer and other scenes do not wait on earlier scenes' rows, and their return paths restore owned UI state even after a failed drain.

The run stops on ownership, Cleanup, identity or restore failure, or a privacy guard it cannot enforce, including a missing backdrop.
The screen-lock guard, HID gate, frontmost/coverage guards and owned-process rules remain in force.
A deadline leaves unstarted sections blocked and still runs Cleanup.
SIGTERM, SIGHUP and SIGINT enter Cleanup before a verdict; pending signals also prevent DONE.
DONE requires every planned section passed, Cleanup exit zero, no scratch residue, unchanged identity and retained proof.
Independent failures yield FAILED after collection; blocked sections yield BLOCKED when no independent failure exists.
Unreadable required receipts or lost proof prevent DONE.
No interrupted or partial run counts as passed.

```text
bin/run --check-only
bin/run --check-guards --evidence DIR
bin/run --tier-s-write DIR
bin/run --smoke --base BASE --tier-s TIER/receipt.json --go GO.json --deadline EPOCH --evidence DIR
bin/run --smoke --base BASE --then-guard --part 6c --baseline BASELINE --tier-s TIER/receipt.json --go GO.json --deadline EPOCH --evidence DIR
bin/run --sections 'settings/Native smoke' --tier-s TIER/receipt.json --go GO.json --deadline EPOCH --evidence DIR
bin/run --slice-end --tier-s TIER/receipt.json --go GO.json --deadline EPOCH --evidence DIR
bin/run --gate --real-quiet --tier-s TIER/receipt.json --go GO.json --deadline EPOCH --evidence DIR
```

Paths in this block are relative to `.agents/skills/verify-veyra/`; run them from the checkout with that prefix.
`--check-only` compiles all shell blocks, Python heredocs and Python helpers, checks the map and guard structure, and runs the offline self-tests.
They call the real `prepare()` through selection/admission/environment setup, exercise the actual guard-then-smoke dispatch and failed-guard refusal, collect independent failures through Cleanup, classify test hunks and verify repaint/file freshness and plain-message assertions.
Native/process/window/capture boundaries are stubbed in those tests.
The HID/lock checks execute Launch’s real heredoc and `start()`, and capture checks execute the actual batch and driver paths.
The raw-I/O cases also run the native frame adapter’s launch, normal/reduced motion scene, per-read identity checks and the real glass/backdrop entries.
The test fixture supplies OS results and tiny on-disk trust inputs; guard functions and scheduling decisions are unchanged.
These tests prove the harness offline; they leave the native smoke and compositor behavior for an armed window on the committed head.

### Mutants and retention

At slice end or the gate, `--native-mutants PREPARED.json` admits at most three mutants, each for a class C property.
Prepare them offline with `bin/run --prepare-mutants MANIFEST.json --evidence DIR`.
Each manifest case names `name`, `property`, `file`, the unique `old`/`new` replacement, `sections`, `capture`, `scene_runs_mutated_line: true` and either `judge: assertion` with `expected_assertion`, or `judge: read`.
The reviewer checks that the scene executes the changed line.
Preparation writes a fresh complete Tier S receipt and binary snapshots for each mutation, restoring exact original source and binary bytes in `finally` before clearing its journal.
The lead adds each case's own `go` path to `prepared.json`, and the parent GO names that file's `mutant_manifest_sha256`.
Native execution installs those already proved bytes, runs their routed scene, collects assertion results, and restores the exact originals even after failure.
A read-judged run seals its own run id, guarded capture hash, freshness record and native outcome, then stops at `awaiting lead read`; the full run cannot continue yet.
After reading that fresh image, the lead records the second step with `bin/run --mutant-read RUN HASH killed|survived --evidence DIR`, where DIR is the same mutant batch evidence root.
The command revalidates that run’s sealed capture and refuses another run’s hash, a changed image, an unknown run or a second verdict.
Only a matching lead verdict counts toward the mutant aggregate; a queue entry or retained image from another run cannot kill it.
A later `--native-mutants` or slice-end full run using the same evidence directory re-reads recorded verdicts by run id and capture hash.
Decided kills stay decided, surviving mutants stop the run, and an altered capture or verdict binding is refused.
Recording a read launches nothing.
An uncleared `mutation-in-flight.json` is a restore failure to resolve before another run.
`bin/run --mutant-route NAME` prints each historical 6c route from `bin/mutant-routes.json`, including Grok's wrapped toolbar prerequisite, the second Codex MCP dock and Settings' sheet scene.
Those routes do not re-admit retired draw checks as native mutants.
Admitted draw defects belong to the tests in [proof ownership](proof-ownership.md).
Plain AssertionError messages are parsed as messages; a malformed literal cannot crash routing.
Retained-image replay is only a checker self-test and never a native mutant kill.

After the lead's part-close status commit, `bin/prune ROOT --discard DISCARD.json --gate-cites GATE --close-cites CLOSE --keep KEEP --status-commit SHA` prints the dry-run JSON and its digest.
`--apply DIGEST` applies only that unchanged plan on the lead's authorization.
The status commit records Tier S identity, guard folder/verdict, smoke run/pages and review aggregate/head/round.
Keep Tier S receipts; guard summaries and per-run results; smoke receipts and every capture read; one failure capture and logs per stopped attempt; all red/green receipts; and review aggregates.
The keep manifest names every retained proof, and gate/Close citations protect their files and descendants regardless of discard requests.
The tool also preserves baselines under `evidence/perf/bin/`, refuses symlinks and flags a part above 1 GB.
Unknown files are kept.
Discard entries explicitly name studies, superseded copies, staged fixtures, synthesized images or debug snapshots.
Store study sources once by content with compact manifests.
The lead's prune, after this pass lands, removes replaced per-part scripts; this pass edits only this skill.

## Selection

`bin/select-pages BASE [HEAD]` prints the tokens a change from BASE selects, one per line; without HEAD it reads the working tree, untracked files included.
`bin/pages.py` reads its lists from this block, so the block is their one owner:

```text
shared    Cargo.toml Cargo.lock crates/veyra/src/ui/mod.rs crates/engine/src/actor.rs
          .agents/skills/verify-veyra/SKILL.md .agents/skills/verify-veyra/bin/**
no-page   crates/*/tests/*.rs crates/*/tests/common/** crates/engine/src/tests/** crates/veyra/src/tests.rs
          crates/veyra/src/verify_map.rs crates/veyra/src/ui/headless.rs crates/engine/src/platform/boundary.rs crates/engine/src/provider/scripted.rs
          ARCHITECTURE.md AGENTS.md clippy.toml scripts/** .agents/skills/verify-veyra/features/README.md
codex     crates/engine/src/provider/codex.rs crates/engine/src/provider/codex/** crates/engine/tests/fixtures/codex/**
grok      crates/engine/src/provider/grok.rs crates/engine/tests/fixtures/grok/**
provider  crates/engine/src/provider/mod.rs crates/engine/src/provider/session.rs crates/engine/src/provider/env.rs
          crates/engine/src/provider/lines.rs crates/engine/src/provider/jsonrpc.rs crates/engine/src/mcp.rs
          crates/engine/src/tools.rs crates/veyra/src/relay.rs
release   crates/veyra/src/control.rs crates/veyra/src/main.rs crates/veyra/build.rs
glass     crates/veyra/src/ui/theme.rs crates/veyra/src/ui/mod.rs crates/veyra/src/ui/sidebar.rs
          crates/veyra/src/ui/settings.rs crates/veyra/src/main.rs crates/engine/src/platform/macos.rs
          Cargo.toml Cargo.lock
frame     crates/veyra/src/** crates/engine/src/actor.rs Cargo.toml Cargo.lock
activation crates/veyra/src/ui/mod.rs crates/veyra/src/ui/notify.rs
clipboard  crates/veyra/src/ui/terminal_view.rs
quit       crates/veyra/src/ui/mod.rs crates/veyra/src/main.rs crates/engine/src/lib.rs
pty        crates/engine/src/terminal.rs crates/engine/src/platform/macos.rs
           crates/veyra/src/ui/drawer.rs crates/veyra/src/ui/terminal_view.rs
```

Selection classifies both sides of every changed hunk before applying the file lists.
A hunk wholly inside a `#[cfg(test)]` item, a file whose `mod` declaration is test-only, any crate's `tests/`, or `crates/engine/src/tests/` selects nothing, including no `release` token for a test's `cfg(debug_assertions)` line.
A mixed production/test file still selects by its whole file when any production hunk remains.
The offline selector test includes the admitted `fd4adea..04ac4f3` diff and its fixture shape.

A `shared` file selects every page and provider section, `permission-matrix` included.
A `no-page` file selects nothing, because it cannot change the window.
A page's own feature file selects that page unless only its Status line changed.
Any other file selects each page whose `Covers:` globs match it, and a file no page covers selects every page and provider section, `permission-matrix` included.
Within a matched page, a `codex` file selects only its Codex section, a `grok` file only its Grok section, a `provider` file all of its sections, and any other file its own sections.
`release` follows the `release` files, a changed line naming `cfg(debug_assertions)` and a changed `[profile` section of `Cargo.toml`; a phase gate runs it too.
A selected `permission-matrix` says its live grid is due; driving it still needs a consented live lane.

`bin/fresh-pages HEAD [--versions FILE]` prints `PAGE STATE` for each page whose Status FINGERPRINT still equals the page's fingerprint at HEAD and which `select-pages COMMIT HEAD` does not select, so a shared or uncovered change since the Status COMMIT makes every page stale, the live rows included.
With `--versions`, a file of `provider: version` lines, it also prints `permission-matrix/PROVIDER STATE` for each live row whose recorded version equals one whitespace-separated token of that provider's line exactly.
The recorded version is the whitespace token after the page's first `Claude`, `Codex` or `Grok` that starts with a digit, less one trailing comma, semicolon or full stop of its sentence; the line's tokens are compared as they are, so `0.9` matches neither `0.9.` nor `0.9.1`.
`bin/fresh-pages --print COMMIT` prints each page's fingerprint at COMMIT, for the Status line.
A FINGERPRINT is the SHA-256 over `BLOB PATH` lines, sorted by path, for the files `git ls-tree -r COMMIT` lists under the page's `Covers:` globs, plus the page's own file, whose blob id is computed without its Status line.
So a docs-only status commit leaves every fingerprint as it was.
`python3 .agents/skills/verify-veyra/bin/pages.py self-test` checks selection rules, test-only hunks and Status-blind fingerprints without any Git writes.

## Launch

The default verification lane uses only the fake CLI.
Its binary path deliberately starts missing, so [new-thread](features/new-thread.md) can prove installation and Check again.
Every section imports [`bin/driver.py`](bin/driver.py), which shares logging, bounded polling, fixture staging and the one expected database-table list.
The driver does not set application state; its writes are fixture copies, screenshots and proof.
Each capture queues a native read through [`bin/native_reads.py`](bin/native_reads.py).
The [proof ownership](proof-ownership.md) lists admitted class A/B tests and remaining native reads.
Vision OCR has no pass/fail path.
For an authorized live lane, export `VEYRA_VERIFY_LANE=live` before Launch.
Launch then removes replay overrides, records to `$PROOF/rec`, and the new-thread and chat pages use their live branches.
A live lane needs separate human authorization because it runs the installed CLI and spends its quota.
The first launch waits in the driver's `gate()` until the screen is unlocked, the HID idle time is between 120 and 9,000 seconds and no `veyra` process has run for 10 seconds; it records the idle time in `input-idle-launch.json`.
Every later relaunch through `start()` refuses a running `veyra` this run did not start: one that is neither an owned app pid nor marked with an owned app's `VEYRA_PARENT_PID`.
When yabai is installed, each owned launch first adds a temporary uniquely labelled one-shot `app=^veyra$ manage=off` rule. The window must report unmanaged and floating before any scene; the recipe never floats a tiled window. Cleanup removes only labels recorded by this run, including an unconsumed rule after a failed launch. No empty desktop is needed. The owned window is centred at 960×740 points inside its display's visible frame; the initial pending-PATH handshake precedes this normalization.
Each fake-lane launch records to its own `rec/NN-LABEL` folder, named in `rec-current`.
The debug-only VEYRA_REAP_PIDS filter restricts startup reaping to the run's saved provider pids, including after a crash.
An empty list permits no startup signals.
The debug-only VEYRA_DEBUG_TERMINAL_GRID=COLSxROWS override is read once at window-app launch; decimal columns 2–1000 and rows 1–1000 fix the PTY/emulator grid at the terminal area’s top left, clipped to that area.
An absent or invalid value uses layout sizing; release contains neither the variable nor its override code.
The release check refuses to launch while any same-user marked process names a dead parent.
Its preflight and Cleanup share an in-memory marker reader and report counts only.
Both lanes remove the caller's Claude session variables while keeping `CLAUDE_CODE_USE_*` user configuration.
The fake lane puts a run-owned `git` first on the app's PATH; it answers `git branch --show-current` with `fixture-branch` only while `$DATA/branch-on` exists, and otherwise runs the real `git`.
Every app launch runs from `$DATA/appcwd`; probes run in Veyra's private data folder.
The fake lane uses an owned login-shell script blocked on a run-owned release file, so the first ready line and dump prove the window opens while `login_path` is `pending`.
Doctor sets Solid through `veyra::SetWindowBackground` before a page captures the window.
Only a page proving Translucent starts the compiled backdrop; Launch compiles it before opening the app.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export a fresh RUN of eight lowercase hex characters first}
printf '%s\n' "$RUN" | grep -Eqx '[0-9a-f]{8}'
DATA=${VEYRA_VERIFY_TMP:-/tmp/veyra-verify-}$RUN
PROOF=target/verify-veyra/$RUN
BIN=.agents/skills/verify-veyra/bin
case "$DATA" in /tmp/veyra-*) ;; *) exit 1 ;; esac
test ! -e "$DATA"
test ! -e "$PROOF"
mkdir -p "$PROOF/fixtures" "$DATA/project" "$DATA/appcwd"
printf '%s\n' "${VEYRA_VERIFY_LANE:-fake}" >"$PROOF/lane"
: >"$PROOF/pages-run"
PYTHONPATH="$BIN" python3 - <<'PYTIER'
from orchestration import code_identity, require_tier
import os, pathlib, shutil
receipt = pathlib.Path(os.environ['VEYRA_VERIFY_TIER_S'])
require_tier(receipt,code_identity())
proof = pathlib.Path('target/verify-veyra')/os.environ['RUN']
for source in (receipt.parent/'tools').iterdir():
    shutil.copy2(source,proof/source.name)
PYTIER
cp crates/engine/tests/fixtures/claude/probe-initialize.jsonl "$PROOF/fixtures/probe.jsonl"
cp crates/engine/tests/fixtures/codex/probe-ready.jsonl "$PROOF/fixtures/probe-codex.jsonl"
cp crates/engine/tests/fixtures/grok/probe-grok.jsonl "$PROOF/fixtures/probe-grok.jsonl"
printf 'RUN=%s\n' "$RUN"
RUN="$RUN" PYTHONPATH="$BIN" python3 - <<'PY'
from driver import *
gate()
(PROOF/'configured-default-mode').write_text('full_access\n')
if LANE == 'fake': make_git_shim()
if LANE == 'fake':
    shell = DATA/'login-shell'
    release = DATA/'login-path-release'
    shell.write_text('#!/bin/sh\nattempt=0\n'
        'while [ ! -e "'+str(release)+'" ]; do\n'
        '  [ "$attempt" -lt 600 ] || exit 124\n'
        '  /bin/sleep 0.05\n  attempt=$((attempt+1))\ndone\nexec /bin/sh "$@"\n')
    shell.chmod(0o700)
    ENV['SHELL'] = str(shell)
pid = start('launch', normalize=False)
pending = ctl('launch-login-path','dump')
assert pending['login_path'] in ['pending','loaded','fallback']
if LANE == 'fake': assert pending['login_path'] == 'pending', pending['login_path']
if LANE == 'fake':
    release.touch()
    poll('launch-login-path-loaded',lambda d:d['login_path']=='loaded')
assert f'veyra ready pid={pid} data_dir={DATA}' in (PROOF/'launch.stderr').read_text()
normalize_owned_window('launch')
print(f"PASS launch RUN={RUN} PID={pid} PGID={(PROOF/'pgid').read_text().strip()} ID={(PROOF/'window-id').read_text().strip()}")
PY
```

The block must exit 0.
A failed launch may still own a process: run Cleanup before reporting.
Launches use `start_new_session=True`, so the app's pid is its process group.
The slowed `ok` fixture uses 500 ms, the design's stderr-wait bound, between its two text deltas and before the result.
The background proof uses three-second intervals, the design's interrupt grace, before ambient-only and empty lists.
The family proof delays its child reply by two seconds so selection precedes the text batch and the visible counter can observe it.
Those delays affect proof copies only; the committed capture-derived fixture is unchanged.
Fixture staging uses each database thread’s turn count and the current app’s recorded structured-spawn names, with eight future slots for terminal entries and resume fallback.
A stopped app stages from slot one because its next launch resets the fake CLI’s lane counters.
Identical fixture content lives once under fixtures/records, and named spawn files are atomically replaced with hard links to those immutable contents.
Use fixture_source and link_fixture for explicit spawn files too; writing through a hard link would change every alias.

## Doctor

Doctor uses `owned_instance_pid()` for the owned process, revision and data folder, then checks the window and appearance.
`orchestration.code_identity()` hashes source and binaries; `run.checkout_identity()` applies it to the selected checkout.
The frame probe uses the same `owned_instance_pid()` at launch and checks its owned pid, group, data folder and expected candidate or pinned baseline revision on every measured dump.
The macOS appearance read treats a missing `AppleInterfaceStyle` key as light.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
start = time.monotonic()
pid = owned_instance_pid()
dump = ctl('doctor','dump')
seconds = time.monotonic()-start
(PROOF/'doctor-time.txt').write_text(f'{seconds:.3f}\n')
assert seconds < 2, seconds
style = subprocess.run(['defaults','read','-g','AppleInterfaceStyle'],capture_output=True,text=True)
expected = 'dark' if style.returncode == 0 and style.stdout.strip().lower() == 'dark' else 'light'
assert dump['appearance'] == expected, (dump['appearance'],expected)
window = window_id(pid)
assert window == (PROOF/'window-id').read_text().strip()
poll('doctor-login-path',lambda d:d['login_path'] != 'pending')
action('doctor-solid','SetWindowBackground',{'background':'solid'})
poll('doctor-solid-applied',lambda d:d['look']['background']=='solid')
bounds = subprocess.check_output([str(PROOF/'winid'),str(pid),'--bounds'],text=True)
(PROOF/'bounds.txt').write_text(bounds)
print(f'PASS doctor pid={pid} appearance={expected} seconds={seconds:.3f} window={window}')
PY
```

## Drive

Run [app-launch](features/app-launch.md), then [new-thread](features/new-thread.md), [chat-send-stream](features/chat-send-stream.md) and [settings](features/settings.md).
Also run [approvals-questions](features/approvals-questions.md), [stop-interrupt](features/stop-interrupt.md) and [restart-recovery](features/restart-recovery.md).
Run each page's Codex and Grok sections in the fake lane after its existing drive.
Every section after new-thread's first starts with the driver's `prepare()`, which adds the project, installs the fake CLI and waits for Ready only when no earlier page did, then opens a new Claude composer, so a page can run alone after Launch, Doctor and app-launch's first section, or after any earlier section without a relaunch.
Before any provider drive, wait with `poll('login-path-ready', lambda d: d['login_path'] != 'pending')`; Doctor performs this wait for the first launch.
After a relaunch, wait again before checking providers.
`login_path` reads `pending`, `loaded` or `fallback`; Checking remains until the first probe answers.
Then run [multi-agent](features/multi-agent.md) for the thread tree, family actions, report delivery and quiet hint.
Run [command-palette](features/command-palette.md) for context filtering and focus return, and [toasts](features/toasts.md) for notices and family attention.
Run [shell-drawer](features/shell-drawer.md) for owned shell fixtures, tab keys, focus, shutdown and one flood.
The fake lane runs those pages without a provider login.
[permission-matrix](features/permission-matrix.md) requires separately authorized live providers.
Use the same exported `RUN` in every fresh shell.
`action NAME [JSON]` builds a registered gpui action.
`action root::Tab` advances gpui-kit keyboard focus, including the composer's chip buttons.
AllowOnce, AllowInThread and Deny accept a request id; AnswerQuestion accepts request, question index and option index.
Stop accepts a target containing thread and turn ids; without a payload it uses the rendered running or pending-admission turn.
Without a request payload, consent actions use the rendered dock target, as focused keyboard chords do.
For an explicit AnswerQuestion request, supply its question index as well.
Action and key replies acknowledge dispatch; poll `dump` for the resulting view or use `ctl wait` for a thread state.
`keys KEYSTROKES` parses the whole sequence before dispatch, then sends key-down followed by key-up for each keystroke through the window.
Buttons activate on key-up; terminal input consumes key-down only.
`text STRING` uses the focused textarea's public input handler or dispatches character key events while the shell drawer has focus; Settings has no text handler.
Enter or ⌘↵ sends through `veyra::Send`; Shift+Enter inserts a newline.
`wait THREAD STATUS [SECONDS]` is a client-only loop over `dump`, polling every 50 ms for 30 seconds by default.
It exits 0 for the status, 1 for timeout or a driver/connection error, 2 for bad usage and 3 for a missing or archived thread.
A timeout writes `veyra ctl wait timed out:` to stderr.
A connection or driver failure writes `veyra ctl driver error:` with its cause.
Every exit-1 expectation must also assert its named stderr line.
`quit` dispatches `veyra::ConfirmQuit`; `veyra::Quit` opens the sheet while work runs.
A timeout does not cancel an enqueued action; run Doctor before retrying.
Commands remain bounded at 64 KiB; the client accepts replies up to 4 MiB so the seeded 5,000-item transcript can be measured without truncating its dump.
After another Send on an existing thread, gate `wait Done` on the new turn row or a changed status so the preceding Done cannot satisfy it.

The read-only dump reports identity, the current view and the look in force.
Each page owns the dump fields and required evidence for the behaviour it drives.
Its `### Evidence contract` JSON block lists that page's required artifacts.
Evidence checks completed tokens' required artifacts through `bin/pages.py`; the glass recipes retain and judge their own pairs.
Before every capture, the debug-only `ActivateWindow` action activates Veyra from inside the app.
The helper waits for two separate rendered frames and 100 ms before inspecting and capturing the window.
Solid captures use `screencapture -x -o -l WINDOW` and keep the existing on-screen and frontmost guards.
Translucent captures use `screencapture -x -R x,y,w,h` of the exact on-screen window rectangle.
They require `bin/backdrop.swift` behind Veyra, with its run-owned pid and window ids in `backdrop-windows.json`.
Immediately before and after each region capture, the helper checks that Veyra is frontmost, its bounds equal the rectangle, the rectangle is wholly covered by one backdrop window on one display, and no other on-screen window above Veyra intersects it.
A changed guard discards the capture; the helper makes at most three attempts in total.
A measured coarse and inverse pair uses one placement.
If the owned-window fallback places either half, discard both and retake both at that rectangle within three pair attempts, keeping every guard and the exact pair check.
The Settings page places a second run-owned backdrop above Veyra and requires the guard to reject it without retaining a PNG.
Veyra must stay frontmost during captures.
Freshness requires an advanced `perf.repaints` counter on the owned pid and a newly written image file.
Identical pixels are valid after a repaint; the guarded glass recipe keeps its measured pairs.
Concurrent windows may cause a capture to fail, but cannot make a covered or off-screen frame pass.
A dump proves state, not layout; inspect each PNG.

## Recover section

After a failed section, Doctor verifies identity before this reset to a known composer.
A reset failure stops the run as a restore failure.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PYRECOVER'
from driver import *
owned_instance_pid()
ctl('recovery-escape','keys','escape')
action('recovery-composer','FocusComposer')
prepare()
poll('recovery-known-composer',lambda d:d['view']=='new_thread' and d['focus']=='composer')
PYRECOVER
```

## Evidence

Run this after the chosen pages and the final Doctor.
Launch initializes `configured-default-mode` to the known Full access default; each successful `SetDefaultMode` action records its requested mode there.
Evidence compares SQLite with that independent intent ledger, and never computes its expected mode from the database query under test.
Each passed section records its name in `pages-run`; Evidence checks only those sections.
The required files enforce the relaunch, release and feature proofs.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
PROOF=target/verify-veyra/$RUN
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - <<'PY'
from driver import *
owned_instance_pid()
ctl('dump-after','dump')
pages = set((PROOF/'pages-run').read_text().split())
assert pages, 'no completed page'
database('final',configured_default_mode())
capture('window')
shutil.copy2(DATA/'logs/veyra.log',PROOF/'veyra.log')
(PROOF/'git-identity.txt').write_text(subprocess.check_output(['git','rev-parse','HEAD'],text=True)
    +subprocess.check_output(['git','status','--short'],text=True))
import pages as page_map
required = page_map.evidence_contract(pathlib.Path.cwd())
assert pages <= required.keys(), pages
for name in ['drive.log','sql.log',*(name for page in pages for name in required[page])]:
    assert (PROOF/name).stat().st_size > 0, name
if 'app-launch' in pages:
    assert len({(PROOF/(label+'-inode.txt')).read_text() for label in ['initial','after-migration']}) == 1
for pid in (PROOF/'owned-app-pids').read_text().split():
    assert f'veyra ready pid={pid} data_dir={DATA}' in (PROOF/'veyra.log').read_text()
if 'chat-send-stream-live' in pages:
    assert list((PROOF/'rec').glob('t*-s*.jsonl')), 'missing live session recording'
print('PASS evidence pages='+','.join(sorted(pages))+'; identity, database, relaunches, screenshots and release kept')
PY
```

Inspect the PNGs themselves for Geist, the 240 px initial sidebar, 44 px header, upper-third new-thread block, centred chat column and composer, collapsed tools/reasoning, failure cause and Settings copy.
Read the status-mark captures at full size: Working is an open accent arc, Needs you a filled warning diamond,
Done a success check, Failed a danger cross, Idle a muted dot, Held two muted pause bars and Interrupted a muted square.
Read the sidebar, header and family card separately. The row-focus sequence on multi-agent shows the inset accent
ring apart from the selected plate; keyboard focus adds no selection wash. Provider and family marks are Veyra drawings.
The transcript caps at 752 px, the composer at 784 px, with 40 px gutters.
The requested app bounds are 1280×800; when yabai is installed the owned one-shot rule prevents tiling and the recipe normalizes to the retained 960×740 geometry. Use Doctor's actual bounds.
Do not count native typing, pointer dragging, IME or a manual theme change as driver proof.

## Cleanup

Read `live_pgid` and `live_pid` into memory first, before quitting or removing anything; a missing `veyra.db` or `threads` table leaves both lists empty, and a read that fails is marked unreadable.
A proof write before DATA is removed that fails is marked too, and Cleanup still makes every stop.
Quit the app and SIGKILL its group, then SIGTERM the provider groups and SIGKILL them one second later, so a member that outlived its leader is stopped too.
Signal only groups saved in this run's database and the app group this run started.
The leftover checks retry for five seconds and fail only for saved run-owned pids or groups.
Other processes carrying the parent marker are allowed up to thirty seconds to drain; any remaining marker then fails Cleanup, regardless of its group, with counts and pids only.
The marker check uses Darwin's `KERN_PROCARGS2` through `sysctl` in memory, never printing arguments or environment values.
It reads same-user parent markers in memory, then counts only saved app/provider pids and members of saved provider groups as owned leftovers.
macOS refuses argument reads for privileged processes whose real uid is the user but effective uid is root; a fake CLI lane cannot create such a process.
Zombies have no argument/environment block on Darwin and are excluded before the read; owned app pids must still be absent.
ESRCH or an empty argument block after either sysctl read means the process is gone.
EINVAL or an incomplete block from a still-listed process keeps it in the leftover count and retries within the same five-second window.
During Cleanup, EIO also keeps a still-listed process in the count for another read within the five-second owned-leftover window or the thirty-second other-marker drain; an unreadable process remaining at that window's deadline fails with its pid.
The startup marker check still refuses EIO immediately.
A failed database read, a failed proof write before DATA is removed, or a failed group or marker check leaves DATA for diagnosis.
A cleanup-listing write fails the section after DATA has been removed.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
if command -v sccache >/dev/null; then export RUSTC_WRAPPER=sccache; fi
set -eu
RUN=${RUN:?export the RUN that Launch printed}
DATA=${VEYRA_VERIFY_TMP:-/tmp/veyra-verify-}$RUN
PROOF=target/verify-veyra/$RUN
case "$DATA" in /tmp/veyra-*) ;; *) exit 1 ;; esac
if PYTHONPATH=.agents/skills/verify-veyra/bin python3 .agents/skills/verify-veyra/bin/driver.py --cleanup-yabai-rules "$PROOF" "$RUN" >"$PROOF/yabai-cleanup.stdout" 2>"$PROOF/yabai-cleanup.traceback"; then rules_clean=yes; else rules_clean=no; fi
PID=$(<"$PROOF/pid")
PGID=$(<"$PROOF/pgid")
test "$PID" = "$PGID"
groups=
pids=
tables=none
if test -f "$DATA/veyra.db"; then
    tables=$(sqlite3 "$DATA/veyra.db" "select count(*) from sqlite_master where type='table' and name='threads'") ||
        tables=unreadable
fi
if test "$tables" = 1; then
    groups=$(sqlite3 "$DATA/veyra.db" 'select live_pgid from threads where live_pgid is not null') || tables=unreadable
    pids=$(sqlite3 "$DATA/veyra.db" 'select live_pid from threads where live_pid is not null') || tables=unreadable
fi
written=yes
printf '%s\n' "$groups" >"$PROOF/provider-groups.txt" || written=no
printf '%s\n' "$pids" >"$PROOF/provider-pids.txt" || written=no
if kill -0 "$PID" 2>/dev/null; then
    test "$(ps -o pgid= -p "$PID" | tr -d ' ')" = "$PGID"
    { VEYRA_DATA_DIR="$DATA" target/debug/veyra ctl quit || true; } >"$PROOF/quit.json" 2>"$PROOF/quit.stderr" || written=no
fi
for attempt in $(seq 1 50); do
    kill -0 "$PID" 2>/dev/null || break
    sleep .1
done
if kill -0 "$PID" 2>/dev/null; then kill -KILL -- "-$PGID"; fi
for attempt in $(seq 1 50); do
    kill -0 "$PID" 2>/dev/null || break
    sleep .1
done
! kill -0 "$PID" 2>/dev/null
test ! -f "$DATA/logs/veyra.log" || cp "$DATA/logs/veyra.log" "$PROOF/veyra.log" || written=no
kill -KILL -- "-$PGID" 2>/dev/null || true
for group in $groups; do kill -TERM -- "-$group" 2>/dev/null || true; done
sleep 1
for group in $groups; do kill -KILL -- "-$group" 2>/dev/null || true; done
PYTHONPATH=.agents/skills/verify-veyra/bin python3 - "$PROOF" "$groups" "$pids" <<'PY'
import ctypes, errno, json, os, pathlib, struct, subprocess, sys, time
proof = pathlib.Path(sys.argv[1])
backdrop_file = proof/'notifications-backdrop-pid'
if backdrop_file.exists():
    backdrop_pid = int(backdrop_file.read_text())
    from driver import stop_owned_backdrop
    ownership = json.loads((proof/'notifications-backdrop-ownership.json').read_text())
    assert ownership['pid'] == backdrop_pid
    stop_owned_backdrop(backdrop_pid,ownership['executable'])
    (proof/'notifications-backdrop-cleanup.json').write_text(json.dumps({'pid':backdrop_pid,'stopped':True})+'\n')
parents = {int(pid) for pid in (proof/'owned-app-pids').read_text().split()}
groups = {int(group) for group in sys.argv[2].split()}
def groups_gone():
    remaining = []
    for group in groups:
        try: os.killpg(group,0)
        except ProcessLookupError: continue
        remaining.append(group)
    return remaining
from driver import marked_processes, saved_marked_processes
pids = parents | {int(pid) for pid in sys.argv[3].split()}
end = time.monotonic()+5
while True:
    remaining_groups = groups_gone()
    marked = marked_processes(parents, retry_exiting=True)
    remaining_markers = saved_marked_processes(marked, pids, groups)
    with (proof/'leftover-checks.jsonl').open('a') as out:
        out.write(json.dumps({'groups':len(remaining_groups),'markers':len(remaining_markers),
            'time':time.monotonic()})+'\n')
    if not remaining_groups and not remaining_markers: break
    assert time.monotonic()<end, (remaining_groups,remaining_markers)
    time.sleep(.05)
started = time.monotonic()
while True:
    other_markers = marked_processes(parents, retry_exiting=True)
    if not other_markers or time.monotonic()-started >= 30: break
    time.sleep(.05)
(proof/'hook-drain.json').write_text(json.dumps({'remaining':len(other_markers),
    'elapsed_seconds':time.monotonic()-started})+'\n')
assert not other_markers, f'{len(other_markers)} marked processes remain after the thirty-second drain: {other_markers}'
(proof/'marker-check.txt').write_text(f'PASS no saved owned pids or provider groups for app pids {sorted(parents)}\n')
print('PASS saved owned pids, provider groups and marker check; other marker drain seconds='+str(round(time.monotonic()-started,3)))
PY
test "$tables" != unreadable
test "$written" = yes
test "$rules_clean" = yes
test "$DATA" = "${VEYRA_VERIFY_TMP:-/tmp/veyra-verify-}$RUN"
rm -rf "$DATA"
test ! -e "$DATA"
/bin/ls -la "$PROOF" >"$PROOF/cleanup-listing.txt"
printf 'PASS cleanup PID=%s gone DATA=%s removed PROOF=%s kept\n' "$PID" "$DATA" "$PROOF"
```

Cleanup leaves `target/verify-veyra/$RUN` intact.
An app interrupted before its database exists is still stopped, and DATA is removed.
An unreadable database still lets Cleanup stop the app and its group; provider groups it cannot read are left to the marker check.
Never kill by name.

### Gotchas

Five live session-end hook processes still carried the parent marker after 10.08 seconds of Cleanup and were gone when checked a few seconds later.
Cleanup signals only saved run-owned pids and groups, while other marked processes get a separate thirty-second drain wait and fail Cleanup if they remain afterward.
The verification recipe chooses thirty seconds to allow that observed tail while bounding retries, including EIO from an exiting process; the wait ends immediately when no marker remains, records counts and elapsed time, and never prints arguments or environments.
It does not signal an unrecorded group.

## Helpers

- `bin/run` is the gated runner described in [Run](#run).
- `bin/orchestration.py` owns the masked child-process helper used by run, frame_guard and mutants, plus code identity and admission.
- `bin/self_tests.py` drives admission, probe, set and batch entry points with OS fixtures during `--check-only`.
- `bin/run --check-guards [--evidence DIR]` plants each registered guard fault in a temporary skill copy, requires a check-only failure naming that guard, restores exact bytes and requires check-only to pass.
  It compares names with `bin/guard-plants.json`; every stop case needs a plant and the manifest names the positive cases.
  Four workers use an independent copy per plant to run the full check-only entry, restore exact bytes and run it again; all use one immutable input snapshot.
  It saves each diff, red and green transcripts and their hashes in `matrix.json`; its default receipt stays under `/tmp/veyra-mverify-matrix-*/receipt/`.
  Markdown plants name the feature section and runtime function, so the page-owned Copy safety paths are exercised through their actual recipe entries with stubbed helpers.
- `bin/negative_tests.py` restores the prior negative contracts through production entries; `bin/raw_test_io.py` supplies process, IPC, filesystem and WindowServer/libproc I/O fixtures.
- `bin/glass_measure.py` holds the Settings pair entry and its unchanged placement/capture guards.
- `bin/heredocs.py` supplies the shared recipe parser; `bin/backdrop_actions.py` owns notification backdrop activation.
- `bin/frame_probe.py` owns probe admission and scene selection; `bin/frame_scene.py` owns shared dump sampling and `bin/frame_native.py` implements native window/process operations.
- `bin/pasteboard.swift` keeps every clipboard snapshot in memory and restores it on pipe EOF.
  Its comparison mode is `pasteboard --audit --expect-text FILE --ready-file READY`.
  The `--audit` snapshot/restore path is mandatory; a standalone `--expect-text FILE` is refused before system-board access.
  Start it before the Copy action, wait for the private READY file, drive Copy, then close stdin and wait for restoration.
  A completed comparison prints only `match` or `no-match`, with exit 0 or 1.
  A restore or verification failure prints no stdout, writes the fixed `PASTEBOARD RESTORE FAILED` diagnostic to stderr and exits 2.
  Unreadable expected files and leftover READY files exit 2 with fixed, distinct setup diagnostics, before snapshot access.
  Other comparison errors also exit 2 with a fixed diagnostic. A mismatch never hides a failed restore.
  Copy recipes preserve exit 2 in a `safety-stop.json` record and raise SafetyStop; the runner stops the batch after Cleanup.
  Recipes wait for readiness or early helper exit, skip Copy after an early exit, and check failures in finally so a poll error cannot hide restoration failure.
  A leftover READY file is refused before helper launch, so an old marker cannot authorize Copy.
  Clipboard text, length and hashes are never printed, logged or saved.
  FILE contains an independently known fixture expectation, never data read back from the clipboard.
  `--self-test` uses a stubbed board and covers match, mismatch, absent text, restoration after error,
  throwing or silently failed restoration, unreadable expectations, leftover READY files and refusal outside the canonical audit path.
  Setup cases inject the stubbed board and assert no capture occurs; self-tests never access NSPasteboard.
  Compile the system helper offline; every system-board use waits for the armed away window.
- `bin/driver.py` is the module every section imports with `PYTHONPATH=.agents/skills/verify-veyra/bin`.
- `bin/capture-window.swift PID WINDOW` and `bin/winid.swift PID [--bounds]` are compiled by Launch into the proof folder; `winid` finds the on-screen window by owner pid and normal window layer, and `--bounds` prints `x,y,w,h`.
- `bin/backdrop.swift` draws its own black and white stripes and sharp text across every display, using one borderless window per screen in one process.
  `--coarse` uses approximately 400 pt blocks and 160 pt text to reveal soft gradients and blurred letters through a translucent window; `--invert` swaps black and white to distinguish backdrop-dependent blur from a flat fill, and combines with `--coarse`.
  It prints each window number and screen frame, then one ready line, and SIGTERM to its own pid closes every window.
- `bin/select-pages`, `bin/fresh-pages` and `bin/pages.py` are described in [Selection](#selection).

A socket path permits 103 usable bytes; keep RUN and the scratch prefix short.

## Maintain the map

Smoke never stamps a Status line.
Only slice-end/gate full-run proposals, completed native reads and `bin/stamp-status` may set fake-page statuses.

Update the matching feature page when actions, commands or dump fields change.
`verify_map_names` checks exact action names, ctl verbs and top-level dump keys.
Each page's line under its title is `Status: STATE, DATE, COMMIT, MISSING, FINGERPRINT`, and a `Covers:` line of path globs follows it.
Set them from the gated run that proved the page, in a docs-only change after the candidate commit, with FINGERPRINT from `bin/fresh-pages --print COMMIT`.
Name in `Covers:` each source path whose change could change the page's result, and leave a file every page depends on uncovered, so it selects every page.
A new page joins the full run as soon as its file exists; link it in [the feature map](features/README.md) to place it in map order.
`bin/run` names app-launch's `Driving it with veyra ctl` and `Release check` and multi-agent's `Probe smoke measurements`, so renaming one fails `--check-only` until `bin/run` follows.
