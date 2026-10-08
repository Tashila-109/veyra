# Veyra verification map

[app-launch](app-launch.md) covers the current native window, debug CLI, instance lock, log, and cleanup.
It is the only feature in M0.

## Preconditions

- Run Launch and Doctor in [verify-veyra](../SKILL.md).
- Use a fresh short `RUN` and `/tmp/veyra-verify-$RUN`.
- Drive only the pid this run started.
- Keep proof in `target/verify-veyra/$RUN/`.

## Proof rules

Capture the action command, its exit code, the dump before and after, and the window PNG.
Read the PNG.
Preserve the app log and proof after Cleanup.
Report a skipped native entry point as skipped.
The driver does not prove external keyboard delivery, IME, or pointer input.

## Entry contract

Each feature has four H2 sections.
They are `Sub-features`, `How to get to it (user POV)`, `Driving it with veyra ctl`, and `Gotchas`.
Add a feature when a later milestone adds user-facing behavior.
