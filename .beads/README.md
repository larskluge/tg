# tg issue tracking

tg tracks its work in Beads (`bd`) since 2026-10-23. Run `bd prime` and `bd ready` at session start; claim with `bd update ID --claim` and close only after verification. Worktrees share the main checkout's database automatically; never initialize a second one.

## Storage and Git

The embedded Dolt database in `~/code/tg/.beads/embeddeddolt` is the live source of truth and is Git-ignored, as are Beads' lock and runtime files (`.beads/.gitignore`). No export of the issues is kept in Git: `export.auto` is off and there is no `issues.jsonl`. For a look at the records, `bd list --json`, or `bd export` to stdout. `.beads/interactions.jsonl` stays opted out (`audit.enabled: false`).

## Dolt remote

The Dolt remote `origin` is `git+ssh://outpost/home/l/beads-remotes/tg.git` (since 2026-10-23; `sync.remote` in `config.yaml` records it): a bare repository on the outpost host holding the database's committed history in `refs/dolt/data` and nothing of the code. A `git push` never carries it, and none of it is on GitHub. It is the one copy that u26 and Kira's container on outpost both write to (`~/code/outpost/AGENTS.md` → "Beads remotes on outpost"). `dolt.auto-push` is on with a 1ms interval, so each successful write pushes at once. An auto-push warning does not mean the write failed; read-only commands and `--sandbox` do not push. Before reporting durable completion, run `bd dolt push`. `bd` on u26 is `~/.local/bin/bd`, outpost's `bin/bd-pull-first`, which pulls before a write; a conflict bd's auto-resolver declines is settled by a person, never by a blanket `--strategy`.

Beads' git hooks are tracked in `.beads/hooks/`, and `core.hooksPath` points there, as in `~/code/mycelium`. For a fresh clone, install them with `bd hooks install --beads`.

Recovery into a fresh disposable clone: `bd init --non-interactive --skip-agents --skip-hooks -p tg --remote git+ssh://outpost/home/l/beads-remotes/tg.git` with `BD_DOLT_AUTO_PUSH=false` while validating; never reinitialize the live database.
