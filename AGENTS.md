# AGENTS

## Agent skills

### Issue tracker

Issues and specs live as markdown files under `.scratch/<feature>/` in this repo. See `docs/internal/agents/issue-tracker.md`. Historical tickets are archived in `docs/internal/tickets/`.

### Domain docs

Single-context: one `GLOSSARY.md` plus `docs/adr/` at the repo root. See `docs/internal/agents/domain.md`.

### User documentation

`README.md` and `docs/user/01`–`08` are the user-facing documents. They are
written in ASD-STE100 Simplified Technical English: short sentences, imperative
steps, one instruction per sentence, the glossary's vocabulary, and
`WARNING`/`CAUTION`/`NOTE` for consequences. A change to a command, an option, a
configuration key or a prerequisite updates the matching document in the same
change.
