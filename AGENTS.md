# Outrim

## Development Rules

- If you need to create a worktree, save it to ./.worktrees.
- After each logically self-contained change (text/fix/feat/docs/refactor/...), stage and create a new commit. Follow /caveman-commit skill for commit messages.
- Never write complex and meaningless test.
- Keep code clean and intentional with /codebase-design skill; prefer clarity over compatibility. Give each module one reason to change, with a small public interface over a private implementation.
- Before completing a Rust task, run `cargo fmt --all` and `cargo clippy --all-targets --all-features -- -D warnings`. No warnings should be emitted.
- Before completing a TypeScript task, run the project's formatter, linter and type-check (`bun check --no-script`) commands. Fix all errors and warnings.
- Do NOT change/write README.md unless user ask you to do so.

## Agent skills

- Issue tracker: Issues and specs live as GitHub issues in `WodenJay/Outrim`; use /gh-cli skill. See `docs/agents/issue-tracker.md`.
- Triage labels: Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.
- Domain docs: Single-context: one `GLOSSARY.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
