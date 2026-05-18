# Bidsmith — Roadmap

> **Status: speculative.** This roadmap was drafted before a
> requirements-driven pass against a real campaign (the `rezolutnie/ads`
> seed). A follow-up agent grounded in actual `[W1]` needs is expected
> to revise priorities and phases. The locked choices in
> [DECISIONS.md](DECISIONS.md) stand; everything below is up for
> revision.

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

- Google Ads SDK: community `google-ads-rs` (thin) vs generate our own
  bindings from Google's published `.proto` files via `tonic-build`.
  Lean toward the latter for ergonomics + version control.
- Auth flow: port the rezolutnie OAuth helper (one-shot refresh token to
  `.env`) to `oauth2` + `reqwest`, or build a richer interactive
  credential manager.
- Multi-account: how do `provider` blocks compose? One provider per
  file? Aliases?
- Lint catalog: which policy heuristics ship by default (phone-in-RSA,
  headline counts, missing negatives, declension hints for PL)?
- `export` input shape: today we accept a clean bidsmith-flavored JSON.
  Should we also accept raw Google Ads API JSON (camelCase,
  `resourceName` paths) directly, or keep that as a separate adapter
  step? Tradeoff: one-step ergonomics vs. a small focused renderer.

## Phases

**Phase 1 — Local parsing & validation (no API)**
- ✅ HCL2 parser (via `hcl-edit`)
- ✅ Resource AST + reference graph (two-pass collect/validate)
- Schemas for the smallest useful set:
  - ✅ `google_ads_campaign_budget`
  - ✅ `google_ads_campaign` (SEARCH)
  - ✅ `google_ads_ad_group`
  - ⏳ `google_ads_ad_group_ad`
  - ⏳ `google_ads_ad_group_criterion`
  - ⏳ `google_ads_campaign_criterion`
- ✅ `validate`: syntax + schema + references (lint TBD)
- ⏳ `fmt`: rewrite canonical (hcl-edit already preserves layout —
  likely a thin wrapper)
- Exit criterion: the rezolutnie `[W1] magazyny-energii` setup
  expressible in `.bid` files; `validate` passes.

**Phase 2 — Provider & plan**
- Google Ads SDK: generate Rust bindings via `tonic-build` from
  Google's `.proto` files, or evaluate `google-ads-rs` community crate
- OAuth helper (port `rezolutnie/ads/oauth_flow.py` → `oauth2` +
  `reqwest`)
- `plan`: builds mutate ops, sends `validate_only=true`, prints diff
  vs live (GAQL queries)

**Phase 3 — Apply**
- `apply --confirm` executes mutates
- Writes `bidsmith:address=...` labels on created resources
- Idempotent by label lookup

**Phase 4 — Refresh / Import**
- `refresh`: regenerate `.bid` from labeled live resources
- `import <address> <api-resource>`: adopt an unlabeled live resource
- Shares the rendering layer with `export` (refresh = export + live
  API client). Grow the renderer once; both commands consume it.

**Phase 5 — Modules**
- `module "x" { source = "..." for_each = ... }`
- Local + GitHub source resolution

**Phase 6 — AI integration**
- `.claude/skills/` — `/add-campaign`, `/review-search-terms`,
  `/explain-not-serving`
- `.github/workflows/` — agent PR review, apply on merge, nightly
  recommendation issues
- Template repo for fresh installs + Renovate-style upgrade bot

## Next session: start here

Phase 1 isn't quite done. Concrete picks for the next session, in
priority order:

1. **List/array literal support in the schema**. `hcl-edit` already
   parses `[a, b, c]` into `Expression::Array`. Extend `FieldType` with
   `List(Box<FieldType>)` and teach `validate_value` to walk it.
   Unblocks `final_urls`, `headlines`, etc.
2. **Add the remaining resource schemas** in `src/schema.rs` *and* the
   matching renderers in `src/commands/export.rs` (the two must stay
   in lockstep — anything `validate` knows about, `export` should be
   able to produce):
   - `google_ads_ad_group_ad` (with the `ad` nested block; needs list
     support)
   - `google_ads_ad_group_criterion` (keyword + match_type)
   - `google_ads_campaign_criterion` (negative keyword, location, etc.)
3. **Real Google Ads JSON adapter for `export`**. Accept the camelCase
   / `resourceName`-flavored output of `googleads-python-lib`
   SearchStream so we can pull a real campaign and feed it in without
   hand-shaping. Either a `--from-gads-search-response` flag or a
   separate `bidsmith adapt` step (see open decisions).
4. **Lint pass**. After type-checking, walk the AST emitting `Diag`s
   with miette severity = warning for soft issues: missing-`status`,
   too few headlines on RSA, phone-number in ad copy. The plumbing for
   warnings already exists in miette — pass `severity` to `Diag`.
5. **`fmt` command**. `hcl-edit` preserves layout — `cargo run -- fmt
   path` could be roughly: parse → re-emit via `Body::to_string()`
   with our own alignment rules.
6. **Exit criterion**: the rezolutnie `[W1] magazyny-energii` setup
   expressible in `.bid` files; `validate` passes; `fmt` is a no-op on
   the canonical form; `export` round-trips a real campaign JSON
   through `validate`.
