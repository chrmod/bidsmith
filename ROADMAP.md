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
- `refresh`: regenerate `.bid` from labeled live resources
- `import <address> <api-resource>`: adopt an unlabeled live resource
- Shares the rendering layer with `export` (refresh = export + live
  API client). Grow the renderer once; both commands consume it.

**Phase 5 — Modules**
- ✅ Files-as-modules: each `.bid` file's basename is its implicit
  module name; addresses are `<module>.<type>.<name>`; references
  resolve same-module first, then globally with an ambiguity guard.
- `module "x" { source = "..." for_each = ... }` — explicit module
  blocks layered on top of the implicit one.
- Local + GitHub source resolution.

**Phase 6 — AI integration**
- `.claude/skills/` — `/add-campaign`, `/review-search-terms`,
  `/explain-not-serving`
- `.github/workflows/` — agent PR review, apply on merge, nightly
  recommendation issues
- Template repo for fresh installs + Renovate-style upgrade bot

## Next session: start here

Phases 1 and 2 are done; Phase 3 has its CREATE/UPDATE half landed.
Priority order for what closes the most user-facing gaps:

1. **Phase 3 v2 — labels + removal**. Write
   `bidsmith:address=<address>` labels on every resource bidsmith
   creates / updates (via `Label` + `CampaignLabel` / `AdGroupLabel` /
   `AdGroupAdLabel` / `AdGroupCriterionLabel` associations). Use those
   labels at diff time to identify managed resources unambiguously
   (no more "matched by name" guessing), and emit `- destroy` rows
   for labeled live resources that no longer appear in `.bid`. Closes
   the lifecycle and makes adoption (`import`) and refresh tractable.
2. **Phase 4 — refresh**. `bidsmith refresh -o <file>` walks labeled
   live resources, runs them through the existing renderer, writes a
   canonical `.bid`. Reuses `live_state.rs` + the renderer; the new
   work is a label-driven filter plus a small CLI.
3. **Account-scoped resource types**, unblocked by the W1 trial:
   - `google_ads_asset` (especially `CallAsset` for the +PL phone)
   - `google_ads_customer_asset` (wire account-level call assets)
   - `google_ads_conversion_action` (Lead, Phone — referenced from
     the account, not from a single campaign)
   These let bidsmith own the policy fix in W1: move the literal
   phone out of RSA copy into a proper Call asset.

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
