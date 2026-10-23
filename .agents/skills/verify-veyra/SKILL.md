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

`bin/run --evidence DIR` runs a gated fake-CLI run: every page's sections in map order, then Doctor and Evidence, and Cleanup last whatever failed.
The pages are the [feature map](features/README.md)'s links in order, then any other `features/*.md` by name, so a new page joins the full run when its file exists.
It waits in Launch's gate, gives each section a fresh `bash -c` with only `RUN` carried forward, times every section into `DIR/ATTEMPT/sections.json` and hard-links the kept proof to `DIR/ATTEMPT/run` after Cleanup.
The runner chooses each attempt's `RUN` before Launch and passes it in, so the attempt's Cleanup knows its `RUN` even when Launch is interrupted.
Everything from Launch on runs inside one `try`, and its `finally` runs Cleanup before anything else.
SIGTERM, SIGHUP and SIGINT always raise, and the handler blocks all three before it raises.
Before the first attempt, nothing is owned yet, so a signal ends the runner at once with exit 128 plus the signal number.
During an attempt, a signal raises into that `try`.
Each section's process starts with the three signals blocked across its fork, and they are unblocked as the first step inside the `try` that stops that section's group; the process unblocks them for itself.
The `finally` runs with the signals blocked, so one that arrives during Cleanup, the proof copy or the verdict stays pending.
After a failed section, still inside that `try`, it takes one `failure` capture, unless human input caused the failure or the deadline stopped it, and records the capture's exit, error or timeout in `DIR/ATTEMPT/failure-capture.json`.
Each write during the attempt records its own error in the attempt's `problems` instead of raising, so no write can stop Cleanup.
After Cleanup, one verdict decides the attempt, and it is the only place a run can become DONE:
- The verdict reads pending signals with `sigpending()` for every outcome, and an interruption ends the attempt FAILED (interrupted) and is recorded in the attempt's record.
- DONE needs every planned section passed, Cleanup exit 0, an unchanged tree, the proof copy kept in `DIR/ATTEMPT/run` and no interrupting signal.
- An interruption, a failed Cleanup, scratch left behind, a proof copy that was not kept or a changed tree ends FAILED, and the recorded errors stay in `problems`.
- The failure capture, the transcript, `sections.json` and `plan.json` are diagnostics: a write error there is listed in `problems` and changes nothing else.
- Otherwise a failed section is classified by the observation `observe_failure` recorded in `focus-failure.json` during it, and a corrupt `focus-failure.json` counts as no observation:
  - Reaching the deadline, or any other failure in its last minute, ends BLOCKED.
  - With human input since launch, any observed failure gets a new attempt after 60 seconds, not counted.
  - A `capture` failure with no human input gets a new attempt after 60 seconds, and the third such failure in a run ends BLOCKED.
  - A `focus-timeout` failure with no human input, such as a dock-focus regression, and any failure with no observation end FAILED.

After the verdict, `DIR/ATTEMPT/attempt.json` is written once, with the verdict, and read back.
Between attempts, the 60-second wait is unblocked, and a signal there ends the run FAILED.
After the last attempt, `DIR/outcome.json` is written once, with the status, every attempt and the gate wait apart from the recipe time, and read back.
A receipt that fails to write or read back ends the run FAILED with exit 1, and its error goes to stderr, because the receipt that would hold it was not kept.
The runner checks for pending signals before it prints the final line, which says FAILED (interrupted) when a signal is pending and every receipt was kept, and once more after it.
A signal still pending at the last check, which arrived after the verdict while the receipts or the final line were written, cannot be recorded in the receipts, so it ends the run with exit 128 plus its number and the reason on stderr.
The run exits 0 only when the verdict was DONE, both receipts were written and read back, and no signal was pending at that last check.

```text
.agents/skills/verify-veyra/bin/run --evidence DIR                     # every page, the full run
.agents/skills/verify-veyra/bin/run --evidence DIR --base BASE         # the pages a change from BASE selects
.agents/skills/verify-veyra/bin/run --evidence DIR --pages multi-agent # one page alone
.agents/skills/verify-veyra/bin/run --evidence DIR --sections 'SKILL/Doctor' 'multi-agent/Probe smoke measurements'
.agents/skills/verify-veyra/bin/run --check-only                       # every block, the map and the gate; no launch
```

`--base BASE` runs [Selection](#selection) from BASE to the working tree itself.
A selection error ends the run with exit 2 before Launch.
A selection with no fake-CLI token exits 0 without launching and says so, and otherwise the run drives the selected tokens.
`--pages` takes the tokens `select-pages` prints: `PAGE` for a page's own sections, `PAGE:codex` and `PAGE:grok` for its provider sections, and `release` for app-launch's release check.
Launch, Doctor, app-launch's first section, Evidence and Cleanup always run; a selected `permission-matrix` is reported as due and never driven, because it needs a consented live lane.
`--sections PAGE/HEADING ...` runs exactly those sections between Launch and Cleanup, as a perf or live plan needs.
`--real-quiet` makes multi-agent wait for the real ten-minute quiet clock, which a phase gate runs once when `ui/header.rs`, `ui/sidebar.rs` or a `ui/mod.rs` line naming `last_activity`, `quiet_minutes` or `cadence_due` changed.
`--expect-head SHA` refuses a checkout at another commit, `--deadline EPOCH` bounds the whole run, and `--scratch PREFIX` sets the scratch prefix, `/tmp/veyra-verify-` by default.
The tree's identity, its HEAD, `git status --short`, the diff's SHA-256 and each untracked file's SHA-256, is saved at the start and checked after Launch's build and after Cleanup.
`--check-only` compiles every block, fails when app-launch's first section, its `Release check` or a perf-tier section is missing, and checks that Launch calls `gate()` before its first `start()`.
It reads `start()`'s relaunch refusal (`foreign = foreign_veyra()` then `assert not foreign`, before the launch) and Cleanup's `assert not other_markers` and `retry_exiting=True` calls as Python structure, and checks only that the 30 s drain, stale-screenshot and covered-window guard texts are present.
It tables the verdict, and drives `gate()` on a virtual clock with stubbed idle, lock and `pgrep` readers, failing if that self-test runs past 30 seconds.

## Selection

`bin/select-pages BASE [HEAD]` prints the tokens a change from BASE selects, one per line; without HEAD it reads the working tree, untracked files included.
`bin/pages.py` reads its lists from this block, so the block is their one owner:

```text
shared    Cargo.toml Cargo.lock crates/veyra/src/ui/mod.rs crates/engine/src/actor.rs
          .agents/skills/verify-veyra/SKILL.md .agents/skills/verify-veyra/bin/**
no-page   crates/*/tests/*.rs crates/*/tests/common/** crates/engine/src/tests/** crates/veyra/src/tests.rs
          crates/veyra/src/verify_map.rs crates/engine/src/platform/boundary.rs crates/engine/src/provider/scripted.rs
          ARCHITECTURE.md AGENTS.md clippy.toml scripts/** .agents/skills/verify-veyra/features/README.md
codex     crates/engine/src/provider/codex.rs crates/engine/src/provider/codex/** crates/engine/tests/fixtures/codex/**
grok      crates/engine/src/provider/grok.rs crates/engine/tests/fixtures/grok/**
provider  crates/engine/src/provider/mod.rs crates/engine/src/provider/session.rs crates/engine/src/provider/env.rs
          crates/engine/src/provider/lines.rs crates/engine/src/provider/jsonrpc.rs crates/engine/src/mcp.rs
          crates/engine/src/tools.rs crates/veyra/src/relay.rs
release   crates/veyra/src/control.rs crates/veyra/src/main.rs crates/veyra/build.rs
```

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
`python3 .agents/skills/verify-veyra/bin/pages.py self-test` checks every selection rule, the fingerprint and the version match in a scratch repository.

## Launch

The default verification lane uses only the fake CLI.
Its binary path deliberately starts missing, so [new-thread](features/new-thread.md) can prove installation and Check again.
Every section imports [`bin/driver.py`](bin/driver.py), which shares logging, bounded polling, fixture staging and the one expected database-table list.
The driver does not set application state; its writes are fixture copies, screenshots and proof.
For an authorized live lane, export `VEYRA_VERIFY_LANE=live` before Launch.
Launch then removes replay overrides, records to `$PROOF/rec`, and the new-thread and chat pages use their live branches.
A live lane needs separate human authorization because it runs the installed CLI and spends its quota.
The first launch waits in the driver's `gate()` until the screen is unlocked, the HID idle time is between 120 and 9,000 seconds and no `veyra` process has run for 10 seconds; it records the idle time in `input-idle-launch.json`.
Every later relaunch through `start()` refuses a running `veyra` this run did not start: one that is neither an owned app pid nor marked with an owned app's `VEYRA_PARENT_PID`.
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
The fake lane uses an owned login-shell script with a 1.5 s delay, so the first ready line and dump prove the window opens while `login_path` is `pending`.
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
cargo build --workspace
mkdir -p "$PROOF/fixtures" "$DATA/project" "$DATA/appcwd"
printf '%s\n' "${VEYRA_VERIFY_LANE:-fake}" >"$PROOF/lane"
: >"$PROOF/pages-run"
swiftc -O "$BIN/capture-window.swift" -o "$PROOF/capture-window"
swiftc -O "$BIN/winid.swift" -o "$PROOF/winid"
swiftc -O "$BIN/backdrop.swift" -o "$PROOF/backdrop"
swiftc -O "$BIN/pasteboard.swift" -o "$PROOF/pasteboard"
cp crates/engine/tests/fixtures/claude/probe-initialize.jsonl "$PROOF/fixtures/probe.jsonl"
cp crates/engine/tests/fixtures/codex/probe-ready.jsonl "$PROOF/fixtures/probe-codex.jsonl"
cp crates/engine/tests/fixtures/grok/probe-grok.jsonl "$PROOF/fixtures/probe-grok.jsonl"
printf 'RUN=%s\n' "$RUN"
RUN="$RUN" PYTHONPATH="$BIN" python3 - <<'PY'
from driver import *
gate()
if LANE == 'fake': make_git_shim()
if LANE == 'fake':
    shell = DATA/'login-shell'
    shell.write_text('#!/bin/sh\n/bin/sleep 1.5\nexec /bin/sh "$@"\n')
    shell.chmod(0o700)
    ENV['SHELL'] = str(shell)
pid = start('launch')
pending = ctl('launch-login-path','dump')
assert pending['login_path'] in ['pending','loaded','fallback']
if LANE == 'fake': assert pending['login_path'] == 'pending', pending['login_path']
assert f'veyra ready pid={pid} data_dir={DATA}' in (PROOF/'launch.stderr').read_text()
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

Doctor fails on a wrong process, revision, folder, window or appearance.
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
pid = identity()
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
Its `### Evidence contract` JSON block owns that page's `required` files and `pairs` comparisons.
Evidence reads those blocks through `bin/pages.py` and applies every completed token's entries.
Before every capture, the debug-only `ActivateWindow` action activates Veyra from inside the app.
The helper waits for two separate rendered frames and 100 ms before inspecting and capturing the window.
Solid captures use `screencapture -x -o -l WINDOW` and keep the existing on-screen and frontmost guards.
Translucent captures use `screencapture -x -R x,y,w,h` of the exact on-screen window rectangle.
They require `bin/backdrop.swift` behind Veyra, with its run-owned pid and window ids in `backdrop-windows.json`.
Immediately before and after each region capture, the helper checks that Veyra is frontmost, its bounds equal the rectangle, the rectangle is wholly covered by one backdrop window on one display, and no other on-screen window above Veyra intersects it.
A changed guard discards the capture; the helper makes at most three attempts in total.
The Settings page places a second run-owned backdrop above Veyra and requires the guard to reject it without retaining a PNG.
Veyra must stay frontmost during captures.
A changed visible state with an identical PNG fails; Evidence also compares named state pairs.
Concurrent windows may cause a capture to fail, but cannot make a covered or off-screen frame pass.
A dump proves state, not layout; inspect each PNG.

## Evidence

Run this after the chosen pages and the final Doctor.
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
identity()
ctl('dump-after','dump')
pages = set((PROOF/'pages-run').read_text().split())
assert pages, 'no completed page'
database('final','ask_first' if pages & {'settings','approvals-questions'} else 'full_access')
capture('window')
shutil.copy2(DATA/'logs/veyra.log',PROOF/'veyra.log')
(PROOF/'git-identity.txt').write_text(subprocess.check_output(['git','rev-parse','HEAD'],text=True)
    +subprocess.check_output(['git','status','--short'],text=True))
import pages as page_map
required,pairs = page_map.evidence_contract(pathlib.Path.cwd())
assert pages <= required.keys(), pages
for name in ['drive.log','sql.log',*(name for page in pages for name in required[page])]:
    assert (PROOF/name).stat().st_size > 0, name
if 'app-launch' in pages:
    assert len({(PROOF/(label+'-inode.txt')).read_text() for label in ['initial','after-migration']}) == 1
for pid in (PROOF/'owned-app-pids').read_text().split():
    assert f'veyra ready pid={pid} data_dir={DATA}' in (PROOF/'veyra.log').read_text()
for page in pages:
    for first,second in pairs.get(page,[]):
        assert (PROOF/(first+'.png')).read_bytes() != (PROOF/(second+'.png')).read_bytes(), (first,second)
if 'chat-send-stream-live' in pages:
    assert list((PROOF/'rec').glob('t*-s*.jsonl')), 'missing live session recording'
print('PASS evidence pages='+','.join(sorted(pages))+'; identity, database, relaunches, screenshots and release kept')
PY
```

Inspect the PNGs themselves for Geist, the 240 px initial sidebar, 44 px header, upper-third new-thread block, centred chat column and composer, collapsed tools/reasoning, failure cause and Settings copy.
The transcript caps at 752 px, the composer at 784 px, with 40 px gutters.
Yabai can override the requested 1280 by 800 outer bounds; use Doctor's actual bounds.
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
- `bin/driver.py` is the module every section imports with `PYTHONPATH=.agents/skills/verify-veyra/bin`.
- `bin/capture-window.swift PID WINDOW` and `bin/winid.swift PID [--bounds]` are compiled by Launch into the proof folder; `winid` finds the on-screen window by owner pid and normal window layer, and `--bounds` prints `x,y,w,h`.
- `bin/backdrop.swift` draws its own black and white stripes and sharp text across every display, using one borderless window per screen in one process.
  `--coarse` uses approximately 400 pt blocks and 160 pt text to reveal soft gradients and blurred letters through a translucent window; `--invert` swaps black and white to distinguish backdrop-dependent blur from a flat fill, and combines with `--coarse`.
  It prints each window number and screen frame, then one ready line, and SIGTERM to its own pid closes every window.
- `bin/select-pages`, `bin/fresh-pages` and `bin/pages.py` are described in [Selection](#selection).

A socket path permits 103 usable bytes; keep RUN and the scratch prefix short.

## Maintain the map

Update the matching feature page when actions, commands or dump fields change.
`verify_map_names` checks exact action names, ctl verbs and top-level dump keys.
Each page's line under its title is `Status: STATE, DATE, COMMIT, MISSING, FINGERPRINT`, and a `Covers:` line of path globs follows it.
Set them from the gated run that proved the page, in a docs-only change after the candidate commit, with FINGERPRINT from `bin/fresh-pages --print COMMIT`.
Name in `Covers:` each source path whose change could change the page's result, and leave a file every page depends on uncovered, so it selects every page.
A new page joins the full run as soon as its file exists; link it in [the feature map](features/README.md) to place it in map order.
`bin/run` names app-launch's `Driving it with veyra ctl` and `Release check` and multi-agent's `Probe smoke measurements`, so renaming one fails `--check-only` until `bin/run` follows.
