# Bidsmith — Roadmap

> Phases 1, 2, and 3 (including the v2 identity labels + whole-resource
> removal) are landed against the real rezolutnie `[W1]` campaign. The
> remaining items are Phase 4 v2 (reconcile-mode refresh + `import`) and
> the account-scoped resource types' live round-trip. Open decisions
> below; locked choices in [DECISIONS.md](DECISIONS.md).

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

- **Module composition v2 — outputs, directory sources, GitHub
  sources**: `locals`, `variable`, a v1 `module "x" { source =
  "./file.bid" }`, and `module` `for_each` (instantiate one source N
  times from a variant map) shipped. The remaining layer (`output "x"
  { value = … }`, reference-typed module inputs, multi-file directory
  sources, `source = "github.com/org/repo//path?ref=v1"`) waits on
  real users hitting the boundaries — issue #87's shared-shell case,
  which module outputs would also have served, was settled with the
  lighter `defaults` block + resource `for_each` instead (see
  DECISIONS.md "`defaults` block").
- Multi-account: how do `provider` blocks compose? **Partially
  resolved:** a per-project `bidsmith.toml` (`customer_id` /
  `login_customer_id` / optional `developer_token`) now supplies the
  target per folder, the provider block's `customer_id` is optional, and
  the resolved target drives the live client end-to-end — so
  account-agnostic `.bid` files are applied to different accounts by
  `cd`-ing into the right folder (target precedence: env → `bidsmith.toml`
  → provider block → global credentials). Still open: in-tree provider
  aliases / a single command spanning several customers in one run, and
  how that composes with modules.
- Lint catalog: starter set shipped (missing `status`, RSA min
  headlines/descriptions, phone-in-RSA). Still open: missing-negatives
  on search campaigns, declension hints for PL, RSA pinning advice,
  policy-wordlist patterns.
- RSA repeating-block diff strategy: today `headline` / `description`
  blocks are matched all-or-nothing (live trumps; we don't diff them
  per-asset). Open: detect added/removed/repinned assets and emit a
  granular update — or accept "replace the whole ad" as the only edit
  path (which matches how Google Ads operators usually edit RSAs).
- Removal mechanics on `apply`: **resolved.** Orphaned criteria
  *members* and *whole* labeled resources (campaign / ad_group /
  ad_group_ad) both plan as `- destroy` and apply terraform-style
  through the normal `yes` prompt — no `--allow-destroy` flag. Whole
  resources are identified by the `bidsmith:address` label (Phase 3 v2);
  unlabeled UI-created resources are never destroyed (see DECISIONS.md
  "Identity labels"). Removing the *last* declared member of a criterion
  category is covered by the per-category `bidsmith:owns=` claim label
  on the parent (issue #88; see DECISIONS.md "Member removal").

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
- ✅ Write `bidsmith:address=...` labels on created / adopted resources
  (Label + CampaignLabel / AdGroupLabel / AdGroupAdLabel associations).
  Labelable types only: campaign, ad_group, ad_group_ad — keywords stay
  unlabeled (the API forbids labels on negative criteria; their
  lifecycle rides on member removal + the parent's category claim). See
  **Identity labels (Phase 3 v2)** in DECISIONS.md.
- ✅ Member-level removal detection: an orphaned criterion whose
  declared parent still exists → `- destroy`, scoped to the
  `(parent, category)` bidsmith owns — claimed by ≥1 declared member or
  by the parent's persisted `bidsmith:owns=<category>` label, so the
  destroys survive removing the category's last declared member
  (issue #88).
- ✅ 1:1 ad matching by body (issue #44): `plan` matches each declared
  `google_ads_ad_group_ad` to a live ad keyed on the ad body (final
  URLs + RSA content), not on the ad group alone. Accounts routinely
  hold several same-bodied ads differing only by status; the old "first
  ad in the group" key collapsed them all onto one live id, which read
  as spurious `~ update (status)` rows, "Cannot mutate the same resource
  twice" rejections, and bogus creates for the unmatched ads — leaving
  the whole batch un-applyable. Within a body bucket, ads that already
  match are claimed first (no diff), the rest become status updates, and
  any declared ad with no live body left is a create, so a `plan`
  straight after `refresh` is a clean no-op. Ads still match by body
  (copy is identity); the label authorizes their cleanup.
- ✅ Whole-resource removal detection: a labeled live campaign /
  ad_group / ad_group_ad with no matching `.bid` entry → `- destroy`
  (children cascade, ordered child-first). Unlabeled live resources are
  left untouched. Gated by the normal `apply` prompt.
- ✅ `bidsmith mv` stays source-only — no second half needed. After a
  rename the moved resource is re-adopted by content fallback and
  `apply` reconciles its `bidsmith:address` label declaratively, so the
  live label rewrite falls out of the normal flow. Terraform-style
  `moved {}` blocks remain deferred (nothing forces them now).

**Phase 4 — Refresh / Import**
- ✅ `refresh`: bootstrap-mode import that pulls live state and writes
  it as a fmt-canonical `.bid`. Three output modes — stdout (default),
  single file (`-o PATH`), or split (`-d DIR` → `account.bid` for
  account-level resources, `campaigns.bid` for campaign-scoped).
  Reuses `live_state::fetch` + the new `export::render_split`
  renderer.
- ✅ Reconcile-in-place mode (`refresh --in-place [PATH]`): match
  resources by `bidsmith:address=` label (reusing the planner's
  label-first diff), then write back only the drifted **scalar**
  fields, updating attribute values in place without overwriting
  unrelated blocks, comments, ordering, or unmanaged resources.
  `--check` previews without writing; a `mv`-style baseline-error
  guard refuses to write a project-breaking edit. Scope is narrow by
  design: it patches only attributes already present in source (an
  absent attribute is reported, not inserted) and only 1:1-block
  scalar kinds. Structural drift (ad copy, keyword/criterion
  membership) is reported, not edited — the diff engine only emits
  scalar `Update`s, so changed copy is a create+destroy handled by
  `apply`. Pure core (`reconcile_sources`) is unit-tested offline.
  Loads the tree through the same `Program` path `validate` / `plan`
  use (issue #93), so `module` templates reconcile as instance scopes
  instead of failing as standalone roots; a shared template is only
  patched where every instance drifted to the same value, and never
  where the attribute holds a `var.` / `local.` / reference.
- ⏳ `import <address> <api-resource>`: adopt an unlabeled live
  resource into a specific `.bid` address. Now unblocked — apply
  already adopts unlabeled live resources by content and labels them;
  `import` is the explicit, address-targeted form of that.

**Phase 5 — Modules**
- ✅ Files-as-modules: each `.bid` file's basename is its implicit
  module name; addresses are `<module>.<type>.<name>`; references
  resolve same-module first, then globally with an ambiguity guard.
- ✅ Explicit `module "x" { source = "./file.bid", ...inputs }` blocks
  with local single-file sources. Each instance is an isolation
  boundary — its variables come from the block's attributes (+
  defaults), and addresses become `<instance>.<type>.<name>`. Wires
  through `validate`, `plan`, `apply`, and `import`.
- ✅ `module` `for_each = { … }` instantiates the source once per map
  entry (instance address `<label>.<key>.<type>.<name>`); inputs merge
  the block's shared attributes with each entry's object. Collapses N
  hand-written clone files into one template plus a variant table.
- `output "x" { value = … }` so the parent can pull values out of a
  module instance.
- Directory sources (multiple `.bid` files per module) and nested
  module blocks.
- GitHub source resolution (`source = "github.com/org/repo//path?ref=v1"`).

**Phase 6 — AI integration**
- `.claude/skills/` — `/add-campaign`, `/review-search-terms`,
  `/explain-not-serving`
- `.github/workflows/` — ✅ apply on merge + plan-on-PR (scaffolded by
  `bidsmith init`: `plan --format markdown --detailed-exitcode` posts a
  sticky PR comment, `apply --auto-approve` runs on merge to `main`).
  Remaining: agent PR review, nightly recommendation issues.
- Template repo for fresh installs + Renovate-style upgrade bot —
  ✅ partial: `bidsmith init` generates the skeleton (starter `.bid`,
  `bidsmith.toml`, GitOps workflow, `.gitignore`, README). A hosted
  template repo and the upgrade bot remain.

## Next session: start here

Phases 1 and 2 are done; Phase 3 is complete — the CREATE/UPDATE half,
the live round-trip e2e test (`tests/e2e.rs`, opt-in via
`cargo test --features e2e`), **and the v2 identity labels + whole-
resource removal** all landed. Campaigns / ad groups / ads now match by
their `bidsmith:address` label (content fallback adopts unlabeled live
resources), a labeled resource dropped from the `.bid` is destroyed, and
`mv` stays source-only because `apply` reconciles labels declaratively.
See **Identity labels (Phase 3 v2)** in DECISIONS.md. Priority order for
what closes the most user-facing gaps next:

1. ✅ **Phase 4 v2 — reconcile-mode refresh** (shipped). `bidsmith
   refresh --in-place [PATH]` matches live resources to an *existing*
   `.bid` by their `bidsmith:address` label and writes back drifted
   scalar fields in place, leaving structure intact. The bootstrap
   modes (`-d` / `-o` / stdout) still exist for first-time pulls.
   A campaign's `frequency_caps` set round-trips too — the whole
   block set is rewritten from live (issue #102). Remaining reconcile
   follow-ups: inserting an attribute that's absent from source (today
   reported, not written) and per-asset RSA / criterion-membership
   reconcile (waits on the per-asset RSA diff below). Next priorities:
2. **`import <address> <api-resource>`** (unblocked). `apply`
   already adopts unlabeled live resources by content and labels them;
   `import` is the explicit, address-targeted form — adopt one named
   live resource into a chosen `.bid` address without a full refresh.
3. **Account-scoped resource types in the live pipeline** (independent;
   can ride alongside). The schema, validator, renderer, adapter,
   and `live_state` queries for `google_ads_conversion_action`,
   `google_ads_call_asset`, and `google_ads_customer_asset` are all in
   place; offline CI exercises them via `examples/exports/raw.json`.
   Outstanding work is the live `apply` round-trip (extend the
   `tests/e2e.rs` fixture to include account-level resources), and
   operator-side adoption — feeding bidsmith the Rezolutnie account's
   Lead/Phone conversion actions and `+48 510 019 081` call asset via
   `refresh -d`.

Smaller independent wins that need no labels: expand the **lint
catalog** (missing-negatives on search campaigns, PL declension hints,
RSA pinning advice, policy-wordlist patterns), **per-asset RSA
diff** (today `headline` / `description` blocks + `final_urls` match
all-or-nothing — the last drift gap below the apply layer), and
**Windows binary distribution** (`.exe` via `cargo-dist` / `cross` +
scoop / winget). See the follow-ups list above and "Open decisions" for
the full set.

Smaller follow-ups that can ride along:

- ✅ Inline campaign targeting (`languages = ["en"]` / `locations =
  ["US"]`) — each entry expands to one positive `campaign_criterion`,
  resolving human-readable codes (or raw `…Constants/NNNN` strings) via
  the in-binary `src/targeting.rs` tables. Country geo constants are
  generated from the stable `2000 + ISO-3166-1-numeric` rule; languages
  are a curated code→id table. The planner matches by resolved constant,
  so converting explicit `location {}` / `language {}` criteria to inline
  — or adopting live targets — is drift-free, and `refresh` / `export`
  emit the inline form by default. `validate` forbids declaring the same
  axis both inline and as an explicit positive criterion (issue #37).
- ✅ `bidsmith mv <from> <to>` — rename a resource's address (block
  label + every reference) as a format-preserving source rewrite.
  Because the planner matches live resources by content, not by
  address, the rename is a no-op against the account — the path from a
  refresh dump's counter-suffixed names to a hand-maintained tree
  without recreating live resources (issue #35). Now that the
  `bidsmith:address` label is the matching key (Phase 3 v2), `mv` still
  needs no second half: after the source rewrite the moved resource is
  re-adopted by content fallback and `apply` reconciles its label
  declaratively (add the new association, drop the stale one). Terraform-
  style `moved {}` blocks stay **deferred** — nothing forces a
  plan-visible move construct now; revisit only if a content-ambiguous
  rename ever needs to pin identity explicitly.
- ✅ List / map locals (issue #39) — a `local` holds lists and maps, not
  just scalars, and a `local.<name>` that resolves to a list is usable
  in every list attribute (RSA `headlines` / `descriptions`,
  `final_urls`, inline `languages` / `locations`, compact `keywords`
  `texts` / `match_types`). The compact keyword block doubles as the
  "repeated block from a list" form (`keywords { texts = local.theme }`
  fans out into one criterion per keyword), so no `dynamic` / `for_each`
  block-expansion construct was added. Cross-file de-duplication rides
  the existing same-module-then-global-fallback resolution: shared lists
  live in one `shared.bid` and are referenced everywhere. Validator and
  RSA lints resolve list references; `examples/lists/` covers it.
  **Deferred:** map indexing (`local.headlines["ublock"]`) and other
  element-level expressions wait on the expression engine; `variable`
  blocks stay scalar-only (list data belongs in `locals`).
- ✅ `defaults` block (issue #87) — a type-scoped
  `defaults "google_ads_campaign" { … }` supplies attribute /
  nested-block defaults to every resource of that type, overridable per
  resource (blocks override wholesale). Merged at import time, so
  adopting it over a live account plans as a no-op. Settled as the
  issue's option B, paired with resource `for_each` (#86) for the
  device-criteria trio; module outputs (option A) and reference-typed
  module inputs (option C) stay deferred under "Module composition v2".
- ✅ `for_each` on resource blocks (issue #86) — one `resource` block
  plus a list (`["MOBILE", "TABLET"]`) or map of references fans out
  into one instance per entry, with `each.key` / `each.value`
  substitution and keyed addresses (`t_devices["MOBILE"]`). Load-time
  expansion (`src/expand.rs`) shared by validate / plan / apply /
  refresh / lints; `fmt` / `mv` see raw source. Collapses the
  per-campaign device-exclusion pair and N-sitelink `campaign_asset`
  attachments (the two measured ghostery/marketing patterns).
  **Deferred:** referencing a keyed instance from another resource
  (`google_ads_campaign.t["a"].id`) and `each.value.<attr>` on object
  values.
- ✅ Reusable ad bodies via `ad_template` (issue #40) — a top-level
  `ad_template "name" { … }` declares an `ad {}` body once, and a
  `google_ads_ad_group_ad` attaches it with `template = ad_template.<name>`
  instead of an inline `ad {}` block (exactly one of the two is required).
  The reference is resolved and substituted at import time, so each
  per-ad-group resource keeps its own address and the mutate is identical
  to the inline body — adopting a template on a live account is a no-op
  `plan` (chosen over the fan-out "one ad → N ad groups" form, which would
  re-address live ads into delete+create). Templates resolve same-module
  then global, so one template serves campaigns across files; the
  template's RSA is linted at its declaration. `examples/ad-templates/`
  covers it.
- ✅ Per-instance `ad_template` overrides (issue #58) — a
  `google_ads_ad_group_ad` attaching a `template` may also set
  `final_urls` / `path1` / `path2` on the resource; each overrides the
  template body field, unset fields inherit. Applied at import time, so
  the merged mutate matches an inline body and `plan` is unchanged.
  `final_urls` is now optional on `ad_template` (a URL-agnostic template
  lets every reference supply its own; a reference that supplies none
  fails `validate`) but stays required on an inline `ad {}` block.
  Collapses the near-duplicate templates that existed only to vary the
  landing URL. **Deferred:** per-asset headline/description overrides and
  the `ad_groups = [...]` fan-out form.
- ✅ Folding emitter (issue #57) — `refresh` / `export` recognize repeated
  structure and emit the compact constructs instead of re-exploding the
  tree every pull. Repeated ad bodies fold into one `ad_template`
  (URL-variant bodies onto one URL-agnostic template + #58 per-instance
  overrides); RSA arrays used by ≥ 2 sites and campaign negative lists
  shared by ≥ 2 campaigns fold into `locals`. Folding is source-only —
  every construct expands to the identical mutate at import time, so the
  folded tree round-trips through `validate` / `plan` exactly like the
  verbose one (enforced offline by `fold_roundtrips_to_verbose`:
  render → import → re-render unfolded, assert identical). Chosen `locals`
  over the issue's literal `google_ads_shared_set` for the negatives fold:
  live negatives are per-campaign criteria, so emitting a SharedSet would
  plan as a real create+attach+destroy migration, breaking the zero-drift
  property a refresh must hold. **Deferred:** ad-group negative lists, and
  emitting an actual `shared_set` once Phase 3 v2 labels make adopting one
  a no-op against live.
- ✅ YouTube video ads — end to end. A
  `google_ads_youtube_video_asset` references an already-published
  YouTube video by `youtube_video_id`; a `video_responsive_ad` block
  inside an `ad {}` attaches it as the creative for a `VIDEO`-channel
  campaign (schema / validate / `fmt` / `export` / `refresh` renderer +
  round-trip, `examples/video/`). `plan` / `apply` create the asset and
  the creative like any other resource: the asset matches live by video
  id (so it is never duplicated), and the creative is create-only the way
  an RSA body is. bidsmith never uploads the video (that's the YouTube
  Data API); that one boundary is communicated from the CLI via
  `export::video_upload_notice`. See DECISIONS.md "YouTube video ads".
  **Deferred:** a `google_ads_call_to_action_asset` resource, which is
  what a Demand Gen `call_to_actions` needs (today a declared value
  blocks the create with an explanatory error); extending the
  `tests/e2e.rs` fixture with a video campaign; multiple `videos` per
  `video_responsive_ad` (today one `video` ref); companion / logo images;
  and per-asset RSA/video-asset diffs.
- ✅ Video bidding strategies (issue #104). `google_ads_campaign` takes
  one bidding block out of `manual_cpc` / `manual_cpm` / `manual_cpv` /
  `target_cpm` / `target_cpv`, so a video campaign's strategy round-trips
  instead of being invisible. **This does not make a new VIDEO campaign
  appliable** — the issue assumed the bidding field was the blocker, but
  live testing showed Google refuses *every* create and update on the
  channel (`MUTATE_NOT_ALLOWED`), which their docs confirm. Video is
  adopt-only; `plan` now warns before sending a batch that contains one.
  **Deferred:** `target_cpm`'s optional
  `target_frequency_goal` (target-frequency reach buying), `fixed_cpm`,
  a portfolio `google_ads_bidding_strategy` resource, and the
  conversion-based strategies (`target_cpa`, `maximize_conversions`, …),
  which want conversion tracking modelled first.
- ✅ Campaign flight windows (issue #113). `google_ads_campaign` takes
  `start_date` / `end_date`, so a time-boxed test carries its own stop
  instead of relying on someone remembering. Verified live: eleven
  ENABLED campaigns on the account carried Google's `2037-12-30`
  "no end date" sentinel, including three video campaigns documented
  in-repo as 14-day tests — at EUR 20/day each, the gap between the
  intended flight and the configured one is roughly EUR 840 against
  roughly EUR 240k of exposure. Six of the eleven are SEARCH, where
  bidsmith can write the correction; a `validateOnly` update of
  `end_date` on a live search campaign is accepted. The sentinel maps to
  an omitted attribute so adoption doesn't bake a fake date into every
  file, dates are validated as real calendar dates, and `lint` warns on
  a window that ends before it starts.
  **Deferred:** an `end_date`-in-the-past warning (needs injectable
  time), and dates on other resources — only the campaign has them
  today.
- ✅ Advanced location options (issue #114). `google_ads_campaign` takes
  a `geo_target_type_setting { positive_geo_target_type,
  negative_geo_target_type }` block, so a campaign whose whole identity
  is a market can declare that it means people *in* it rather than
  people anywhere interested in it. Reported live on the account as
  `PRESENCE` for the three `GH_YouTube_*` in-stream campaigns — the
  value we wanted, but by way of a UI default nothing was checking, and
  on Google's `PRESENCE_OR_INTEREST` default a per-market A/B stops
  being comparable. Each side is managed on its own, an omitted one is
  unmanaged, and the `UNKNOWN` the API reports for a type it has no
  value for maps to an omitted attribute rather than into the file.
  **Deferred:** the deprecated `SEARCH_INTEREST` positive type, which
  Google has removed from the UI.
- ✅ Local pre-flight for impossible operations (issue #116). The mutate
  batch is atomic, so one operation the account can never accept rejects
  every unrelated one with it — a stale declaration in one corner of the
  repo turns every other author's plan red. Those cases are knowable from
  the live state alone, so `plan` now decides before sending: a create or
  update on the VIDEO channel blocks the plan (nothing is submitted,
  exit `1`), and a removal of a labeled VIDEO resource the file no longer
  declares is skipped with a warning so the rest of the batch goes
  through. A rejected batch now separates operations that drew their own
  error from those that were merely collateral, which is what made a red
  plan on an untouched campaign so hard to diagnose.
  **Deferred:** impossible operations outside the video channel, which
  need a general notion of "the API refuses this here" rather than one
  channel check.
- ✅ Declarable adopt-only (issue #115). A `resource` may carry
  `lifecycle { create = false }`, so "this campaign is adopted, not
  created" is a property of the file instead of a warning in PR prose.
  A miss on the content match now stops the plan with the key it looked
  for (`by name "…"`) rather than degrading into a create that Google
  rejects and that sinks the whole atomic batch. Channel-agnostic: any
  resource except the three criterion types, where the API creates
  freely and one declaration can fan out into several live members.
  **Deferred:** `adopt_match` (the match key is label-first-then-name
  for every type, so the attribute would have one legal value), and the
  rest of a Terraform-shaped `lifecycle` — `prevent_destroy`,
  `ignore_changes` — which want a real requirement behind them.
- ✅ Visible schema gaps (issue #111). `bidsmith drift` reports the
  surface `plan` is silent about: for every resource the plan makes a
  claim about, which settable fields bidsmith models, which it does not,
  and — the part that matters — which of the unmodelled ones actually
  carry a value on the account. `plan` gained a one-line footer saying
  what `unchanged` covers, so the gap is visible even to someone who
  never runs `drift`. This is the general form of #109 / #110: both were
  found by accident, and the whole point is that the next one shouldn't
  have to be. Sources are all live — `GoogleAdsFieldService` for what
  exists, the public discovery document for what a mutate could write,
  `live_state::QUERIES` for what bidsmith reads — because a bundled
  catalog would go stale in the under-reporting direction.
  **Deferred:** a `--type` scope flag (the whole audit is one command
  today, and the per-resource sections already read independently);
  folding the count into the plan footer itself, which would make every
  `plan` pay for the catalog fetch; and reconciling `drift` findings back
  into `.bid` files, which is the schema work each field needs anyway.
  **Unverified live:** the value scan has not been run against a real
  account — the chunk-bisect path that isolates a field the API refuses
  in a given `SELECT` combination is exercised by construction, not by
  observation.
- ✅ Ad group bid fields (issue #109). `google_ads_ad_group` models all
  eight settable `AdGroup` bid fields, not just `cpc_bid_micros`, so the
  amount a CPV or CPM campaign actually bids is declarable and — more to
  the point — *diffable*. The re-raise corrected #104's premise on the
  record: an in-stream ad group's max CPV is **not** `cpc_bid_micros`,
  which sits at zero while `target_cpv_micros` governs the auction, so
  `plan` had been reporting those ad groups clean against a bid the repo
  never saw. An omitted bid field is unmanaged (Google returns the
  unused ones zeroed), and `export` skips zero-valued bids so a Search
  ad group doesn't render five bids it doesn't use.
  **Verified live** (`validateOnly` plans against 6571974784): the VIDEO
  channel refuses ad-group mutates the same way it refuses campaign ones
  — a bid update on a TARGET_CPV in-stream ad group returns
  `OPERATION_NOT_PERMITTED_FOR_CONTEXT`, a bare `status` update on it
  returns `MUTATE_NOT_ALLOWED`, and the same bid update against a
  DEMAND_GEN ad group is accepted. On a DEMAND_GEN ad group
  `target_cpv_micros`, `target_cpm_micros`, `cpm_bid_micros` and
  `target_cpa_micros` are all accepted, so none of them is output-only;
  `cpv_bid_micros` is refused there with
  `OPERATION_NOT_PERMITTED_FOR_CONTEXT` — it is the manual-CPV bid and
  that strategy only exists on the read-only VIDEO channel, so it is
  modelled for reading and drift, not for writing.
  `percent_cpc_bid_micros` and `fixed_cpm_micros` remain unprobed: the
  account's daily API quota ran out first (a full-account `plan` spends
  one operation per resource, so probes are expensive — probe against a
  small test account, not this one). So on video these
  fields buy drift *visibility*, not declarative bidding; Demand Gen is
  where they are writable. `diff` now warns before the batch goes out on
  a video **ad group** create or update, not just a video campaign —
  previously an ad-group-only edit sailed into the atomic batch and took
  every unrelated operation down with it.
  **Deferred:** `google_ads_ad_group_criterion`'s `cpv_bid_micros` /
  `cpm_bid_micros`. bidsmith's criterion resource is KEYWORD-only
  (`live_state.rs` filters on `type = KEYWORD`) and keywords bid by CPC,
  so those fields would be unusable where they'd be declarable. Revisit
  with non-keyword criteria.
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

> Status: both shipped. `pull` (signature `run(output, verbose)`) and
> the live round-trip e2e test (`tests/e2e.rs`, gated on the `e2e` Cargo
> feature + `BIDSMITH_E2E_CUSTOMER_ID`) are in the tree; the design
> notes below are retained as the rationale. The `--customer-id` /
> `--campaign-id` scoping flags in the original sketch were skipped for
> v1 — the verb dumps everything for the customer in env, matching what
> `plan --read-live` reads. Add scoping flags when there's a concrete
> need.

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
isn't set, the test is skipped (not failed). Shipped as the Cargo
feature path (`cargo test --features e2e`, `tests/e2e.rs`); the
alternative hidden CLI verb (`bidsmith self-test --live`, friendlier
for ad-hoc runs without a Rust toolchain) was considered and not built
— add it later if a no-toolchain run is wanted.

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
