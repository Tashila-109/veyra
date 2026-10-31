# Veyra verification map

[app-launch](app-launch.md) covers the native window, debug CLI, instance lock, migrated database, log, release exclusion, and cleanup.
[new-thread](new-thread.md) covers project selection, startup readiness, Check again, the no-account probe and the live composer choices without creating a thread, including Codex's CLI models and efforts and Grok's Default-only menus.
[chat-send-stream](chat-send-stream.md) covers fake-CLI streaming, Working → Done, failure cause and stderr, Retry and ctl wait, plus Codex's foreign-thread filtering and retry/limit notices and Grok's session authentication failure naming `grok login`.
[command-palette](command-palette.md) covers action/context filtering, request-answer refusal, title search and focus return.
[toasts](toasts.md) covers persistent notices, per-family attention, Copy and the storage-only card.
[settings](settings.md) covers OpenSettings, all defaults, existing-versus-new thread modes and default persistence after relaunch.

[approvals-questions](approvals-questions.md) covers approval decisions, exact grants, question keys and NextNeedsYou, plus Codex directory-aware command grants and MCP approvals without grants, and Grok Deny-only consent, question answers and expiry after Stop.
[stop-interrupt](stop-interrupt.md) covers Stop, held queues, awake and banners.
[restart-recovery](restart-recovery.md) covers the quit sheet, ConfirmQuit, ctl quit and recovery.
[multi-agent](multi-agent.md) covers child rows, task and family cards, archive choices, held wakes, report delivery and quiet time, plus Codex and Grok root relay calls and a Claude root's Codex child.
[shell-drawer](shell-drawer.md) covers per-thread login shells, focus, tabs, running-job consent, restart, hidden output, archive, quit, crash and one debug flood.
[permission-matrix](permission-matrix.md) records the live provider permission grid.

## Preconditions

- Run Launch and Doctor in [verify-veyra](../SKILL.md).
- Launch takes the exported `RUN`, eight lowercase hex characters that `bin/run` chooses for each attempt, prints it and uses `/tmp/veyra-verify-$RUN`, or `$VEYRA_VERIFY_TMP$RUN` when that prefix is set.
- Each later section recovers run, process, and window records in a fresh bash process.
Every later section needs that `RUN` exported, as [SKILL.md](../SKILL.md) says.
- Drive only the pid this run started.
- Keep proof in `target/verify-veyra/$RUN/`.

## Proof rules

Capture the action command, its exit code, the dump before and after, and the window PNG.
Veyra must stay frontmost during captures.
The debug-only ActivateWindow action activates the owned window before each capture.
Read the PNG.
Preserve the app log and proof after Cleanup.
Report a skipped native entry point as skipped.
The driver does not prove external keyboard delivery, IME, or pointer input.
`view`, `sidebar.rows`, `selected_thread`, `transcript`, `composer_text`, `composer`, `providers`, `settings`, `drawer`, `focus`, `composer_caret` and `perf` are live state.
Use their assertions alongside identity, appearance, sidebar width and notice, palette and toasts, and inspect the PNG for layout.
Each key check must reset to a state that its key changes.
The sidebar recipe checks the theme's 240 px default, 200 to 448 px bounds, and 16 px key step.
Inspect the 44 px header and the composer capped at 784 px in the PNG.

## Entry contract

Each feature starts with `Status: STATE, DATE, COMMIT, MISSING, FINGERPRINT` directly under its title, and a `Covers:` line of space-separated path globs follows it.
STATE is `passed`, `failed`, `blocked`, or `not run`.
DATE is the last run's date.
COMMIT is the short SHA of the candidate commit whose gated run proved the page.
MISSING lists every sub-feature that run did not prove, or `none` when all passed.
FINGERPRINT is the page's fingerprint at COMMIT from `bin/fresh-pages --print COMMIT`; see [Selection](../SKILL.md#selection).
A partial run never counts as passed.
Fill the values from preserved verification results.
Fill COMMIT and FINGERPRINT in a docs-only commit after the candidate commit, since a commit cannot contain its own SHA.
`Covers:` names the source paths a page's result depends on beyond its own file, so `bin/select-pages` selects it and its fingerprint changes when they change.
A file every page depends on stays out of every `Covers:` line, so it selects every page.
A live cell keeps the commit, CLI versions and RUN of the consented live run that proved it; no fake-CLI run sets it.

Each feature has five H2 sections.
They are `Sub-features`, `How to get to it (user POV)`, `Driving it with veyra ctl`, `Observable success`, and `Gotchas`.
Observable success names the `dump` field, `sqlite3` read, or preserved artifact that proves each sub-feature.
For example, check `dump.pid` against `DATA/veyra.lock` and use `pragma user_version` for the migration.
Add a feature when a later milestone adds user-facing behavior.
`verify_map_names` checks every action, `ctl` verb and top-level `dump` key against this map and SKILL.md.
