# Claude Code instructions for Bidsmith

## Read first

- [DECISIONS.md](DECISIONS.md) — locked choices and current state. Treat
  as authoritative; do not relitigate (language, syntax, state model,
  `--auto-approve` gating, etc.).
- [ROADMAP.md](ROADMAP.md) — open decisions and "Next session: start
  here" priority list. The phases and priorities are speculative —
  feel free to revise if a real requirement contradicts them.
- [README.md](README.md) — high-level pitch (skim only).

Default starting point: take the top unblocked item from "Next session:
start here" in ROADMAP.md unless the user redirects.

## Verifying changes

- `cargo build` should stay warning-free.
- `cargo run -- validate examples/basic` → `OK: 1 file(s) valid.`
- `cargo run -- validate examples/broken` → exits non-zero with
  source-mapped miette diagnostics.
- `cargo run -- export --from-json examples/exports/basic.json
  --output /tmp/out.bid && cargo run -- validate /tmp/out.bid` →
  round-trips cleanly.

## Project-specific rules

- **Lockstep rule**: any resource type added to `src/schema.rs` gets a
  matching renderer in `src/commands/export.rs` in the same change.
  Anything `validate` knows about, `export` should be able to produce.
- **No Google Ads API code yet** — that's Phase 2+. Don't pull in
  networking, auth, or gRPC dependencies without an explicit ask.
- **User-facing errors with source locations**: use `Diag` from
  `src/diagnostics.rs`. miette handles the rendering.
- **`.bid` extension is provisional** but stable for now — don't rename
  it as part of unrelated work.

## Things to update alongside code

- Add resource → update DECISIONS.md "Validator covers" list.
- Add CLI verb or change a flag → update DECISIONS.md verbs table and
  README.md "Commands" section.
- Settle an open decision → move it from ROADMAP.md "Open decisions"
  to DECISIONS.md "Locked decisions" with a one-line rationale.
