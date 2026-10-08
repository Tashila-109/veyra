# Veyra verification map

[app-launch](app-launch.md) covers the native window, debug CLI, instance lock, migrated database, log, release exclusion, and cleanup.
It is the only feature page through M1.

## Preconditions

- Run Launch and Doctor in [verify-veyra](../SKILL.md).
- Launch prints a fresh short `RUN` and uses `/tmp/veyra-verify-$RUN`.
- Each later section recovers run, process, and window records in a fresh bash process.
Every later section needs that `RUN` exported, as [SKILL.md](../SKILL.md) says.
- Drive only the pid this run started.
- Keep proof in `target/verify-veyra/$RUN/`.

## Proof rules

Capture the action command, its exit code, the dump before and after, and the window PNG.
Read the PNG.
Preserve the app log and proof after Cleanup.
Report a skipped native entry point as skipped.
The driver does not prove external keyboard delivery, IME, or pointer input.
M0's `view`, `sidebar.rows`, and `composer_text` dump fields are fixed placeholders, not evidence of rendered state.
Use the live identity, appearance, sidebar width, and notice fields for their checks, and inspect the PNG for layout.
Each key check must reset to a state that its key changes.
The sidebar recipe checks the theme's 240 px default, 200 to 448 px bounds, and 16 px key step.
Inspect the 44 px header and the composer capped at 784 px in the PNG.

## Entry contract

Each feature starts with `Status: STATE, DATE, COMMIT, MISSING` directly under its title.
STATE is `passed`, `failed`, `blocked`, or `not run`.
DATE is the last run's date.
COMMIT is the short SHA of the phase commit that holds the candidate the lanes ran.
MISSING lists every sub-feature that run did not prove, or `none` when all passed.
A partial run never counts as passed.
The lead fills the values from lane results.
The lead fills COMMIT in a docs-only commit after the phase commit, since a commit cannot contain its own SHA.

Each feature has five H2 sections.
They are `Sub-features`, `How to get to it (user POV)`, `Driving it with veyra ctl`, `Observable success`, and `Gotchas`.
Observable success names the `dump` field, `sqlite3` read, or preserved artifact that proves each sub-feature.
For example, check `dump.pid` against `DATA/veyra.lock` and use `pragma user_version` for the migration.
Add a feature when a later milestone adds user-facing behavior.
`verify_map_names` checks every action, `ctl` verb and top-level `dump` key against this map and SKILL.md.
