# Provider permission matrix

Status: not run, 2026-10-11, pending, all live cells

## Sub-features

One live prompt checks reading, a command, a project write and an outside write in every permission mode.
Every cell below starts not run; only preserved live evidence can change it.

| Provider | Ask first | Accept edits | Full access |
| --- | --- | --- | --- |
| Claude | not run | not run | not run |
| Codex | not run | not run | not run |
| Grok | not run | not run | not run |

## How to get to it (user POV)

In a disposable project, select the provider and permission mode, then send the prompt below.
Ask first asks before edits and commands the CLI does not treat as safe; commands can use the network.
Accept edits permits edits and asks before commands the CLI does not treat as safe; commands can use the network.
Full access permits commands and writes without asking; a request the CLI still sends always docks.
Reading and the safe echo command should run without an approval in all three modes.
Ask first should dock both file writes; Accept edits should allow the project write, while the outside write records the provider's actual directory boundary.
Full access should complete all four steps without an approval.
The live provider's actual behavior is the result, including any approval beyond that expectation.

## Driving it with veyra ctl

This page requires separate human authorization to run installed providers and spend quota.
Do not run it from the fake-only worker lane.
Use a fresh Launch with VEYRA_VERIFY_LANE=live, the exported RUN, Doctor and new-thread.
Create project file `permission-input.txt` with the text `veyra permission probe` before sending.
Replace RUN in this one prompt with the exported run identity:

> Read permission-input.txt and report its content. Run echo veyra-permission-probe. Write permission-project.txt in this project containing veyra permission probe. Write /tmp/veyra-outside-RUN.txt containing veyra permission probe. Describe each step and wait for required approvals.

Select each mode with PickPermission, send through Send, and preserve every dock and decision with dump and capture.
Use AllowOnce or Deny as the operator directs; do not silently grant the whole thread.
Read the project and outside files to prove the effects, then remove only this run's outside file.
Record cell status, provider version, requested approvals, decisions, files and the run proof path.
Codex and Grok remain not run until their adapters exist.

## Observable success

Each cell links a live run with selected_thread.config.mode, the exact dock commands, transcript and the four observed effects.
An unexpected permission behavior marks the cell failed and retains the approval request.
A missing provider or authorization marks it blocked with the missing prerequisite.
Replay evidence never marks a live cell passed.

## Gotchas

The worker is forbidden to run claude, codex or grok; all nine cells remain not run here.
CLI upgrades require another live permission-grid run with human consent.
Full access can change files outside the project.
Fill Claude's row only from an authorized live permission run.
