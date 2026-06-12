# Bidsmith — Roadmap

> Phases 1, 2, and the core of 3 are landed against the real
> rezolutnie `[W1]` campaign. The remaining items are the v2 work for
> Phase 3 (labels + removal), Phase 4 (refresh), and account-scoped
> resource types. Open decisions below; locked choices in
> [DECISIONS.md](DECISIONS.md).

## Architecture (envisioned)

```
.bid files (HCL2)
   │
   ▼
parser  ─►  AST
   │
   ▼
schema  ─►  typed resource graph  (lint, reference resolution)
   │
   ▼
planner ─►  mutate ops sent with validate_only=true, diffed against live state
   │
   ▼
applier ─►  GoogleAdsService.mutate (atomic per resource graph)
   │
   ▼
Google Ads API
```

**State model**: no `.tfstate`. Each managed resource carries a label
`bidsmith:address=<module.x.resource_type.name>`. `refresh` reads
labeled resources back into `.bid` files; drift detection compares
declared HCL against live labeled state. Local cache is rebuildable.

## Open decisions

- **Module composition v2 — `for_each`, outputs, directory sources,
  GitHub sources**: `locals`, `variable`, and a v1 `module "x" {
  source = "./file.bid" }` shipped. The v1 `module` is a single-file
  source, no outputs, no `for_each` — repeat the block to repeat the
  shape. The next layer (`for_each = var.cities`, `output "x" { value
  = … }`, multi-file directory sources, `source =
  "github.com/org/repo//path?ref=v1"`) waits on real users hitting
  the boundaries.
- Multi-account: how do `provider` blocks compose? One provider per
  file? Aliases? Today's `provider` block is single-customer, with the
  customer/login_customer ids overridable via env at `export` / `plan`
  / `apply` time — works for the rezolutnie loop but needs revisiting
  before bidsmith manages multiple customers in one tree.
- Lint catalog: starter set shipped (missing `status`, RSA min
  headlines/descriptions, phone-in-RSA). Still open: missing-negatives
  on search campaigns, declension hints for PL, RSA pinning advice,
  policy-wordlist patterns.
- RSA repeating-block diff strategy: today `headline` / `description`
  blocks are matched all-or-nothing (live trumps; we don't diff them
  per-asset). Open: detect added/removed/repinned assets and emit a
  granular update — or accept "replace the whole ad" as the only edit
  path (which matches how Google Ads operators usually edit RSAs).
- Removal mechanics on `apply`: once labels land, do we delete on
  apply (terraform-style "configured-state is reality") or require a
  separate `apply --allow-destroy` flag?

## Phases

**Phase 1 — Local parsing & validation (no API)**
- ✅ HCL2 parser (via `hcl-edit`)
- ✅ Resource AST + reference graph (two-pass collect/validate)
- Schemas for the smallest useful set:
  - ✅ `google_ads_campaign_budget`
  - ✅ `google_ads_campaign` (SEARCH)
  - ✅ `google_ads_ad_group`
  - ✅ `google_ads_ad_group_ad` (with `ad` → `responsive_search_ad` →
    pinned `headline { text, pin? }` / `description { text, pin? }`
    blocks)
  - ✅ `google_ads_ad_group_criterion` (keyword + match_type)
  - ✅ `google_ads_campaign_criterion` (keyword, location, language,
    proximity)
- ✅ `validate`: syntax + schema + references + lint warnings
- ✅ `fmt`: canonical re-emitter (parse → walk → emit; in-place or
  `--check`)
- ✅ Exit criterion: the rezolutnie `[W1] magazyny-energii` setup is
  expressible in `.bid` files, `validate` passes clean, `fmt --check`
  is a no-op on the canonical form, both `export` paths round-trip
  through `validate`.

**Phase 2 — Provider & plan**
- ✅ REST client over `reqwest::blocking` (chose REST over gRPC for the
  debugging UX during early iteration; tonic-build remains a future
  swap if we hit gRPC-only ergonomics)
- ✅ OAuth via Google's refresh-token endpoint; same env vars as
  `rezolutnie/.env`
- ✅ `plan`: parse + validate + import .bid, fetch live state via
  `googleAds:searchStream`, diff by name with parent cascade, scalar
  field-level drift detection, send a validateOnly batch with
  CREATE+UPDATE operations, merge per-resource accepted/rejected with
  no-ops into the output

**Phase 3 — Apply**
- ✅ `apply <path>` shows the validateOnly diff, prompts for `yes`,
  then mutates. `--auto-approve` skips the prompt (required for
  non-TTY runs). Same prepare stage as `plan`; validateOnly errors
  short-circuit before the prompt.
- ⏳ Write `bidsmith:address=...` labels on created/updated resources
  (state tracking via Google Ads Label + CampaignLabel / AdGroupLabel
  / AdGroupAdLabel / AdGroupCriterionLabel associations)
- ⏳ Removal detection: labeled live resources with no matching .bid
  entry → `- destroy`

**Phase 4 — Refresh / Import**
- ✅ `refresh`: bootstrap-mode import that pulls live state and writes
  it as a fmt-canonical `.bid`. Three output modes — stdout (default),
  single file (`-o PATH`), or split (`-d DIR` → `account.bid` for
  account-level resources, `campaigns.bid` for campaign-scoped).
  Reuses `live_state::fetch` + the new `export::render_split`
  renderer.
- ⏳ Reconcile-in-place mode: match resources by
  `bidsmith:address=` label (or `(resource_type, name)` for unlabeled
  ones), update fields without overwriting unrelated blocks. Blocked
  on Phase 3 v2 labels.
- ⏳ `import <address> <api-resource>`: adopt an unlabeled live
  resource into a specific `.bid` address.

**Phase 5 — Modules**
- ✅ Files-as-modules: each `.bid` file's basename is its implicit
  module name; addresses are `<module>.<type>.<name>`; references
  resolve same-module first, then globally with an ambiguity guard.
- ✅ Explicit `module "x" { source = "./file.bid", ...inputs }` blocks
  with local single-file sources. Each instance is an isolation
  boundary — its variables come from the block's attributes (+
  defaults), and addresses become `<instance>.<type>.<name>`. Wires
  through `validate`, `plan`, `apply`, and `import`.
- `for_each = var.cities` to instantiate one block per element.
- `output "x" { value = … }` so the parent can pull values out of a
  module instance.
- Directory sources (multiple `.bid` files per module) and nested
  module blocks.
- GitHub source resolution (`source = "github.com/org/repo//path?ref=v1"`).

**Phase 6 — AI integration**
- `.claude/skills/` — `/add-campaign`, `/review-search-terms`,
  `/explain-not-serving`
- `.github/workflows/` — agent PR review, apply on merge, nightly
  recommendation issues
- Template repo for fresh installs + Renovate-style upgrade bot

## Next session: start here

Phases 1 and 2 are done; Phase 3 has its CREATE/UPDATE half landed.
Priority order for what closes the most user-facing gaps:

1. **Live round-trip e2e test**. The `pull` verb landed — it runs
   the same SearchStream queries `plan --read-live` issues and writes
   the raw API JSON in the shape `export --from-gads-search-response`
   consumes (`bidsmith pull -o dump.json`). The remaining piece is the
   test loop itself: wire
   `apply → pull → export --from-gads-search-response → plan` as an
   opt-in `cargo test --features e2e` (or `bidsmith self-test --live`)
   that asserts the final `plan` reports `0 to create, 0 to update`
   against a dedicated Google Ads **test manager account**. Highest-
   value test the project can have: Google Ads itself is the oracle,
   the read path uses real bytes off the wire, and the write path is
   exercised end-to-end without invented mock assumptions. Details and
   design notes in
   [§ `pull` verb + live round-trip e2e](#pull-verb--live-round-trip-e2e)
   below.
2. **Phase 3 v2 — labels + removal**. Write
   `bidsmith:address=<address>` labels on every resource bidsmith
   creates / updates (via `Label` + `CampaignLabel` / `AdGroupLabel` /
   `AdGroupAdLabel` / `AdGroupCriterionLabel` associations). Use those
   labels at diff time to identify managed resources unambiguously
   (no more "matched by name" guessing), and emit `- destroy` rows
   for labeled live resources that no longer appear in `.bid`. Closes
   the lifecycle and makes adoption (`import`) and refresh tractable.
3. **Phase 4 v2 — reconcile-mode refresh**. The bootstrap-mode
   `refresh` shipped — `bidsmith refresh -d <DIR>` writes
   `account.bid` + `campaigns.bid` from live, and `-o <FILE>` /
   no-flag stdout variants exist for one-file workflows. What's still
   missing: matching live resources against an *existing* `.bid` and
   updating fields in place instead of overwriting. That needs the
   `bidsmith:address=` label plumbing from Phase 3 v2 to identify
   managed resources without name guessing. Until then, bootstrap
   refresh + `git diff` is the recovery loop.
4. **Account-scoped resource types in the live pipeline**. The
   schema, validator, renderer, adapter, and `live_state` queries
   for `google_ads_conversion_action`, `google_ads_call_asset`, and
   `google_ads_customer_asset` are all in place; offline CI exercises
   them via `examples/exports/raw.json`. Outstanding work is the live
   `apply` round-trip (needs the test manager account fixture to
   include account-level resources), and operator-side adoption —
   feeding bidsmith the Rezolutnie account's Lead/Phone conversion
   actions and `+48 510 019 081` call asset via `refresh -d`.

Smaller follow-ups that can ride along:

- Repeating-block field-level diff for RSA `headline` / `description`
  blocks and the `final_urls` list. Today these are matched
  all-or-nothing; per-asset add/remove/repin detection would close
  the last drift gap below the apply layer.
- Campaign label support beyond bidsmith state — making `[W1]` a real
  campaign label (rather than a name prefix) once the label
  infrastructure exists for state tracking.
- Tighten `fmt` ↔ export alignment for non-bidsmith outputs (the
  internal renderer already pipes through fmt; external pretty-print
  use cases may want their own knobs).
- ✅ `bidsmith auth` subcommand — `login` runs a browser OAuth loopback
  + PKCE authorization-code flow, then writes `~/.bidsmith/credentials.toml`
  (`0600`) and lists the accounts `listAccessibleCustomers` returns;
  `status` / `logout` / `profile` round out the set. Credentials resolve
  env var → file → bundled default, per value, so CI/env-var setups are
  unchanged. **Phase A (shipped)** supports a bring-your-own OAuth client
  (`--client-id`/`--client-secret` or env), so any agency that creates one
  "Desktop app" client is productive today. **Phase B (remaining):**
  register + verify the bundled bidsmith OAuth client with Google
  (sensitive-scope verification — app name, logo, privacy-policy URL, demo
  video) and inject it at release-build time via
  `option_env!("BIDSMITH_DEFAULT_CLIENT_ID"/..._SECRET)`; that flips on the
  zero-config solo path with no code change. A hosted-helper variant
  (`bidsmith.dev/auth`) is a later follow-up if the local-browser flow
  proves not enough.
- Windows binary distribution — `.exe` build via `cross` or
  `cargo-dist`, plus a `scoop` or `winget` recipe to mirror the
  Homebrew tap UX. Today's macOS + Linux targets cover engineers but
  block marketers on Windows laptops; the docs site will mark
  Windows as "coming soon" until this lands (with WSL as the
  documented interim path).

## `pull` verb + live round-trip e2e

> Status: `pull` shipped (signature `run(output, verbose)`); e2e test
> still to write. The `--customer-id` / `--campaign-id` scoping flags
> in the original sketch below were skipped for v1 — the verb dumps
> everything for the customer in env, matching what `plan --read-live`
> reads. Add scoping flags when there's a concrete need.

### Why

Today the test plan is the "Verified locally" checklist in
DECISIONS.md, run by hand. We have no automated coverage at all (zero
`#[test]` blocks in the tree). HTTP-mock based tests work in principle
but risk encoding hand-written assumptions about wire shapes that
don't match what Google actually emits.

The strongest oracle available is Google Ads itself. The loop
`apply → pull → export → plan` against a test account proves, in one
shot:

- the renderer round-trips every field bidsmith claims to model;
- `adapt::from_search_response` doesn't drop anything we sent in;
- the mutate request bodies in `api/mutate.rs` are accepted by the
  real API (not just by a fixture);
- the diff engine in `api/diff.rs` correctly recognises a freshly
  applied state as drift-free.

Wins this catches that fixture-replay tests can't: Google adding a new
required field, our enum spelling drifting from the API, server-side
defaults filling in fields we don't render.

### The `pull` verb

Add a subcommand alongside `plan` / `apply` / `query`. Reuses the
existing `live_state::QUERIES` table and `Client::search_stream`; the
*only* difference vs `live_state::fetch` is that it skips the adapter
and writes the accumulated batches as a JSON array to disk in the same
shape as `examples/exports/raw.json`.

```
bidsmith pull -o dump.json
bidsmith pull --customer-id 1234567890 -o dump.json
bidsmith pull --campaign-id 9876543210 -o dump.json   # optional filter
```

Implementation sketch (`src/commands/pull.rs`):

```rust
pub fn run(output: Option<&str>, campaign_id: Option<&str>, verbose: bool) -> ExitCode {
    // 1. Same OAuth + client construction as plan/apply.
    // 2. Loop over live_state::QUERIES; if --campaign-id is set,
    //    append `AND campaign.id = <id>` (or the per-resource
    //    equivalent) to each query.
    // 3. Accumulate batches into one Vec<Value>.
    // 4. Pretty-print as JSON array to `output` (or stdout).
}
```

Notes:

- The QUERIES table is already the source of truth for "everything
  bidsmith models." Whenever a new resource type gets a query there,
  `pull` picks it up for free — that's the same lockstep rule as the
  validator/renderer pair.
- Output is the *raw* SearchStream JSON, not the adapted ExportInput.
  Adapter logic stays in `commands/adapt.rs` so `pull` and
  `--from-gads-search-response` share zero new code.
- `--campaign-id` is the practical scoping flag; matches what
  `dump_campaign.py` already does for fixture generation.

Side benefit: makes it trivial to commit small, redacted real dumps
under `tests/fixtures/` as offline test inputs for the read path.

### The e2e test

Opt-in tier. Gated on a single env var (`BIDSMITH_E2E_CUSTOMER_ID`)
that points at a dedicated Google Ads **test manager account** — a
free, non-serving account flagged at MCC creation time. If the var
isn't set, the test is skipped (not failed). Either a Cargo feature
(`cargo test --features e2e`) or a hidden CLI verb
(`bidsmith self-test --live`); the latter is friendlier for ad-hoc
runs against a real account without a Rust toolchain.

The loop:

```
1. fixture.bid           ← small canonical .bid (one budget, one
                           campaign, one ad group, one RSA, a few
                           keyword + location criteria)
2. rewrite all resource names with prefix "bidsmith-e2e-<run-id>-"
3. bidsmith apply --auto-approve fixture.bid
4. bidsmith pull -o /tmp/dump.json
5. bidsmith export --from-gads-search-response /tmp/dump.json \
       -o /tmp/roundtrip.bid
6. bidsmith fmt --check /tmp/roundtrip.bid         (should be no-op)
7. bidsmith plan /tmp/roundtrip.bid                MUST report
                                                   0 to create,
                                                   0 to update
8. (optional) edit one scalar in roundtrip.bid → plan reports
   exactly 1 update — proves drift detection against the same live
   state we just pulled
9. teardown (always runs, even on failure): REMOVE every resource
   whose name begins with "bidsmith-e2e-"
```

The pivotal assertion is step 7. Strict text equality between
`fixture.bid` and `roundtrip.bid` is *too strict* — the API legitimately
fills in defaults, normalises proximity to micro-degrees, assigns
`resourceName` / `id`, etc. "Plan is a no-op" is the semantically
correct invariant: it says the file we'd commit matches reality.

### Test-account hygiene

- **Dedicated test manager account**, separate from any production
  account, so a runaway test can't touch real campaigns or spend.
  Test manager accounts can't serve ads and don't spend money — exactly
  what we want.
- **Unique name prefix per run** (`bidsmith-e2e-<timestamp>-` or
  `<git-sha>-`). Two pre-flight steps: sweep anything matching the
  prefix older than 1 hour before the run starts (defensive cleanup
  for prior aborted runs), and assert there are zero matches for the
  current run-id (concurrency guard).
- **Teardown runs unconditionally** via `Drop` on a guard struct (or
  a `defer!`-style cleanup) so a panic mid-test still removes the
  resources it created.
- **CI integration**: a separate workflow file, nightly schedule, with
  the test-account creds in repo secrets. Not gated on every PR — too
  slow, too much quota.

### What this still doesn't cover

- Apply failure paths (rejected validateOnly, retired API version
  surfaced as 404, expired refresh token). Capture real error
  responses as fixtures once observed and replay them in unit tests
  in `api/client.rs`.
- Removal / `- destroy` flow (waits on Phase 3 v2 labels).
- Multi-customer / module composition (waits on the open decision in
  the "Open decisions" section above).
