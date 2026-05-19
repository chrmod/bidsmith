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
- Lint catalog: starter set shipped (missing `status`, RSA min
  headlines/descriptions, phone-in-RSA). Still open: missing-negatives
  on search campaigns, declension hints for PL, RSA pinning advice,
  policy-wordlist patterns.

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

1. **Account-scoped resource types**, unblocked by the W1 trial:
   - `google_ads_asset` (especially `CallAsset` for the +PL phone)
   - `google_ads_customer_asset` (wire account-level call assets)
   - `google_ads_conversion_action` (Lead, Phone — referenced from the
     account, not from a single campaign)
   These let bidsmith own the policy fix in W1: move the literal phone
   out of RSA copy into a Call asset. They also unblock the next-most-
   common refresh paths.
2. **Re-trial against W1** once the asset/conversion types land. Verify
   we round-trip the full account-scoped graph, not just one campaign.
3. **Exit criterion**: the rezolutnie `[W1] magazyny-energii` setup
   expressible in `.bid` files; `validate` passes; `fmt` is a no-op on
   the canonical form; `export` round-trips a real campaign JSON
   through `validate`. Campaign-scoped resources, RSA pinning, and
   proximity are in; account-scoped pieces above are what's left.

Smaller follow-ups that can ride along:

- Campaign label support — `[W1]` is encoded in the name today; making
  it a real `label` would let us migrate off the name-prefix
  convention.
- Repeating `text { value, pin? }` for non-RSA ad types when they land
  (Discovery, Performance Max ad-strength assets) — share the
  asset-block design we have for RSA headlines.
