# Provider permission matrix

Status: not run, 2026-10-12, 950317c, Codex and Grok live cells

## Sub-features

One live prompt checks reading, a command, a project write and an outside write in every permission mode.
Claude's row comes from preserved authorized live evidence; Codex and Grok remain not run.

| Provider | Ask first | Accept edits | Full access |
| --- | --- | --- | --- |
| Claude | passed: Read and `echo` ran without approval; both writes docked as `Write` with their absolute targets, inside and outside the project, Allow once, both files written | passed: Read, `echo` and the project write ran without approval; the outside write docked as `Write`, outside the project, Allow once, written | passed: all four steps ran with no approval; both files written |
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

Workers do not run installed providers; the Claude cells above retain the authorized live run's results.
CLI upgrades require another live permission-grid run with human consent.
Full access can change files outside the project.
Fill Claude's row only from an authorized live permission run.

Live run RUN `d654ddf0`, Claude 2.1.296, model `sonnet`, 2026-10-11, checkout `8ea19cf`.
One prompt per mode was sent through Send after PickPermission; every dock was captured and answered Allow once with dock focus.
Proof paths below name the preserved lane evidence.

### Preserved live evidence

#### Ask first

- Thread `01a12b32-71bd-7487-a0d3-15968f920528`, `selected_thread.config.mode` / SQLite mode `ask_first`, turn `"Completed"`.
- Tool calls in order: `Read`, `Bash`, `Write`, `Write`; `echo` printed `veyra-permission-probe`; Read returned `veyra permission probe`.
- Docks (2): `Write` `{"file_path":"/private/tmp/veyra-verify-d654ddf0/project/permission-project.txt","content":"veyra permission probe\n"}`; `Write` `{"file_path":"/tmp/veyra-outside-d654ddf0.txt","content":"veyra permission probe\n"}`.
- Requests in SQLite: approval answered "AllowOnce"; approval answered "AllowOnce".
- Effects: `permission-project.txt` = "veyra permission probe\n"; `/tmp/veyra-outside-d654ddf0.txt` = "veyra permission probe\n" (removed after the check).
- Files: `lane-3/checks/l3-matrix-ask_first.txt`, `lane-3/run/l3-matrix-ask_first-dock-*.{json,png}`, `lane-3/run/l3-matrix-ask_first-done.{json,png}`, `lane-3/samples/l3-matrix-ask_first-*.jsonl`, recording `lane-3/rec/`.

#### Accept edits

- Thread `01a12b32-a337-7784-b7cc-cf3b74283187`, `selected_thread.config.mode` / SQLite mode `accept_edits`, turn `"Completed"`.
- Tool calls in order: `Read`, `Bash`, `Write`, `Write`; `echo` printed `veyra-permission-probe`; Read returned `veyra permission probe`.
- Docks (1): `Write` `{"file_path":"/tmp/veyra-outside-d654ddf0.txt","content":"veyra permission probe\n"}`.
- Requests in SQLite: approval answered "AllowOnce".
- Effects: `permission-project.txt` = "veyra permission probe\n"; `/tmp/veyra-outside-d654ddf0.txt` = "veyra permission probe\n" (removed after the check).
- Files: `lane-3/checks/l3-matrix-accept_edits.txt`, `lane-3/run/l3-matrix-accept_edits-dock-*.{json,png}`, `lane-3/run/l3-matrix-accept_edits-done.{json,png}`, `lane-3/samples/l3-matrix-accept_edits-*.jsonl`, recording `lane-3/rec/`.

#### Full access

- Thread `01a12b32-ce7f-7070-a171-7da550d1d2e8`, `selected_thread.config.mode` / SQLite mode `full_access`, turn `"Completed"`.
- Tool calls in order: `Read`, `Bash`, `Write`, `Write`; `echo` printed `veyra-permission-probe`; Read returned `veyra permission probe`.
- Docks (0): none.
- Requests in SQLite: none.
- Effects: `permission-project.txt` = "veyra permission probe\n"; `/tmp/veyra-outside-d654ddf0.txt` = "veyra permission probe\n" (removed after the check).
- Files: `lane-3/checks/l3-matrix-full_access.txt`, `lane-3/run/l3-matrix-full_access-done.{json,png}`, `lane-3/samples/l3-matrix-full_access-*.jsonl`, recording `lane-3/rec/`.

### Evidence notes

- Every observed behaviour matches the page's expectation for its mode, so no cell is marked failed.
- Accept edits records Claude's directory boundary: a write outside the project folder still asks.
- In every mode Claude's own closing text said no step needed approval, including Ask first, where two writes docked. That is the model's narration, not Veyra state.
- The `Write` docks show the tool input as raw JSON under the copy "Claude Code will run this command in the project folder.", also for the `/tmp` write (the recording predates the tool-target dock repair).
- Codex and Grok cells stay not run here.
