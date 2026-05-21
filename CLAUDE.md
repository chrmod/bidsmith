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
- `cargo test --features e2e --no-run` should compile clean — keeps
  the opt-in e2e tier from rotting.
- `cargo run --quiet -- schema` should print well-formed JSON. After
  any schema change, regenerate the committed snapshot via
  `cargo run --quiet -- schema --output website/src/data/schema.json`.

### Optional: live round-trip e2e

The `e2e` Cargo feature gates `tests/e2e.rs`, a live round-trip test
(`apply → pull → export → fmt --check → plan`) against a dedicated
Google Ads test manager account. Don't run this on a production
account — every step issues real mutate ops.

```sh
export BIDSMITH_E2E_CUSTOMER_ID=1234567890   # test account, NOT prod
# (plus the usual GOOGLE_ADS_DEVELOPER_TOKEN / _CLIENT_ID / _CLIENT_SECRET /
#  _REFRESH_TOKEN — same envelope plan/apply use)
cargo test --features e2e -- --nocapture
```

The test forces `GOOGLE_ADS_CUSTOMER_ID` to `BIDSMITH_E2E_CUSTOMER_ID`
for every subprocess, so a developer's shell env can't redirect it.
Resources are named `bidsmith-e2e-<unix-ts>-…`; teardown (run from a
`Drop` guard, so it fires even on panic) sweeps that prefix via the
hidden `_e2e-cleanup` subcommand.

## Project-specific rules

- **Lockstep rule (export)**: any resource type added to
  `src/schema.rs` gets a matching renderer in
  `src/commands/export.rs` in the same change. Anything `validate`
  knows about, `export` should be able to produce.
- **Lockstep rule (docs)**: any schema change in `src/schema.rs`
  (new resource, new attribute, new enum value, new nested block)
  ships with a regenerated `website/src/data/schema.json` in the
  same commit. Run
  `cargo run --quiet -- schema --output website/src/data/schema.json`.
  The auto-generated reference pages under
  `website/src/content/docs/resources/` consume this file directly,
  so the docs cannot drift from the validator.
- **No Google Ads API code yet** — that's Phase 2+. Don't pull in
  networking, auth, or gRPC dependencies without an explicit ask.
- **User-facing errors with source locations**: use `Diag` from
  `src/diagnostics.rs`. miette handles the rendering.
- **`.bid` extension is provisional** but stable for now — don't rename
  it as part of unrelated work.

## Docs site (`website/`)

The user-facing documentation site lives in `website/` and is built
with [Astro Starlight](https://starlight.astro.build/). Target audience
is **marketers**, not engineers — tone is plain-language, examples are
realistic campaigns (not `foo`/`bar`), and external links are preferred
over in-house explainers for general concepts (Git, OAuth, terminal
basics, HCL syntax).

- Local dev: `cd website && npm install && npm run dev`.
- Build: `npm run build` from inside `website/` — must complete clean
  before merging changes that touch docs.
- Deploy: `.github/workflows/docs.yml` rebuilds and pushes to
  `gh-pages` on every push to `main` that changes `website/**`.
- Live URL: `https://chrmod.github.io/bidsmith/`.

Source-of-truth layout:

```
website/src/content/docs/
├── index.mdx                    # splash home
├── welcome/                     # what is bidsmith, vs editor, workflow
├── before-you-start/            # install, github, google ads, first run
├── tutorials/                   # end-to-end walkthroughs
├── recipes/                     # short "how do I X" answers (mostly stubs)
├── concepts/                    # plain-language explainers (stubs)
├── commands/                    # CLI reference (stubs)
├── resources/                   # auto-generated reference (stubs)
└── reference/glossary.mdx       # terse, externally-linked
```

`recipes/`, `concepts/`, `commands/` are marked "coming soon" —
flesh them out as features land. `resources/` is fully populated and
auto-generated from `src/schema.rs` via `website/src/data/schema.json`
and the `AttributeTable.astro` component.

## Things to update alongside code

- Add resource → update DECISIONS.md "Validator covers" list,
  regenerate `website/src/data/schema.json`, and add a hand-written
  page under `website/src/content/docs/resources/<type>.mdx` (intro,
  realistic example, `<AttributeTable>` reference, see-also links).
  Wire the new page into `website/astro.config.mjs` sidebar.
- Add CLI verb or change a flag → update DECISIONS.md verbs table,
  README.md "Commands" section, **and**
  `website/src/content/docs/commands/index.mdx` (plus per-verb page if
  one exists).
- Settle an open decision → move it from ROADMAP.md "Open decisions"
  to DECISIONS.md "Locked decisions" with a one-line rationale.
- User-visible behavior change → write a `website/src/content/docs/recipes/`
  entry if it's something a marketer would search for ("how do I…?").

## Cutting a release

The Homebrew formula lives in a separate tap repo (`chrmod/homebrew-tap`,
cloned alongside this repo at `../homebrew-tap/`). Every release needs a
matching formula bump there — `brew upgrade` won't pick up new binaries
until the tap is pushed.

1. Bump `version` in `Cargo.toml`, run `cargo build --release` so
   `Cargo.lock` updates.
2. Commit, push `main`, then `git tag vX.Y.Z && git push origin vX.Y.Z` —
   `.github/workflows/release.yml` builds four binaries and publishes the
   GitHub Release.
3. Once the workflow is green, run `./scripts/bump-formula.sh X.Y.Z` —
   downloads the release assets, hashes them, regenerates
   `homebrew/bidsmith.rb`.
4. Copy that file into `../homebrew-tap/Formula/bidsmith.rb`, commit
   (`bidsmith X.Y.Z`), push.
5. Verify with `brew upgrade chrmod/tap/bidsmith` (or `brew install` on a
   fresh box).

Binaries are unsigned; the formula's `install` block ad-hoc signs on the
user's machine (`codesign --force --sign -`). The `chrmod/bidsmith` repo
must stay **public** — Homebrew downloads release assets anonymously.
