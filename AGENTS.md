# Work on Veyra

Veyra is a native macOS Rust app.
Read [ARCHITECTURE.md](ARCHITECTURE.md) before changing ownership, dependencies, data, startup, or the debug protocol.
Update that document in the same change as a structural change.
After a change merges into `main`, update `.tstack/docs/architecture.md` when the architecture changed, then run `node .tstack/docs/sync.mjs` from the primary checkout.
The docs under `.tstack/docs/` stay uncommitted.

## Commands

Run commands in this checkout.
Homebrew rustup is keg-only on this Mac.

```sh
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
cargo build --workspace
cargo run -p veyra
cargo test --workspace
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

For a UI or CLI change, follow [.agents/skills/verify-veyra/SKILL.md](.agents/skills/verify-veyra/SKILL.md).
Prove the real window and CLI path, not only compilation or unit tests.
Use a disposable `VEYRA_DATA_DIR`.
Never drive an instance this run did not start.

## Standing rules

- Never commit `.tstack/` or any planning, design, or exploration artifact.
The planning documents live in `.tstack/tasks/` of the primary checkout and are never committed.
- Cross-review every design and plan on the `interrogate reviewers` role before the human's gate.
Evidence does not replace that approval.
- Learn from zeron, monocode, and T3 Code, but write Veyra's own code, values, fixtures, and assets.
Never copy them from third parties.
The one exception is the violet accent the human kept, `accent` `#8b7cf6` in dark and `#5b43e8` in light, with `accent_selection` `#8b7cf652` and `#5b43e838`.
Keep no third-party notices file.
Take a third-party asset from its author's official release and keep its licence beside it.
- Keep the engine crate free of gpui dependencies.
- Pin direct dependencies exactly and keep `Cargo.lock`.
- Name the data shape before writing stateful logic.
- Keep controls on gpui actions and bind their keys in `ui/actions.rs`.
The debug driver reaches those actions through the window.
- Keep every driver reference behind `cfg(debug_assertions)`.
Release builds must contain no driver socket name.
- Keep proof outside scratch state.
Cleanup stops only the pids and groups the run started.
- The lead owns Git writes during a tstack task.
Workers do not commit, push, create branches, merge, rebase, stash, reset, or checkout.
