# Veyra verification map

[app-launch](app-launch.md) covers the current native window, debug CLI, instance lock, log, release exclusion, and cleanup.
It is the only feature in M0.

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

Each feature has four H2 sections.
They are `Sub-features`, `How to get to it (user POV)`, `Driving it with veyra ctl`, and `Gotchas`.
Add a feature when a later milestone adds user-facing behavior.
