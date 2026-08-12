# Bidsmith — Decisions

Settled choices and a snapshot of what currently exists. The
forward-looking plan lives in [ROADMAP.md](ROADMAP.md).

## Vision

Declarative, AI-friendly tooling for Google Ads campaigns. Think
**Terraform for Google Ads**: HCL2 config files, modules, validate /
plan / apply / refresh. The engine is deterministic; AI sits **on top**
— authoring `.bid` files, reviewing PRs, recommending optimizations.
Distribution: a Rust-compiled CLI binary (~4 MB release once the REST
+ TLS stack is linked in), GitHub-native workflows for collaboration
and continuous tuning.

The seed is `/Users/chrmod/Projects/github.com/chrmod/rezolutnie.com/ads/`
— a Python module that already encodes the right shape (dry-run by
default, atomic mutates, idempotent-by-name, dataclass data,
generator-based content). Bidsmith generalizes it: any Google Ads
resource type, any file layout, modules, schema validation.

## Locked decisions

- **Language**: Rust. Distribution = single binary via `cargo build
  --release`. HCL2 parsing via `hcl-edit` (preserves source spans).
  Diagnostics via `miette` (compiler-style error rendering with source
  snippets). CLI via `clap`.
- **Config syntax**: HCL2 — modules, `for_each`, interpolation. YAML
  rejected (loses module system, forces Jinja-style hacks).
- **File extension**: `.bid` (provisional — easy to rename).
- **Resource model**: mirror Google Ads API entities 1:1
  (`google_ads_campaign`, `google_ads_campaign_budget`,
  `google_ads_ad_group`, …). No opinionated abstractions like
  "search-campaign-per-city" baked into core — that's a community module.
- **No `.tfstate`**: state lives on Google Ads as labels on managed
  resources. The project-folder `.bidsmith/cache/` is a *read cache*
  (last SearchStream batches + last OAuth access token), not a state
  file — bidsmith never trusts it as ground truth. It exists purely to
  reduce API/quota usage on tight authoring loops. Default TTL 15 min
  for live state; access tokens reuse their server-issued expiry. Bust
  via `--refresh-state` (plan/apply), or `BIDSMITH_NO_CACHE=1` for a
  full bypass. Cache directory is gitignored; token file is written
  mode `0600`. A successful `apply` invalidates the live-state cache
  so the next `plan` starts from fresh data.
- **`locals` block**: HCL2-style top-level `locals { ... }` blocks declare
  reusable constants. References use `local.<name>` and resolve at
  validate / plan / apply time — the value is substituted for type
  checking and for the API mutate body. Scoping mirrors resource
  scoping: same-module first, then global with an ambiguity guard.
  Chains (`local.a = local.b = 5`) and cycle detection are supported.
  Resolves the rezolutnie use case where every per-city `.bid` was
  repeating bid micros, proximity radius, budget, and language
  constant.
- **List / map locals** (issue #39): a `local` value is an arbitrary HCL
  expression, so it holds lists and maps as readily as scalars. A
  `local.<name>` that resolves to a list is usable anywhere a list
  attribute is expected — RSA `headlines` / `descriptions` (the mixed
  string / `{ text, pin }` form included), `final_urls`, inline
  `languages` / `locations`, and the compact `keywords` / `negative_keywords`
  `texts` / `match_types` lists. Because the compact keyword block already
  fans a `texts` list out into one criterion per `(text, match_type)`
  pair, `keywords { texts = local.<theme> }` is the "repeated block from a
  list" form — no `dynamic` / `for_each` block-expansion construct is
  needed. Maps are usable as whole values (a `module` `for_each` already
  takes `for_each = local.<variants>`); map **indexing**
  (`local.headlines["ublock"]`) is deferred with the rest of the
  expression engine. The de-duplication crosses files via the existing
  same-module-first-then-global-fallback resolution: declare a shared list
  in one file (a conventional `shared.bid`) and reference `local.<name>`
  from every other file — one declaration, account-wide reuse, and editing
  it shows as an in-place update fanning out to every referencing
  resource. Resolution happens at load time, so `plan` / `apply` are
  unchanged and `export` / `refresh` keep emitting literals (folding into
  locals stays an edit-time operation). The validator resolves the binding
  before type-checking (a `local` that resolves to a scalar in a list slot
  fails with the normal "expected list…" error at the use site); the RSA
  min-headline / max-length lints resolve list references too, so a
  `headlines = local.<set>` no longer mis-reports as zero. `variable`
  blocks stay scalar-only (`string` / `number` / `bool`) — a CLI-supplied
  list input has no obvious `--var` syntax, and the measured duplication
  is list **data**, which belongs in `locals`.
- **String interpolation** (issue #84): HCL template expressions
  (`"${local.utm_base}-rsa_a"`) evaluate to strings anywhere a string is
  accepted — resource names, headlines, paths, URLs — including inside
  `locals`, so one local can be built from another
  (`utm_base = "${local.landing}?utm_campaign=${local.tag}"`). This is
  the first slice of the deferred expression engine, implemented as an
  evaluator (`src/eval.rs`) that subsumes the existing `local.`/`var.`
  chain resolution: an interpolated expression may be any chain that
  resolves to a string, number, or bool (numbers and bools stringify,
  Terraform-style); resource references, lists, and objects inside
  `${…}` are validate-time errors, as are template directives
  (`%{ if … }`). Evaluation happens before schema validation and before
  lints, so enum checks and the RSA length caps run on the *rendered*
  string (the issue's UTM-slug case: a headline that only exceeds 30
  chars once the brand splices in is flagged). Evaluation is load-time
  only — `plan` / `apply` see rendered literals and `export` / `refresh`
  keep emitting literals, exactly like list locals (#39). Cycle
  detection is DFS-based so `"${local.x} ${local.x}"` is legal while
  mutually-recursive templates error. `variable` defaults stay
  literal-only. A `format()` function was considered and skipped:
  interpolation subsumes it.
- **`concat()` for lists** (issue #85): `concat(list, list, …)` merges
  lists in argument order, evaluated in `locals` and directly in list
  attributes, so a partially-shared list — per-ad headlines followed by
  a shared brand tail — dedupes without copy-pasting the tail into
  every RSA. Rides the same evaluator as interpolation (`src/eval.rs`);
  arguments may be inline lists, `local.`/`var.` chains, or nested
  `concat()` calls, and element types stay mixed (bare strings and
  `{ text, pin }` objects in one result). List-level validation and the
  RSA lints (min 3 headlines / max lengths, duplicate detection) run on
  the post-concat result, per the issue's requirement. A non-list
  argument and HCL's `...` argument expansion are validate-time errors,
  as is any function other than `concat` ("unknown function 'x';
  supported functions: concat") — the function table grows one entry at
  a time as real needs land, not speculatively. The spread-form
  alternative (`[local.a..., "extra"]`) was skipped: `concat` matches
  Terraform muscle memory, which is what agents and Terraform-literate
  users reach for first.
- **`ad_template` block** (issue #40): list locals fold an RSA's
  `headlines` / `descriptions`, but the duplication that remains is the
  *whole ad body* run in every ad group of a campaign (measured: 142 RSAs,
  44 distinct bodies, the top body repeated 20× across files). A top-level
  `ad_template "name" { … }` block declares an `ad {}` body once — same
  shape as the inline `ad` block (`final_urls`, `responsive_search_ad`,
  paths) — and a `google_ads_ad_group_ad` attaches it with
  `template = ad_template.<name>` instead of an inline `ad {}` block.
  **Exactly one of the two is required** (validate-time XOR). Chosen over
  the alternative "one ad resource fanned out to N ad groups via
  `ad_groups = [...]`" (the issue's Option B) precisely because the
  per-ad-group `google_ads_ad_group_ad` resources keep their existing
  addresses: adopting a template is a pure source refactor that `plan`
  sees as a no-op, whereas re-addressing live ads into a fanned-out
  resource would plan as delete+create for every folded ad (address is
  identity until the Phase 3 v2 label lands). Expansion is pure
  load-time substitution — `import` resolves the reference to the
  template's body and builds the same `ad_group_ad` mutate it would for an
  inline body, so plan / apply are byte-identical to the unfolded form and
  there is no new API surface (RSAs are per-ad-group entities; there is no
  server-side shared-ad object). Templates resolve like resources / locals
  — same-module first, then a single global declaration with an ambiguity
  guard — so one template in a shared file serves campaigns across files
  (the 20× body spans files). The template's RSA is linted once at its
  declaration (`ad_template.<name>`), not per use site.
- **`ad_template` per-instance overrides** (issue #58): a
  `google_ads_ad_group_ad` that attaches a `template` may also set
  `final_urls`, `path1`, and `path2` directly on the resource; each
  overrides the template body's value while every unset field inherits.
  Overrides apply at import time — the merged body is the same mutate as
  writing it inline, so `plan` is unchanged and no live ad is
  re-addressed. This collapses the near-duplicate templates that existed
  only to vary the landing URL (the measured Ghostery case: 19 templates,
  5 distinct `final_urls`). `final_urls` is therefore now **optional on
  `ad_template`** (a URL-agnostic template lets every reference supply its
  own) but stays **required on an inline `ad {}` block**; a reference to a
  template that declares no `final_urls` and supplies no override fails
  `validate` at the use site. Overrides are rejected alongside an inline
  `ad {}` block (set the fields inside it instead). The merged path
  overrides are linted like any RSA path; headline/description overrides
  stay deferred (the measured duplication is URLs, not copy). The Option B
  fan-out form remains a deferred follow-up. `variable` blocks
  stay scalar; this is a block-level reuse primitive, not a value one.
- **Folding emitter** (issue #57): `refresh` / `export` recognize repeated
  structure and emit the compact constructs instead of re-exploding the
  tree on every pull. Three folds, all computed in `plan_fold` and applied
  during rendering: (1) **ad bodies → `ad_template`** — RSA-bearing ads
  sharing the same creative (ad `name` + headlines + descriptions, keyed
  *excluding* `final_urls` / `path1` / `path2`) collapse onto one
  top-level `ad_template`; bodies that differ only by those overridable
  fields collapse onto one URL-agnostic template plus per-instance
  overrides (#58), with a field templated when uniform across the group
  and overridden per-instance when it varies. (2) **repeated RSA arrays →
  `locals`** — a `headlines` / `descriptions` array used by ≥ 2 emission
  sites (templates + still-inline ads) is lifted into a `locals` block and
  referenced as `local.<name>`. (3) **repeated campaign negative lists →
  `locals`** — a campaign's negative-keyword text list, when its members
  share one match_type and the same list backs ≥ 2 campaigns, is lifted to
  a `local` referenced via the compact
  `negative_keywords { texts = local.<name>, match_type }` form.
  **Deliberately *not* a `google_ads_shared_set`** (the issue's literal
  ask): live negatives are per-campaign criteria, so emitting a SharedSet
  would plan as create-set + attach + destroy-the-criteria — a real
  migration, not the zero-drift representation change a `refresh` must be.
  Folding is purely source-level: every construct expands back to the
  identical mutate at import time (templates #40/#58, list `locals` #39,
  compact negatives), so the folded tree round-trips through
  `validate` / `plan` exactly like the verbose one. This is the property
  the issue calls out, enforced offline by `fold_roundtrips_to_verbose`
  (render → import → re-render the *unfolded* form, assert byte-identical
  to the input's). Folding is the default for every `render` path; a
  test-only `render_inner(input, fold=false)` keeps the verbose form for
  the round-trip oracle. Names are content-derived and deterministic
  (`<first-headline>_rsa`, `<first-item>_headlines` / `_descriptions` /
  `_negatives`), deduped in a dedicated namespace. **Deferred:** ad-group
  negative lists (same mechanism, lower measured volume) and emitting an
  actual `shared_set` (waits on the Phase 3 v2 labels that make adopting
  one a no-op against live).
- **`variable` block**: HCL2-style top-level
  `variable "name" { type = …, default = …, description = … }` blocks
  declare typed inputs that the same `.bid` can pivot on without
  edits. References use `var.<name>`; values come from `--var name=…`
  CLI flags first, then `BIDSMITH_VAR_<name>` env vars, then the
  block's `default`. Declared types are `string`, `number`, or `bool`
  (bare identifiers, not strings); defaults are validated against the
  declared type at parse time, CLI/env strings are parsed to the
  declared type, and a variable with no value (no input, no default)
  is a validate-time error. Scoping and chain resolution match locals,
  and chains can interleave (`local.x = var.y`, `var.x` referenced
  inside a `locals` block, etc.).
- **`module` block**: HCL2-style top-level
  `module "instance" { source = "./file.bid", ...inputs }` blocks
  instantiate a parameterized `.bid` source. `source` is a path to a
  single `.bid` file, relative to the calling file's directory; all
  other attributes are passed as inputs to that file's `variable`
  blocks (literals or top-level `local.*` / `var.*` references that
  resolve to literals). Each instance is an isolation boundary —
  `--var` and `BIDSMITH_VAR_<name>` do not flow into the module; the
  module's own locals/variables are private; resources inside cannot
  cross-reference the parent or sibling instances. Resource addresses
  become `<instance>.<type>.<name>`, replacing the file stem so two
  module instances of the same source file don't collide. A file used
  as a `module` source is a *template*, not a root file: it is loaded
  only inside its instance scope(s) and excluded from the top-level
  scope, so a template whose `variable`s have no defaults doesn't fail
  top-level validation when it sits in a directory the loader walks.
  Limitations: single-file local sources only, no outputs, no directory
  or GitHub sources, no nested modules (a module source containing its
  own `module` block fails fast). The remaining pieces (`output`,
  directory + GitHub sources) are tracked under "Open decisions" as
  "Module composition v2."
- **`module` `for_each`**: a `module` block can declare
  `for_each = { <key> = { <inputs> }, … }` to instantiate its source
  N times — one instance per map entry — so all N variants coexist in
  one desired state (the cross-file counterpart to `variable` + `--var`,
  which pivots one variant per run). The `for_each` value is an object
  literal, or a `local.<name>` that resolves to one (`var.<name>` is
  scalar-only and can't be a map — feed variant tables from a `local`).
  Each entry's value is an object mapping the source's `variable` names
  to scalar literals; per instance the inputs are the block's shared
  top-level attributes merged with that entry's object, entry winning on
  conflict. Each instance is a normal module scope keyed by
  `<label>.<key>`, so addresses are `<label>.<key>.<type>.<name>`
  (collision-free as long as label + key are unique). There is no
  `each.key` / `each.value` interpolation on *module* blocks — the map's
  values *are* the inputs, which keeps the scalar `variable` machinery
  unchanged and needs no object variable type. (`each.*` exists on
  *resource* `for_each`, issue #86, where there are no named inputs to
  carry the entry.) An
  empty `for_each` map is an error (almost always a mistake, and once
  removal-detection lands an empty table would silently destroy the
  instances). Like all addresses, instance addresses are source-only;
  the planner matches live resources by content, so converting N
  hand-written clone files into one `for_each` template is live-neutral
  (no campaign recreation) even though the addresses change.
- **`for_each` on resource blocks** (issue #86): a `resource` block can
  declare `for_each = ["MOBILE", "TABLET"]` (a list of strings, possibly
  via `local.`) or `for_each = { key = <expr>, … }` (a map whose values
  may be resource references) to fan out into one instance per entry.
  `each.key` / `each.value` substitute anywhere inside the block —
  attributes, nested blocks, list items, `${…}` interpolations,
  `concat()` arguments. Instance addresses key the label:
  `google_ads_campaign_criterion.t_devices["MOBILE"]`. This is the
  child-resource counterpart to `module` `for_each` (which clones whole
  files): it collapses the mandated per-campaign device-exclusion pair
  and N-sitelink `campaign_asset` attachments into one block each.
  Implemented as a **load-time expansion pass** (`src/expand.rs`) shared
  by `validate` / `plan` / `apply` / `refresh` and the lints: each
  instance becomes an ordinary block with `each.*` replaced by the
  entry's literal/reference, so the schema validator, importer, diff
  engine, and mutate builder are untouched — exactly the
  substitution-not-new-machinery pattern `ad_template` set. `fmt` / `mv`
  operate on raw source and never see expanded blocks. List entries must
  be strings (key == value, Terraform set semantics); map values are
  spliced verbatim so `asset = each.value` resolves through normal
  reference validation. Empty tables and duplicate keys are errors
  (same policy as module `for_each`); `each.<anything else>` is an
  error pointing at the offending expression. Adopting `for_each` for
  existing hand-written resources is live-neutral for content-matched
  types (criteria, assets, keywords); labelable types re-adopt via
  content fallback and show visible `~ adopt (label)` rows as their
  `bidsmith:address` label moves to the keyed form. Referencing a
  keyed instance from another resource
  (`google_ads_campaign.t["a"].id`) is not supported yet — fan out
  parents with `module` `for_each` instead. `refresh --in-place`
  reports (rather than silently drops) drift on generated instances,
  since there is no source block to patch.
- **`defaults` block** (issue #87): shared campaign boilerplate is
  factored with a top-level, type-scoped
  `defaults "google_ads_campaign" { … }` block — attribute and
  nested-block defaults merged into every resource of that type that
  doesn't set them itself. A resource's own attribute always wins; a
  nested block (`manual_cpc`, `network_settings`) overrides
  **wholesale**, never deep-merged, so override behavior stays
  predictable. One defaults block per resource type per scope
  (duplicate → error citing both files); module scopes don't inherit
  the caller's defaults (modules stay isolation boundaries). The
  defaults body is schema-validated at its declaration (correct spans,
  no required-attr enforcement — it legitimately provides a subset),
  required attributes on resources count defaults as present, and the
  merge happens at import time, so `plan` / `apply` see the merged
  resource and adopting defaults over an existing account is a plan
  no-op — pure sugar over existing addresses, the property that made
  option B attractive in the issue. The inline-targeting
  one-source-of-truth guard sees defaults too (a defaults `locations`
  plus an explicit positive geo criterion is still a conflict).
  `defaults` cannot provide an ad body (`ad` / `template` on
  `google_ads_ad_group_ad`) — that's `ad_template`'s job and would
  break the per-resource ad/template XOR. **This settles issue #87 as
  option B + issue #86**: the campaign shell dedupes via `defaults`,
  and the per-campaign device-criteria trio (separate resources, which
  a defaults block can't express) via resource `for_each`. Option A
  (module outputs / cross-module references) and option C
  (reference-typed module inputs) stay deferred under "Module
  composition v2" — B + #86 eliminate the measured duplication with a
  fraction of the machinery, and defaults adoption churns nothing live.
- **Files are modules**: each `.bid` file's basename (slugified file
  stem) is its implicit module name. Resource addresses are
  `<module>.<type>.<name>`. Two files in one directory can each declare
  `google_ads_campaign_criterion.broad_wikipedia` without conflict —
  they live in different modules. References resolve same-module first
  and fall back to a global search; cross-module matches are accepted
  if exactly one resource matches and rejected as `ambiguous reference`
  otherwise. Display strips the module prefix when every diff line is
  in the same module so single-file projects keep their bare
  `<type>.<name>` UX. This is the foundation for explicit `module`
  blocks in Phase 5.
- **Directory loading is recursive**: pointing a command at a
  directory loads every `.bid` file in the tree at any depth (not just
  the top level), merging them all into one namespace so cross-folder
  references resolve. The walk skips hidden entries (`.git`,
  `.bidsmith`) and `node_modules` / `target`. A big account can be
  split into subfolders (`campaigns/search/`, `campaigns/video/`, …)
  for navigation; subfolders carry no semantics. `validate`, `plan`,
  `apply`, and `fmt` share one `collect_bid_files` walker.
- **AI is outside the engine**: skills/agents author and review; engine
  is deterministic. Engine behavior must not depend on a model version.
- **Agent docs split — facts in the binary, behavior in the skill**:
  version-coupled facts (verbs, flags, usage examples) live in the
  binary itself — clap `--help` / `<verb> --help` with `after_help`
  examples, `bidsmith schema` for resource shapes — so a `brew upgrade`
  updates docs and behavior atomically and they cannot drift. The
  Claude Code skill (`skills/bidsmith/SKILL.md`, shipped via
  `.claude-plugin/`) stays a thin, version-agnostic layer owning only
  what the binary can't: triggering, installation, safety conventions
  (plan-before-apply, gap-reporting protocol), with an explicit
  tie-breaker that the binary wins when the two disagree. No skill
  self-update machinery — the skill is designed to tolerate being
  stale rather than trying never to be.
- **Plan = live validate**: use Google Ads API's `validate_only` flag
  for free server-side validation (auth, references, policy, length).
- **Apply shows the plan first, then prompts**: terraform-shaped flow.
  `apply` always runs the validateOnly diff first, displays it, and
  asks for a literal `yes` before mutating. `--auto-approve` skips the
  prompt (and is required when stdin is not a TTY). Never auto-apply
  silently.
- **Module distribution**: Git refs
  (`source = "github.com/org/repo//path?ref=v1"`). No central registry
  in v1.
- **Single platform first**: Google Ads only. Meta / LinkedIn after a
  second installer exists.
- **`export` input shape**: keep the flat bidsmith JSON as the canonical
  input. Real Google Ads SearchStream dumps are handled by an internal
  adapter exposed via `bidsmith export --from-gads-search-response
  <PATH>` (no separate `adapt` verb). Rationale: one-step ergonomics,
  the renderer keeps a single output, and the adapter module can be
  promoted to a standalone verb later if needed.
- **Google Ads transport**: REST over `reqwest::blocking`, not gRPC.
  Chosen for the early-iteration debugging UX (curl-able endpoints,
  human-readable JSON wire format, no proto compile step). Endpoint
  version pinned via `BIDSMITH_API_VERSION` env var (default `v22`);
  retired-version 404s are detected and surfaced with an actionable
  hint. tonic-build / `google-ads-rs` remain available as future
  swaps if a specific gRPC-only RPC is needed.
- **Auth**: every credential resolves env var → `~/.bidsmith/credentials.toml`
  → built-in default, evaluated per value, so the env-var setup the
  rezolutnie `ads/` scripts and CI use is byte-for-byte unchanged (env
  always wins) while newcomers get a managed file. The six values are the
  same ones (`GOOGLE_ADS_DEVELOPER_TOKEN`, `GOOGLE_ADS_CLIENT_ID`,
  `GOOGLE_ADS_CLIENT_SECRET`, `GOOGLE_ADS_REFRESH_TOKEN`,
  `GOOGLE_ADS_CUSTOMER_ID`, `GOOGLE_ADS_LOGIN_CUSTOMER_ID`).
  `bidsmith auth login` runs a browser-based OAuth loopback + PKCE
  authorization-code flow to mint the refresh token, then writes the
  credentials file (mode `0600`, in the user's home — never the project
  tree, never Git). It supports **both** a bundled bidsmith OAuth
  "Desktop app" client (injected at release-build time via
  `option_env!("BIDSMITH_DEFAULT_CLIENT_ID"/..._SECRET)`, gated on
  Google's OAuth verification — absent and harmless in ordinary builds)
  **and** a bring-your-own client (`--client-id`/`--client-secret` or
  env). A stored refresh token is pinned to the client that minted it; a
  later client mismatch is reported rather than surfacing as an opaque
  `invalid_grant`. bidsmith still exchanges the refresh token for an
  access token on every run, caching only that short-lived access token
  in `.bidsmith/cache/` (mode `0600`) — the **refresh and developer
  tokens** live solely in `~/.bidsmith/credentials.toml`.
  `--customer-id` / `--login-customer-id` flags still override.
- **Project config (`bidsmith.toml`)**: a committable, per-project file
  at the project root (found by searching upward from the working
  directory) supplies the *routing* axis — `customer_id`,
  `login_customer_id`, and optionally `developer_token`. It sits in the
  resolver between env and the global credentials file. Full target
  precedence: env var → `bidsmith.toml` → `.bid` provider block → global
  `~/.bidsmith/credentials.toml`. This is how multi-account works without
  env juggling: one global sign-in (the refresh token spans the user's
  whole Google identity), and each client folder declares its own
  account/MCC. Only the ids are non-secret and meant to be committed; a
  developer token placed here should be gitignored. Consequently the
  provider block's `customer_id` is now **optional** — `.bid` files can be
  account-agnostic and take their target from `bidsmith.toml`/env. The
  resolved target is authoritative end-to-end (the importer merges the
  precedence and the live client is built from that value via
  `Client::for_target`), removing the prior footgun where the provider
  block's `customer_id` was silently overwritten by the env for live runs.
- **Plan = dry-run diff against live**: `plan` always fetches live
  state via `googleAds:searchStream`, matches by name with parent
  cascade, computes scalar field-level drift, and sends one
  `googleAds:mutate` batch with `validateOnly=true` containing
  CREATE+UPDATE ops for only the diffs.
- **Apply = plan + prompt + real mutate**. `apply` runs the same
  prepare stage as `plan` (parse → import → fetch live → diff), prints
  the diff with validateOnly outcomes, then either prompts for `yes`
  on a TTY or honours `--auto-approve`. Only the second POST sets
  `validateOnly=false`. One atomic batch, ordered removes-first (child
  before parent) then creates/updates in dependency order (budgets →
  campaigns → ad_groups → ads → criteria) then label writes. Removes
  lead because the API checks per-parent caps against the running
  state, so a destroy+create replacement (e.g. rewriting RSA copy in an
  ad group already at the 3-enabled cap) must destroy the old body
  before creating the new one or the batch transiently exceeds the cap
  and is atomically rejected (issue #74). If validateOnly rejects
  anything, the real mutate is skipped.
- **Member removal is planned (`- destroy`); whole-resource removal
  still waits on labels**: a `negative_keyword` / `keyword` block
  removed from a resource that otherwise survives now plans as a
  `- destroy` (criterion `remove` at the API level), so cleanup and
  dedupe migrations (e.g. moving ad-group negatives into a shared set)
  no longer silently half-apply. The pruning is parent-scoped — it only
  removes orphaned members of an `ad_group_criterion` /
  `campaign_criterion` / `shared_set` whose parent is still declared,
  and only inside a `(parent, category)` bidsmith owns. Category
  partitions keyword polarity (positive/negative) and
  campaign-criterion shape (keyword/location/language/proximity), so
  declaring negatives never deletes positives a user manages in the UI,
  and declaring one axis never prunes another. Ownership of a category
  is claimed two ways: the file declares ≥1 member of it, or the live
  parent carries a `bidsmith:owns=<category>` label written by a
  previous apply (issue #88: without the persisted claim, removing the
  *last* declared member-resource of a category closed the gate and
  silently orphaned the live members). `apply` writes the claim
  association when a category gains its first declared member and
  releases it — in the same batch as the member destroys — when the
  category's last declared member goes away; `plan` shows the work as
  `~ claim (+negative keywords)` / `~ claim (-negative keywords)` rows.
  `shared_set` membership keeps the ≥1-declared-member gate and the
  last-member gap: the API has no shared-set label association to hang
  a claim on, and matching alone can't prove ownership (sets match by
  bare name, so an empty declared set adopting a UI-curated list must
  not empty it). In practice removing a set's last member block means
  removing the set. Destroys are gated by the normal `apply` prompt
  (and `--auto-approve`), not a separate `--allow-destroy` flag.
  Dropping an *entire* resource from desired state now also destroys it
  live for the labelable types (campaign / ad_group / ad_group_ad) via
  the `bidsmith:address` identity labels — see **Identity labels (Phase
  3 v2)** below. An unlabeled live resource (UI-created, never managed
  by bidsmith) is still never destroyed, and live criteria in a category
  bidsmith never claimed are still never destroyed. A live resource
  already in `REMOVED` status is never re-flagged for destroy: the API
  forbids mutating a removed resource (`Removed ads may not be
  modified`), and removed resources keep their `bidsmith:address` label,
  so a removal that succeeded would otherwise re-plan its own destroy on
  every subsequent plan and — one atomic batch — sink every unrelated op
  (issue #91, same failure family as #82/#88). The same guard skips an
  `ad_group_ad` orphaned under an ad group that is gone from live state:
  removing an ad group leaves its ads addressable but un-mutatable, so
  the doomed per-ad destroy is dropped and the parent removal stands.
- **Rename = `bidsmith mv`, source-only**: renaming a resource's
  address (the `<name>` in `resource "<type>" "<name>"`) is a pure
  source rewrite — `mv` renames the block label and every reference
  that resolves to it, across all files, format-preserving. There is
  **no live mutate**: today the planner identifies live resources by
  content (campaign name, keyword text, geo/lang constant, …), not by
  address, so an address rename is invisible to Google Ads — it never
  becomes a delete+create, so performance history, learning state, and
  ad-review status survive. This is what makes cleaning up
  refresh-minted names (the `_2`/`_7`/`_19` dedupe suffixes) safe.
  Chosen over Terraform-style `moved {}` blocks consumed at plan/apply:
  a `moved` block's whole job is to rewrite the *stored* identity, and
  bidsmith's identity is the (not-yet-implemented) `bidsmith:address`
  label (Phase 3 v2). Until that label is the matching key, a `moved`
  block would have nothing live to act on, so it's deferred — `mv` is
  the complete mechanism now. When labels land, a move gains a second
  half (rewrite the live `bidsmith:address` label) and `moved` blocks
  can be reconsidered as the GitOps-friendly, plan-visible form.
  File renames (which change a file's implicit module name) are
  likewise address-neutral against live state for the same reason; they
  only affect in-tree reference resolution.
- **License**: MPL-2.0 (Mozilla Public License 2.0). Weak, file-level
  copyleft — the license Terraform itself shipped under for its entire
  open-source life. Using bidsmith (CLI, CI, managing clients'
  accounts) carries zero obligations; only modifications to bidsmith's
  own source files, once distributed, must stay open. Chosen over AGPL
  (corporate-adoption chill, and no dual-licensing desk wanted) and
  over permissive MIT/Apache (MPL keeps improvements to the tool itself
  open). Supersedes the earlier `license = "MIT"` placeholder; full
  text in `LICENSE`, declared as `license = "MPL-2.0"` in `Cargo.toml`.
- **Inline campaign targeting (`languages` / `locations`)**: a
  `google_ads_campaign` can carry `languages = ["en"]` /
  `locations = ["US"]` list attributes instead of one boilerplate
  positive `google_ads_campaign_criterion` per language/geo. Each entry
  expands at import time to one positive criterion; the planner already
  matches campaign criteria by their resolved constant (not by address),
  so converting explicit `location {}` / `language {}` criteria to the
  inline form — or adopting targets already live — is drift-free.
  Resolution ships in the binary (`src/targeting.rs`): country geo
  constants follow Google's stable `geoTargetConstants/(2000 + ISO-3166-1
  numeric)` convention (US → `…/2840`), so the full alpha-2 country table
  is generated; languages are a hand-curated code→id table for the major
  languages. Codes are canonical; the raw `geoTargetConstants/NNNN` /
  `languageConstants/NNNN` strings are accepted in the same list for
  cities, regions, and uncommon languages. **One source of truth per
  axis**: a campaign that sets `locations` *and* has an explicit positive
  location criterion pointing at it fails `validate` (negatives,
  proximity, and non-default statuses stay explicit). `refresh` / `export`
  emit the inline form by default for plain positive targeting, which
  retires the collision-suffixed criterion addresses for the common case
  (issue #37).
- **Schema defaults** (issue #38): optional attributes can carry the
  Google Ads API's effective create-default in the schema
  (`AttributeSchema::default`). An omitted attribute that has a default is
  **managed at that default** — *not* unmanaged: `plan` diffs the live
  value against the default (a UI flip back to `ACCELERATED` surfaces as
  drift), and `refresh` / `export` / `fmt --minimal` stop emitting the
  attribute once the live value equals the default. The fill happens once,
  on both the declared and the live state, right before diffing
  (`ExportInput::apply_schema_defaults` in `plan::build_prepared`), so a
  None→default fill never masks a real value. This generalizes the
  per-criterion `negative = false` round-trip patch (issue #15) into one
  rule. Defaults are **pinned in the schema, not inferred from proto zero
  values** — the API's create-default differs (campaign `status`: proto
  `UNSPECIFIED`, server creates `ENABLED`). Defaults set so far:
  `status = "ENABLED"` (campaign / ad_group / ad_group_ad /
  ad_group_criterion / campaign_criterion / conversion_action /
  customer_asset / shared_set / campaign_shared_set), `negative = false`
  (ad_group_criterion / campaign_criterion), budget
  `delivery_method = "STANDARD"` and `explicitly_shared = false`.
  Context-dependent defaults are deliberately **not** pinned: ad_group
  `type` and the `network_settings` booleans vary by channel, so a flat
  schema default would be wrong. `contains_eu_political_advertising`
  defaults to `DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING` but is flagged
  `always_emit` — omission still enforces the default, but the compliance
  declaration stays visible in every campaign file. Plain `fmt` is
  unchanged (it preserves explicit defaults); only `fmt --minimal` strips
  them, keeping a stable canonical form for hand-written files while
  refresh output is minimal by default. The old "missing `status` —
  defaults to ENABLED; set it explicitly" lint is **retired** — it
  encouraged exactly the per-line noise this closes.
- **Identity labels (Phase 3 v2)**: bidsmith writes a Google Ads label
  named `bidsmith:address=<address>` on every resource it creates or
  adopts, for the labelable types — **campaign, ad_group, ad_group_ad**.
  (The API has no label association for budgets, campaign criteria,
  shared sets, or assets; **individual keywords are deliberately left
  unlabeled** — see below.) The label is bidsmith's identity key, read
  back via a `label` query plus the four `*_label` association queries
  in `live_state`.
  - **Match**: campaigns and ad groups resolve to the live resource
    carrying their `bidsmith:address` label first; failing that they
    fall back to content (name / campaign + name) to *adopt* an
    unlabeled live resource and write its label. So renaming a
    campaign's display **name** is an in-place update (not a
    create-plus-orphan), and `bidsmith mv` (which still only rewrites
    source) stays a no-op against live — the next `apply` reconciles the
    moved resource's label declaratively, which is why no live-mutate
    half was added to `mv` and `moved {}` blocks stay deferred.
  - **Ads** keep matching by **body** (an RSA's copy *is* its identity,
    and copy is creation-only — label-first would mask a copy edit), but
    gain a label so a replaced ad's predecessor is cleaned up rather than
    left to linger. **Keywords are not labeled**: their text + match_type
    is identity, per-keyword labels would be high volume (thousands of
    label ops), and the API outright forbids them on negative criteria
    (`CANNOT_ADD_LABEL_TO_NEGATIVE_CRITERION`), which is exactly where
    lifecycle tracking matters most. Their removal lifecycle is owned by
    the parent-scoped member pruning plus the per-category
    `bidsmith:owns=<category>` claim label on the **parent** (one
    association per campaign / ad group per claimed category — e.g.
    `bidsmith:owns=keyword_negative` — written on the first declared
    member, released with the last), which keeps last-member removals
    planning as destroys — see **Member removal** above.
  - **Removal**: a labeled live campaign / ad_group / ad that no longer
    appears in the `.bid` is destroyed (children cascade; removes are
    ordered child-first). A live resource with **no** bidsmith label is
    unmanaged (UI-created) and never touched. Destroys are gated by the
    normal `apply` `yes` prompt — same as member removal, **no** separate
    `--allow-destroy` flag.
  - **Writes**: `apply` emits a `Label` create (reusing an existing
    label by name — a duplicate name is an API error) plus the
    association op, wiring temp ids; a relabel also removes the stale
    association. First-run adoption of an already-matching resource
    surfaces as a visible `~ adopt (label)` row and counts as a pending
    change, so the label write is never silent.
  - **80-char cap**: Google Ads caps `label.name` at 80 chars and rejects
    a longer one with `Too long.`, sinking the whole atomic adoption batch.
    `address_label_payload` keeps the address verbatim when it fits and
    otherwise encodes it as a legible head plus a stable SHA-256 suffix
    that always fits. Matching, label reuse, and relabel all run in this
    payload space, so a hash-encoded long address still resolves to its
    live label instead of re-adopting every run.
  - Chosen over a single `bidsmith-managed` marker (the per-address label
    *is* the identity, not just an ownership flag) and over label-first
    matching for *all* types (which would mask ad copy edits and bloat
    keyword label volume).
- **GitOps CI scaffold (`init`)**: the `.bid` files are meant to live in
  a user-controlled GitHub repo, and `bidsmith init` writes the skeleton
  for that — a starter `campaigns.bid`, a `bidsmith.toml` routing file,
  `.gitignore`, README, and a `.github/workflows/bidsmith.yml` that runs
  `bidsmith plan --format markdown --detailed-exitcode` on pull requests
  (posting the diff as a sticky PR comment) and `bidsmith apply
  --auto-approve` on merge to `main`. The **pull-request merge is the
  approval gate** — this is the one sanctioned use of `--auto-approve`,
  because a human has already reviewed the rendered plan before merging —
  and Google Ads credentials live solely in Actions secrets, never on a
  laptop. Two CLI affordances back the flow, both on `plan`:
  `--format markdown` (a `Resource | Action | Result` table for the PR
  comment) and `--detailed-exitcode` (terraform-style: `0` = no changes,
  `2` = changes pending, `1` = error), so the workflow can post the plan
  and reserve a red check for genuine failures. CI installs bidsmith by
  downloading the linux release asset directly rather than via Homebrew —
  the tap formula's ad-hoc `codesign` step is macOS-only and would fail
  on a Linux runner. Templates live in `templates/init/`, `include_str!`'d
  into `src/commands/init.rs`; the offline CI checklist runs
  `init` → `validate` → `fmt --check` so the starter `.bid` can't drift
  from the schema or the formatter. Chosen over a remote/hosted runner
  for `apply`: keeping execution in the user's own GitHub Actions means
  the account stays under their control and there's no bidsmith-operated
  service holding live-mutation credentials.
- **YouTube video ads — reference the video, don't upload it**: a
  `google_ads_youtube_video_asset` records a video that is *already
  published on YouTube* by its `youtube_video_id`; a `video_responsive_ad`
  block inside an `ad {}` attaches that asset as the creative for a
  `VIDEO`-channel campaign's in-stream / bumper ad. bidsmith deliberately
  does **not** upload video files — that is the YouTube Data API's job (a
  separate system, separate OAuth scope, resumable binary upload), and the
  Google Ads API itself only ever *references* a video, it never hosts one.
  Everything downstream of the upload *is* managed: `apply` creates the
  asset via `AssetService` from the video id (+ optional title) and creates
  the `AdGroupAd` with a `VideoResponsiveAdInfo` / `DemandGenVideoResponsiveAdInfo`
  pointing at it, in one atomic batch (the asset op is emitted ahead of the
  ad so the temp resource name resolves). Assets are content-addressed, not
  labelled: a declared asset matches a live one by `youtube_video_id`, so
  re-running `plan` reuses the account's existing asset instead of piling up
  duplicates — the same rule the sitelink / callout / structured-snippet
  assets follow, and the reason no video asset ever plans as an update or a
  destroy (the API has no remove for `Asset`). The creative itself is
  create-only like an RSA body: `ad_body_key` hashes the video ref plus the
  copy, so editing a headline plans as a new ad + a destroy of the old one.
  The corollary is that an `ad {}` with **no** creative block now means "the
  URLs are mine, the creative is not": it matches any live ad in the group
  with the same `final_urls`, creative and all. That is the shape a `refresh`
  rendered for a UI-built video ad before the creative was readable, and
  without the carve-out those adopted ads would plan as a destroy plus a
  create of an ad with no creative at all.
  Rejected: an `apply`-time upload — it breaks the
  no-networking-beyond-Google-Ads scope and can't be idempotent (YouTube has
  no `bidsmith:address` label to dedupe re-uploads, and there is no
  `.tfstate`). The one remaining boundary is surfaced from the CLI per the
  "facts live in the binary" rule: `plan` / `apply` print
  `export::video_upload_notice` whenever the desired state references a
  video, naming the out-of-band upload step.
- **Demand Gen creatives apply, minus the call-to-action button**: the same
  mutate path covers `demand_gen_video_responsive_ad`, with two model gaps
  that are reported rather than silently dropped. `business_name` is
  REQUIRED by the API on that ad type but only *optional* in the schema (a
  required attribute would fail `validate` on every `.bid` written before it
  existed), so a create without it is a `PlanBuildError` and `validate`
  warns. `call_to_actions` are `AdCallToActionAsset` references on the wire
  — asset ids, not text — which bidsmith has no resource type for; a
  declared value therefore blocks the create with an explanatory error
  instead of applying an ad that quietly lacks its button. For the same
  reason live CTAs never round-trip, so `ad_body_key` leaves them out —
  including them would make every adopted Demand Gen ad look like a body
  change on the next plan. Chosen over inventing implicit CALL_TO_ACTION
  assets at mutate time (every other asset in bidsmith is an explicit
  declared resource) — a `google_ads_call_to_action_asset` resource is the
  follow-up.
- **Frequency caps are a repeated block managed as a whole set** (issue
  #98): `Campaign.frequency_caps` is a repeated `FrequencyCapEntry`, so
  the campaign takes a repeatable `frequency_caps { event_type,
  time_unit, time_length, cap, level? }` block — the first repeatable
  block on `google_ads_campaign`, and the reason the defaults merge's
  by-block-name rule matters (a resource declaring one cap inherits none
  of the defaults' caps, never a mix). `level` defaults to `CAMPAIGN`,
  the only value a video cap uses; `AD_GROUP` / `AD_GROUP_AD` stay
  available for display campaigns. The list diffs **as a set**, not
  positionally: reordering blocks is a no-op, and the whole field is one
  `frequency_caps` entry in the update mask, replaced wholesale (an empty
  list clears every cap). Consequence, and the point of the issue: caps
  set in the Google Ads UI on a campaign bidsmith manages surface as
  drift instead of staying invisible. Frequency capping is unsupported on
  Demand Gen, so `validate` warns rather than letting the setting apply
  and do nothing.
- **A budget backing two campaigns must say `explicitly_shared = true`**,
  and `plan` says so first (verified live). Google Ads rejects the second
  campaign with "Only explicitly shared campaign budgets can be used with
  multiple campaigns" — a trap precisely because `explicitly_shared`
  defaults to `false` and bidsmith *fills that default in*, so the file
  that earns the rejection never mentions the field. The raw failure is
  also badly attributed: it lands on the second campaign while the first
  reports "Resource was not found", and the atomic batch takes every
  unrelated change with it. Checked in `diff` over the resolved
  `ExportInput`, not in `lint`: grouping by source name counts five
  modules that each declare a local `budget` as one budget shared five
  ways — the same false-positive class the video check hit. bidsmith
  warns rather than flipping the flag itself, because `explicitly_shared`
  is real Google Ads state (a shared budget behaves differently and shows
  up in the shared library), not a formality. The neighbouring
  `BIDDING_STRATEGY_TYPE_INCOMPATIBLE_WITH_SHARED_BUDGET` is deliberately
  *not* modelled: it needs a shared budget plus a CPM/CPV strategy, and
  every channel that reaches that pairing is already refused (VIDEO is
  unmutable, DISPLAY rejects `target_cpm` regardless of the budget), so
  there is no reachable case to warn about.
- **The VIDEO channel is read-only through the Google Ads API** (issue
  #104, verified live): "You cannot create new Video campaigns or update
  existing ones using the Google Ads API"
  (developers.google.com/google-ads/api/docs/video/overview). Confirmed
  against a real account — a `validateOnly` create of a VIDEO campaign
  returns `MUTATE_NOT_ALLOWED` on the operation whatever bidding
  strategy it carries, and so does a no-op rename of a live one, while
  the identical SEARCH and DISPLAY operations are accepted. The
  restriction is the **channel**, not the campaign resource: a bid
  update on a live TARGET_CPV in-stream ad group comes back
  `OPERATION_NOT_PERMITTED_FOR_CONTEXT` and a plain `status` update on
  the same ad group comes back `MUTATE_NOT_ALLOWED`, while the same
  ad-group bid update against a DEMAND_GEN campaign is accepted (issue
  #109, verified live). Label writes are allowed, so `~ adopt` is fine.
  No bidsmith change can lift this. Video campaigns are therefore an **adopt-only**
  resource: built in the UI, adopted by name, held in `.bid` files as the
  record of what's live, and planned as no-ops. Because `apply` sends one
  atomic batch, a single video op would reject every unrelated operation
  with it — so `diff` warns *before* the request goes out on any video
  create or drift. The check lives in `diff`, not `lint`: only the live
  side knows whether a declared video campaign is new (uncreatable) or
  adopted (fine), and an offline rule fired on all 21 adopted campaigns
  in the first real-account run. Demand Gen is the API-manageable
  channel for YouTube inventory.
- **Bidding is one block, chosen from a fixed set** (issue #104):
  `Campaign.campaign_bidding_strategy` is a protobuf `oneof`, so the
  campaign takes at most one of `manual_cpc`, `manual_cpm`,
  `manual_cpv`, `target_cpm`, `target_cpv` — declaring two is a
  validate error, and a `defaults "google_ads_campaign"` bidding block
  is suppressed wholesale (not per-name) once the resource picks its
  own, so a video campaign can opt out of a shared `manual_cpc`. Every
  strategy but `manual_cpc` models nothing bidsmith writes — three are
  empty messages in the API and `target_cpm`'s one field
  (`target_frequency_goal`) is deferred — so its block carries no
  attributes: picking it is the whole declaration and the bid amount
  lives on the ad group. For the four video strategies
  that makes the block a *read* surface (see the VIDEO entry above) —
  `manual_cpc` is the one bidsmith genuinely writes. The empty ones are
  also unreadable through GAQL — there is no leaf field to select — so the
  live side derives them from `campaign.bidding_strategy_type`
  (`manual_cpc` still comes off `manual_cpc.enhanced_cpc_enabled`,
  because enhanced CPC reports as `ENHANCED_CPC` rather than
  `MANUAL_CPC`). A strategy switch sends the desired member alone —
  setting one member of a `oneof` clears the others — but **masks it by
  its subfields, not by name** (issue #120): Google Ads refuses an
  update mask that names a message field carrying subfields, even when
  the operation leaves every one of them unset, so `manual_cpc` goes out
  as `manual_cpc.enhanced_cpc_enabled` and `target_cpm` as
  `target_cpm.target_frequency_goal`. Only the field-less messages can be
  masked by name. Conversion-based strategies (`target_cpa`,
  `maximize_conversions`, …) stay out until bidsmith models conversion
  tracking.
- **An undeclared bidding block means unmanaged**: the same rule the
  frequency caps follow, for the same reason — a file that never names
  a strategy diffs as if the field didn't exist rather than planning a
  switch to whatever the block list happens to default to. Consequence:
  a campaign adopted without a bidding block no longer reports
  `manual_cpc.enhanced_cpc_enabled` drift against the live account.
- **A flight window is a committed fact, and "no end date" is an
  omitted attribute** (issue #113): `start_date` / `end_date` are
  modelled so a time-boxed campaign carries its own stop, rather than
  the end of a flight living in a README as prose. Google records "runs
  until further notice" as the sentinel date `2037-12-30` instead of
  clearing the field, so the live side maps that sentinel to `None` —
  otherwise every adopted campaign would render a fake end date that
  someone then has to maintain. An omitted date is unmanaged, as
  everywhere else. Dates get their own `FieldType::Date` rather than
  passing as strings, because `2026-02-30` and `11.08.2026` are exactly
  the kind of typo a validator should catch before the API does, and
  `lint` warns when a campaign would end before it starts — which
  Google accepts and then silently never delivers. **Deferred:** warning
  on an `end_date` already in the past, which needs a clock and so needs
  injectable time before it can be tested.
- **An operation that can never succeed is caught locally, and what
  happens next depends on what the file is asserting** (issue #116): the
  mutate batch is atomic, so one doomed operation rejects every unrelated
  one with it — which turns one team's stale declaration into everybody's
  red plan. A **create or update** on the read-only VIDEO channel blocks
  the plan and nothing is sent: the file is asserting a state the account
  can never reach, and quietly dropping it would leave the repo and the
  account disagreeing forever under a green plan. A **removal** of a
  labeled VIDEO resource the file no longer declares is skipped with a
  warning instead: a file that stopped mentioning a resource is not
  asserting anything about it, nothing is lost by leaving it alone, and
  skipping lets every unrelated operation through. The same precedent
  already existed for ads orphaned under a removed ad group, which the
  API also refuses to mutate. Skips are counted in the summary
  (`4 to destroy (2 skipped)`) so they read as a decision, not an
  omission.
- **The ad group models every settable bid field, and an omitted one is
  unmanaged** (issue #109): the campaign's block picks the strategy, the
  ad group carries the amount, and which field holds it follows from the
  strategy — a TARGET_CPV in-stream ad group bids through
  `target_cpv_micros` and leaves `cpc_bid_micros` at zero. Modelling
  only `cpc_bid_micros` therefore made a video bid not merely unwritable
  but *invisible*: an unmodelled field is an undiffed one, so `plan`
  reported a clean ad group while the repo's documented bid and the live
  bid disagreed. Google returns all eight fields with the unused ones
  zeroed, so the diff follows the bidding-block rule and only compares
  what the file names — otherwise every Search ad group would plan five
  spurious bid clears. For the same reason `export` renders a bid field
  only when it is non-zero (`cpc_bid_micros` excepted, which renders as
  it always has): a declared zero is a real value the create path would
  send, and Google rejects a bid that doesn't match the strategy. The
  read-only `effective_*` variants stay out.
- **An undeclared `frequency_caps` block means unmanaged, not "clear
  the caps"** (issue #102): the set is the one non-criterion field whose
  "declared as empty" is unwritable — a repeated block has no empty
  form — so it follows the criteria ownership rule instead. Declaring
  ≥1 cap claims the field (`bidsmith:owns=frequency_caps` on the
  campaign, the same association the criterion categories use, shown as
  a `~ claim (+frequency caps)` row); dropping the last block on a
  claimed campaign plans the clear and releases the claim. A campaign
  that never declared a cap diffs as if the field didn't exist. Without
  the gate, merely *reading* a new repeated field turned every
  UI-capped campaign into a pending clear — destructive where it
  applied, and un-appliable where the API refuses to mutate the
  campaign at all, which sinks the whole atomic batch. The same gate is
  why reading further repeated fields later is safe by construction.
  `refresh --in-place` now round-trips the blocks instead of reporting
  them: the live set replaces the declared blocks wholesale (in place,
  or appended when the file declared none), which is the one insert
  reconcile will make — a repeated block has a canonical rendering, so
  writing one isn't the formatting guesswork an absent scalar is.
- **Video targeting is criterion subtypes plus one segment builder**
  (issue #99): `google_ads_campaign_criterion` gains `youtube_channel`,
  `youtube_video`, `topic`, `user_interest`, `age_range`, `gender`, and
  an `audience` block — each one more `CampaignCriterion` variant on the
  existing create / read / diff / destroy path, so the whole set is
  additive and `negative = true` flips any of them to an exclusion. The
  `audience` block is the one departure from the schema's usual 1:1
  naming: `CustomAudienceInfo` / `UserListInfo` / `CombinedAudienceInfo`
  are three API messages that all answer "which audience?", and the
  faithful shape (`custom_audience { custom_audience = … }`) stutters, so
  they collapse into one block taking exactly one of three attributes —
  the shape the issue proposed, and the one Google's newer unified
  `Audience` resource can join later as a fourth. Ownership follows the
  existing rule unchanged: declaring one criterion of a kind on a
  campaign makes bidsmith own that kind there, so a channel added in the
  UI reads as drift while kinds you never declare stay untouched.
  Rendering a criterion had to start emitting `negative` — negative
  keywords fold into the grouped form, but every other exclusion is a
  singleton and would otherwise round-trip as positive targeting.
  `google_ads_custom_audience` is the one new resource, because a
  search-intent segment is the piece Google has no *other* way to
  express declaratively (user lists and combined audiences have no
  create API worth wrapping — they are referenced by resource name).
  It matches live **by name** like `shared_set` (custom audiences carry
  no labels), its repeated `member` blocks are a whole-set field like
  `frequency_caps`, and `type` is creation-only.
- **Custom audiences are mutated by their own service, before the batch**
  (issue #105): `MutateOperation` has no `custom_audience_operation`
  member, so a `customAudienceOperation` in the unified batch is rejected
  at JSON-parse time and takes every other op down with it. They go to
  `CustomAudienceService.MutateCustomAudiences` in a call of their own,
  issued first, and the batch references the real resource names it
  returns. That service has no temp-id mechanism, so a create body
  carries no `resourceName` — and under `validateOnly` it returns errors
  but no results, which means the pre-flight has no name to give a
  criterion that targets a *new* audience. Those criteria are held out
  of the validate batch and reported as `deferred` rather than sunk with
  a resource-not-found; the real apply, which does get names back,
  includes them. Consequence to know: the two calls are not one
  transaction, so a batch that fails after the audiences committed
  leaves them created. Chosen over dropping the resource or making the
  user pre-create audiences in the UI — the ordering is the same
  dependency the plan already models, just across two calls.
- **Keyword Planner is a read-only research verb, not a resource**: Google
  Keyword Planner (`KeywordPlanIdeaService.GenerateKeywordIdeas`) is surfaced
  as `bidsmith keyword-ideas` — an imperative, live-only command in the same
  family as `query`, **not** a declarative `.bid` resource. Keyword ideas are
  exploratory output (seed terms / a landing-page URL → related keywords with
  search volume, competition, top-of-page bid estimates), with no desired
  state to declare, diff, or reconcile, so it deliberately touches none of the
  declarative machinery: no `src/schema.rs` entry, no `export` renderer, no
  `plan` / `apply` path, and it reads no `.bid` files. It rides the existing
  REST transport (a new colon custom-method on `Client` —
  `customers/{id}:generateKeywordIdeas`, which hangs directly off the customer
  with no `googleAds:` segment, unlike searchStream / mutate) and the existing
  OAuth + customer envelope. Locations / languages accept the same
  human-readable codes as a campaign's `locations` / `languages`, resolved
  through the in-binary `src/targeting.rs` tables, so research and authoring
  speak one vocabulary. Because it's a new API call site, the Basic-Access
  design doc lists the endpoint (design-doc lockstep). Chosen over modelling a
  server-side `KeywordPlan` resource tree (saved plans / forecasts) — that is a
  heavier, rarely-wanted surface; the 90% use is one-shot idea generation, which
  a read verb serves without any state model. `GenerateKeywordHistoricalMetrics`
  and forecast RPCs are deferred follow-ups.

## Current state

```
bidsmith/
├── .gitignore
├── Cargo.toml            # hcl-edit, miette, clap, thiserror, serde, serde_json, reqwest
├── Cargo.lock
├── DECISIONS.md          # this file
├── ROADMAP.md            # forward-looking plan
├── README.md
├── src/
│   ├── main.rs           # clap dispatcher, subcommands
│   ├── parser.rs         # hcl-edit wrapper: parse_file → ParsedFile
│   ├── schema.rs         # resource-type registry + validator
│   ├── targeting.rs      # geo/language code ↔ Google Ads constant tables
│   ├── lint.rs           # soft-issue warnings (status, RSA min, phone)
│   ├── diagnostics.rs    # miette Diag type with severity
│   ├── api/
│   │   ├── mod.rs
│   │   ├── auth.rs       # OAuth refresh-token → access token
│   │   ├── cache.rs      # .bidsmith/cache/ read cache (live state + access token)
│   │   ├── creds.rs      # credential resolver (env → ~/.bidsmith/credentials.toml → default) + storage
│   │   ├── oauth.rs      # browser loopback + PKCE authorization-code flow (auth login)
│   │   ├── client.rs     # reqwest::blocking wrapper; googleAds:mutate + :searchStream + listAccessibleCustomers
│   │   ├── live_state.rs # six GAQL queries → populated ExportInput
│   │   ├── import.rs     # AST → ExportInput (round-trip with the renderer)
│   │   ├── diff.rs       # declared vs live → Create / NoOp / Update(fields) / Delete(orphaned member)
│   │   └── mutate.rs     # ExportInput + DiffReport → googleAds:mutate body
│   └── commands/
│       ├── mod.rs        # module declarations + small shared helpers
│       ├── adapt.rs      # SearchStream JSON → ExportInput (used by export + live_state)
│       ├── auth.rs       # auth login / status / logout / profile
│       ├── apply.rs      # prepare + plan display + prompt + real mutate (--auto-approve skips the prompt)
│       ├── design_doc.rs # render the Basic-Access design document from templates/ + introspection
│       ├── e2e_cleanup.rs # hidden _e2e-cleanup verb: sweep bidsmith-e2e-* resources
│       ├── export.rs     # render .bid from a JSON source description
│       ├── fmt.rs        # canonical re-emitter (in-place / --check)
│       ├── init.rs       # scaffold a GitOps project (templates/init/ → repo skeleton)
│       ├── keyword_ideas.rs # read-only Keyword Planner research (generateKeywordIdeas)
│       ├── plan.rs       # parse + validate + import + diff + validateOnly batch
│       ├── pull.rs       # dump raw SearchStream batches as JSON
│       ├── query.rs      # read-only GAQL passthrough (table / json / tsv)
│       ├── refresh.rs    # bootstrap-mode import of live state into .bid files
│       ├── schema.rs     # dump the resource + provider schema as JSON
│       ├── validate.rs   # parse + validate orchestration
│       └── vars.rs       # --var / BIDSMITH_VAR_* resolution for variable blocks
└── examples/
    ├── basic/main.bid          # provider, budget, campaign, ad group, ad, criteria
    ├── broken/
    │   ├── schema.bid          # schema/type/ref errors
    │   └── syntax.bid          # parse error
    ├── bulk/main.bid           # bulk keyword sub-blocks, shared sets,
    │                           # RSA headlines/descriptions list attributes
    ├── compact/main.bid        # compact `keywords {}` / `negative_keywords {}`
    │                           # blocks (texts list + match_type / match_types fan-out)
    ├── lint/
    │   └── warnings.bid        # valid syntax/schema but trips every lint rule
    ├── multi/                  # two campaigns in one dir with colliding bare
    │   ├── nadarzyn.bid        # criterion names — addresses disambiguate via
    │   └── warszawa.bid        # the file-stem module prefix
    ├── locals/main.bid         # `locals { … }` plus `local.<name>` use sites
    ├── interpolation/main.bid  # string templates: "${local.x}-suffix" in names,
    │                           # URLs, headlines, and inside locals
    ├── variable/main.bid       # `variable "x" { type, default }` plus var.<name>
    ├── resource-for-each/main.bid  # `for_each` on resource blocks: device
    │                           # exclusions from a list, sitelink attachments
    │                           # from a map of references
    ├── defaults/               # `defaults "google_ads_campaign" {}` shell in
    │   ├── shared.bid          # shared.bid; two slim campaign files inherit
    │   ├── cookie-banners.bid  # it (one overrides `locations`) and fan out
    │   └── fingerprint.bid     # device exclusions via for_each
    ├── modules/                # `module "x" { source = "./…", ...inputs }`
    │   ├── main.bid            # root: two `module` instances of city-campaign
    │   └── modules/city-campaign.bid  # the parameterized source
    ├── exports/
    │   ├── basic.json          # flat bidsmith input for `export --from-json`
    │   └── raw.json            # SearchStream-shaped input for `export --from-gads-search-response`
    └── trial/
        ├── README.md           # end-to-end runbook against the rezolutnie account
        └── dump_campaign.py    # Python helper that produces raw.json-shaped dumps
```

Verified locally:
- `cargo build` clean (no warnings)
- `cargo build --features e2e` clean (gates the live round-trip
  integration test in `tests/e2e.rs`)
- `cargo build --release` → ~4 MB binary (the jump from ~1.5 MB is
  the reqwest + rustls TLS stack added for the live API client)
- `cargo run -- validate examples/basic` → `OK: 1 file(s) valid.` —
  the `summer_search` campaign now uses inline `languages = ["pl"]` /
  `locations = ["US"]` (the old explicit language/location criterion
  resources were folded into it); proximity + negatives stay explicit.
- `cargo run -- export --from-json examples/exports/basic.json -o
  /tmp/out.bid` folds the US location + Polish language criteria into
  inline `languages`/`locations` on the campaign; `validate /tmp/out.bid`
  and `fmt --check /tmp/out.bid` both pass (inline is the default
  `export` / `refresh` form for plain positive targeting).
- `cargo run -- validate examples/multi` → `OK: 2 file(s) valid.` —
  both files declare `google_ads_campaign_criterion.broad_wikipedia`
  and `…broad_olx`; the file-stem module prefix
  (`nadarzyn.…` vs `warszawa.…`) makes the addresses unique.
- `cargo run -- validate examples/locals` → `OK: 1 file(s) valid.` —
  exercises the `locals { ... }` block plus `local.<name>` references
  for budget micros, default cpc, language constant, and proximity
  radius; `fmt --check examples/locals` is a no-op.
- `cargo run -- validate examples/interpolation` → `OK: 1 file(s) valid.`
  — exercises **string interpolation** (issue #84). A shared
  `local.utm_base` is itself built from two other locals via `${…}`,
  campaign/budget names embed `${local.utm_campaign}`, every ad's
  `final_urls` appends a per-ad slug to the shared base, and headlines
  splice in `${local.brand}`. `fmt --check examples/interpolation` is a
  no-op (templates round-trip through the formatter untouched).
- `cargo run -- validate examples/lists` → `OK: 3 file(s) valid.` —
  exercises **list-valued locals** (issue #39). `shared.bid` declares
  the headline set, description set, competitor-keyword theme, landing
  URL, and `languages` / `locations` lists once; `ublock.bid` and
  `generic.bid` reference them across files via the global fallback —
  two RSAs reuse the same `local.brand_headlines` / `local.brand_descriptions`,
  the campaigns take inline `languages` / `locations` from a shared list,
  and a compact `keywords { texts = local.competitor_keywords }` block
  fans the shared list out into one criterion per keyword. The generic
  RSA merges three ad-specific headlines with the shared
  `local.brand_tail_headlines` via `concat(...)` (issue #85). No false
  RSA min-headline warnings; `fmt --check examples/lists` is a no-op.
- `cargo run -- validate examples/ad-templates` → `OK: 2 file(s) valid.`
  — exercises **reusable ad bodies** (issue #40). `templates.bid`
  declares `ad_template "ublock_rsa"` / `"generic_rsa"` once; `ublock.bid`
  attaches `ad_template.ublock_rsa` to three ad groups
  (`template = ad_template.ublock_rsa`) across the file boundary, so three
  `google_ads_ad_group_ad` resources share one body. Two of them add
  per-instance `final_urls` / `path1` overrides (issue #58) — same body,
  different landing page — so one template serves all three. `fmt --check
  examples/ad-templates` is a no-op.
- `cargo run -- validate examples/variable` → `OK: 1 file(s) valid.` —
  exercises the `variable "x" { type, default, description }` block
  plus `var.<name>` references for a string (campaign name), number
  (city radius), and bool (enhanced CPC). `--var city_radius_km=25`
  and `BIDSMITH_VAR_city_radius_km=30` both override the default; an
  invalid input value (`--var enhanced_cpc=not-a-bool`) fails with a
  span-mapped error pointing at the variable declaration; removing
  the `default` and not supplying `--var` / env produces a
  "variable 'x' has no value" diagnostic.
- `cargo run -- validate examples/modules` → `OK: 3 file(s) valid.` —
  exercises the top-level `module "instance" { source = "./…", ...inputs }`
  block. `examples/modules/main.bid` instantiates
  `examples/modules/modules/city-campaign.bid` twice (Warsaw + Krakow)
  with distinct city names, coordinates, radii, and budgets. Resources
  inside each instance get addresses prefixed with the instance name
  (`warsaw.google_ads_campaign.search`,
  `krakow.google_ads_campaign.search`). A bogus `--var` is rejected;
  a missing required input surfaces inside the module file; a wrong-
  typed input (`latitude = "fifteen"`) gets a span-mapped error
  pointing at the module's `variable` declaration; a duplicate
  `module "x"` block is rejected; a nested `module` block inside the
  source file is rejected with "nested modules are not supported yet."
  The `city-campaign.bid` source (whose `variable`s have no defaults)
  is excluded from the top-level scope, so the recursive directory
  walk doesn't try to validate it as a standalone file.
- `cargo run -- validate examples/defaults` → `OK: 3 file(s) valid.` —
  exercises the **`defaults` block** (issue #87). `shared.bid` declares
  the ~20-line search-campaign shell (`advertising_channel_type`,
  `languages` / `locations`, EU-political declaration, `manual_cpc`,
  `network_settings`) once; the two campaign files shrink to
  name + status + budget. `fingerprint.bid` overrides `locations` per
  resource, and both files fan out the mobile/tablet exclusions from
  `local.excluded_devices` via resource `for_each` (#86) — together the
  full issue-#87 shape. Omitting a required attribute the defaults
  provide (e.g. `advertising_channel_type`) validates clean;
  `fmt --check examples/defaults` is a no-op.
- `cargo run -- validate examples/resource-for-each` →
  `OK: 1 file(s) valid.` — exercises **`for_each` on resource blocks**
  (issue #86). One `google_ads_campaign_criterion` block fans out into
  the mobile + tablet `bid_modifier = 0` exclusions from
  `local.excluded_devices` (`each.value` inside the `device` block), and
  one `google_ads_campaign_asset` block attaches two sitelink assets
  from a `for_each` map whose values are resource references
  (`asset = each.value`). Instance addresses are keyed
  (`…cookies_device_exclusions["MOBILE"]`);
  `fmt --check examples/resource-for-each` is a no-op.
- `cargo run -- validate examples/modules-for-each` →
  `OK: 4 file(s) valid.` — exercises `for_each` on a `module` block.
  `examples/modules-for-each/main.bid` instantiates
  `templates/preroll-campaign.bid` three times (one per `for_each`
  entry) with a shared `geo` and per-entry `campaign_name` / `final_url`;
  resources get addresses `ghostery_search.<key>.<type>.<name>`
  (`ghostery_search.privacy.google_ads_campaign.search`, …).
  `fmt --check examples/modules-for-each` is a no-op.
- `cargo run -- validate examples/broken` → exit 1 with 11 errors and
  5 warnings (parse failure, type mismatch, enum violation, dangling
  reference, unknown attribute at two depths, unknown resource type,
  missing required field, list type mismatch, wrong list-element type,
  invalid keyword match_type, invalid RSA pin; plus incidental
  status/RSA-block lint warnings on the affected resources).
- `cargo run -- validate examples/bulk` → `OK: 1 file(s) valid.` —
  exercises the bulk `keyword {}` / `negative_keyword {}` ad-group
  form, `google_ads_shared_set` + `google_ads_campaign_shared_set`,
  and the `headlines = [...]` / `descriptions = [...]` RSA list
  attributes. Re-encodes the rezolutnie `[W2]` campaign (1025 lines
  one-resource-per-keyword) in 177 lines.
- `cargo run -- validate examples/compact` → `OK: 1 file(s) valid.` —
  exercises the compact `keywords {}` / `negative_keywords {}` form on
  ad-group criteria (single `match_type` and a `match_types` fan-out),
  campaign criteria, and a shared set; `fmt --check examples/compact`
  is a no-op.
- `cargo run -- validate examples/lint` → exit 0 with 10 warnings (the
  lint rules trip: missing `status` on three blocks, RSA headlines
  < 3, RSA descriptions < 2, phone number in a headline, the
  suspicious `languageConstants/1045` (Afar) entry, a headline over 30
  chars, a description over 90 chars, and a path1 with uppercase /
  underscore outside the `[a-z0-9-]` charset).
- `cargo run -- export --from-json examples/exports/basic.json`
  round-trips through `validate` cleanly (`-o out.bid` then
  `validate out.bid` → OK).
- `cargo run -- export --from-gads-search-response
  examples/exports/raw.json -o /tmp/raw.bid && validate /tmp/raw.bid
  && fmt --check /tmp/raw.bid` → all OK. Export output is
  fmt-canonical by construction (the renderer's string is parsed and
  re-emitted through the same `format_body` fmt uses), so chaining
  through `fmt --check` is always a no-op.
- `cargo run -- fmt --check examples/basic examples/lint` →
  `fmt: N file(s) already canonical.` (idempotent; canonical = 2-space
  indent, single space around `=`, blank line between blocks but not
  within attribute runs, arrays wrap onto multiple lines when the
  single-line form exceeds 80 chars).
- `cargo test` (offline) runs three `render_split` checks in
  `src/commands/export.rs` that lock in the account-vs-campaign
  bucket split.
- `cargo run -- export --from-gads-search-response examples/exports/raw.json`
  now also emits a `google_ads_conversion_action`, `google_ads_call_asset`,
  and `google_ads_customer_asset` from the same dump, round-tripping
  through `validate` and `fmt --check` cleanly. The fixture is the
  offline CI's account-level smoke test.
- `bidsmith plan --whoami` against a real `.env` exchanges the refresh
  token and prints `access token … expires_in: 3599s` plus the
  customer / login / developer-token envelope. No Google Ads API call
  yet — just the OAuth token endpoint.
- `bidsmith plan --read-live` lists per-resource-type counts for the
  customer (one `googleAds:searchStream` call per type bidsmith
  models).
- `bidsmith plan examples/basic` against the rezolutnie account
  validates 8 CREATE operations on the live API and prints
  `8 accepted, 0 rejected (validateOnly)`. Proves every resource
  bidsmith models is API-faithful end-to-end.
- `bidsmith plan /tmp/w1.bid` against the rezolutnie account (where
  the campaign already exists) prints `Plan: 0 to create, 0 to
  update, 0 to destroy, 97 unchanged. (no API call needed)` once the .bid is
  in-sync with live. Editing any scalar in the file produces a
  single `~ update (field)  ok` row + 96 no-ops on the next run.
- `cargo test` runs the three offline `render_split` checks plus a
  cache round-trip suite (`api::cache::tests::*`) covering token
  fingerprint mismatch, near-expiry skew, live-state TTL eviction, and
  `invalidate_live_state`.
- `bidsmith plan --offline path/to/.bid` reads `.bidsmith/cache/`
  only, prints the diff with `(offline — diff only, not server-
  validated)` summary, and makes zero API calls. Errors if no fresh
  cache exists, pointing the user at `bidsmith pull` to warm it.
- `bidsmith apply` shows the validateOnly diff first, then prompts
  `Apply these changes? Only 'yes' will be accepted to approve.`
  before flipping `validateOnly: false` and mutating. `--auto-approve`
  skips the prompt (required when stdin is not a TTY).
- `bidsmith refresh -d <DIR>` pulls live state and writes the split
  pair `<DIR>/account.bid` (provider + conversion actions + call
  assets + customer assets + shared sets) and `<DIR>/campaigns.bid`
  (provider + budgets + campaigns + ad groups + RSAs + criteria +
  campaign-shared-sets). `-o <FILE>` collapses both into one file;
  no flag writes the concatenation to stdout. These are the
  **bootstrap** modes — they overwrite their output files.
- `bidsmith refresh --in-place [PATH]` is **reconcile mode** (Phase 4
  v2, unblocked by the Phase 3 v2 identity labels): it reads the
  existing `.bid` files at PATH (default `.`), diffs them against
  live, and writes back only the **drifted scalar fields** on
  resources bidsmith manages — updating attribute values in place
  while leaving comments, block layout, ordering, and unmanaged
  resources untouched. Matching reuses the planner's label-first
  diff, so the live value is written to the resource the label
  already points at. Scope is deliberately narrow: it only patches
  attributes that already exist in source (an absent attribute is
  reported, never inserted — formatting an insert is guesswork), and
  only 1:1-block scalar kinds (budget, campaign incl. `manual_cpc.*`
  / `network_settings.*`, ad_group, ad_group_ad, conversion_action,
  customer_asset, shared_set, campaign_shared_set), plus the campaign's
  `frequency_caps` set, which round-trips as whole blocks (issue #102).
  Structural drift (ad copy, keyword/criterion membership) is
  reported, not edited —
  the diff engine only ever yields scalar `Update`s, so a changed RSA
  is a create+destroy elsewhere, handled by `apply`, not this pass.
  `--check` previews without writing. A `mv`-style baseline-error
  guard re-validates the mutated tree and refuses to write if the
  edit would break the project. Pure core (`reconcile_sources`) is
  unit-tested offline; the live fetch reuses the plan/apply cache.
- **Reconcile loads the tree as a program, not as loose files**
  (issue #93): `refresh --in-place` goes through `Program::load` +
  `import_program`, the same path `validate` / `plan` use, so a
  `templates/*.bid` file reached through a `module` block is an
  instance scope rather than a standalone root and its `var.*`
  references resolve against the caller's inputs. It takes `--var` /
  `BIDSMITH_VAR_<name>` for the same reason `plan` does. A template
  is written once no matter how many instances it backs, which
  constrains what may be patched: a field is only writable when
  **every** instance drifted to the **same** value, and only when the
  attribute is a literal — an attribute holding `var.*` / `local.*` /
  a reference is reported, never flattened to the live literal, since
  that would erase the indirection for every instance at once.
- `bidsmith auth login` runs the browser OAuth loopback + PKCE flow and
  writes `~/.bidsmith/credentials.toml` (mode `0600`, home dir `0700`);
  `auth status` shows the resolved credentials and (given a developer
  token) lists accessible accounts; `auth logout` drops the sign-in but
  keeps the developer-token + MCC "team profile", `--all` wipes the
  file; `auth profile` prints the shareable team blob. The
  offline-resolvable paths (field display, precedence, profile, logout
  round-trip, file mode) verified by hand with `BIDSMITH_HOME` pointed
  at a temp dir and the GOOGLE_ADS_* env cleared; env still wins over the
  file. Unit tests in `api::creds` (precedence, mismatch guard, TOML
  round-trip) and `api::oauth` (PKCE RFC-7636 vector, query parsing,
  percent-encoding) cover the pure logic.
- `cargo test` runs three offline unit tests in `commands::export`
  that lock in the account-vs-campaign split (`render_split`).

Validator covers (so far):
- `google_ads_campaign_budget`, `google_ads_campaign` (one bidding
  block out of `manual_cpc` / `manual_cpm` / `manual_cpv` /
  `target_cpm` / `target_cpv`, plus `network_settings` and the required
  `contains_eu_political_advertising` enum — defaults to
  `DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING` at mutate time when the
  attribute is omitted, since Google Ads rejects new campaigns that
  don't declare it; plus optional inline `languages = [...]` /
  `start_date` / `end_date` as `YYYY-MM-DD` dates validated as real
  calendar dates locally; plus optional inline `languages = [...]` /
  `locations = [...]` list attributes that each expand to one positive
  campaign criterion at import time, resolving human-readable codes
  (`"en"`, `"US"`) — or raw `languageConstants/NNNN` /
  `geoTargetConstants/NNNN` strings — to the API constants, plus a
  repeatable `frequency_caps { event_type, time_unit, time_length,
  cap, level? }` block managed as a whole set once declared, with a
  validate-time guard against declaring the same axis both inline and as
  an explicit positive criterion resource), `google_ads_ad_group`
  (with every settable `AdGroup` bid field — `cpc_bid_micros`,
  `cpv_bid_micros`, `cpm_bid_micros`, `target_cpa_micros`,
  `target_cpm_micros`, `target_cpv_micros`, `percent_cpc_bid_micros`,
  `fixed_cpm_micros` — of which the campaign's strategy decides which
  one carries the bid), `google_ads_ad_group_ad`
  (with `ad` → `responsive_search_ad` → repeating
  `headline { text, pin? }` / `description { text, pin? }` blocks,
  plus an equivalent list-attribute form `headlines = [...]` /
  `descriptions = [...]` whose items are either bare strings or
  `{ text, pin? }` object literals — both forms can coexist, and
  `final_urls` still uses `list<string>`; in place of an inline `ad {}`
  block the resource may carry `template = ad_template.<name>`, attaching
  a reusable body declared in a top-level `ad_template "name" { … }` block
  — exactly one of `ad {}` / `template` is required, and the template is
  resolved and substituted at import time so the mutate is identical to
  the inline form; a templated resource may additionally set `final_urls`
  / `path1` / `path2` to override those fields of the template body
  per-instance),
  `google_ads_ad_group_criterion` (positive/negative keyword with
  match_type, optional per-keyword `cpc_bid_micros`; also accepts a
  bulk form where repeating `keyword {}` and/or `negative_keyword {}`
  sub-blocks in one resource expand into N individual criteria at
  import time — the `negative` attribute is inferred from the block
  shape, so `keyword {}` defaults `negative = false` and
  `negative_keyword {}` is always `negative = true` without writing
  the attribute explicitly; plus a *compact* form `keywords { texts =
  [...], match_type = "EXACT" }` (or `match_types = ["EXACT","PHRASE"]`
  to fan one list out across several match types) and its
  `negative_keywords {}` counterpart, where one block expands into the
  cartesian product of texts × match types — exactly one of
  `match_type` / `match_types` is required, validated up front. The
  compact and per-keyword forms are equivalent at import time (each
  (text, match_type) pair is one criterion, matched by that key in the
  diff), so the choice is purely authoring ergonomics and the two can
  coexist in one resource. `fmt` does not fold between the forms),
  `google_ads_campaign_criterion` (single negative keyword, location,
  language, proximity with flat `latitude` / `longitude` in decimal
  degrees plus `radius` + `radius_units`; the adapter rounds to the
  API's micro-degree integers at the wire boundary; plus a `device`
  block (`type` = `MOBILE` / `DESKTOP` / `TABLET` / `CONNECTED_TV` /
  `OTHER`) paired with a top-level `bid_modifier` number for device bid
  adjustments — `0.0` = −100% = opt the device out, the desktop-only /
  mobile-excluded case from issue #71; the device type is the match key
  and `bid_modifier` is a scalar field diff, so retuning a modifier is an
  in-place update, not a recreate; live device criteria are never
  destroyed — the API forbids removing them, and Google auto-materializes
  every device type once any device targeting exists (issue #82), so an
  undeclared default-state device criterion is implicitly desired and one
  carrying an adjustment surfaces as a plan warning instead of a doomed
  remove op that would sink the whole atomic batch; plus the video
  targeting axes from issue #99 — `youtube_channel { channel_id }`,
  `youtube_video { video_id }`, `topic { topic_constant }`,
  `user_interest { user_interest_category }`, `age_range { type }`,
  `gender { type }`, and `audience { custom_audience | user_list |
  combined_audience }` taking exactly one of three — each usable as an
  exclusion via `negative = true`), `google_ads_custom_audience`
  (`name`, `description`, creation-only `type` = `AUTO` / `INTEREST` /
  `PURCHASE_INTENT` / `SEARCH`, `status`, and repeatable
  `member { keyword | url | place_category | app }` blocks managed as a
  whole set; matched to live by name like `shared_set`), plus a bulk
  syntactic-sugar form where repeating `negative_keyword { text,
  match_type }` sub-blocks in one resource expand into N individual
  negative criteria at import time (same `negative`-from-block-shape
  inference as ad-group criteria), plus the compact
  `negative_keywords { texts = [...], match_type/match_types }` form,
  `google_ads_shared_set` (named negative-keyword set with a bulk
  `negative_keyword { text, match_type }` sub-block form, also the
  compact `negative_keywords { texts = [...], match_type/match_types }`
  form; type defaults to `NEGATIVE_KEYWORDS` at mutate time),
  `google_ads_shared_criterion` (a single negative keyword inside a
  shared set, declared as its own top-level resource for fine-grained
  add/remove diffs — equivalent at mutate time to a single
  `negative_keyword` sub-block on the parent set),
  `google_ads_campaign_shared_set` (links a `google_ads_shared_set`
  to a `google_ads_campaign`; both `campaign` and `shared_set` accept
  either a typed reference or a literal Google Ads resource-name
  string for gradual adoption),
  `google_ads_conversion_action`
  (`type`, `category`, lookback windows, optional `value_settings`
  sub-block with default value / currency / always-use flag),
  `google_ads_call_asset` (country code + phone number, optional
  `call_conversion_reporting_state` and reference to a
  `google_ads_conversion_action` — the field also accepts a literal
  Google Ads resource-name string so refreshes against accounts that
  reference removed or out-of-scope conversion actions still
  round-trip),
  `google_ads_sitelink_asset` (a text-extension link: `link_text`,
  optional `description1` / `description2`, and its own `final_urls`),
  `google_ads_callout_asset` (a short `text` highlight), and
  `google_ads_structured_snippet_asset` (a `header` plus a `values`
  list) — the three search-ad text extensions that grow an RSA on the
  page,
  `google_ads_customer_asset` (links any modeled asset — call / sitelink
  / callout / structured snippet — to the whole account with a matching
  `field_type`), `google_ads_campaign_asset` (the same link scoped to a
  single `campaign`) and `google_ads_ad_group_asset` (scoped to a single
  `ad_group`); assets are content-immutable, so an asset's identity is
  its full content and a text edit plans as a new asset, while the link
  resources diff only on `status`,
  `google_ads_youtube_video_asset` (a reference to a video already
  published on YouTube — `youtube_video_id` required, optional
  `youtube_video_title`; the creative side of a `video_responsive_ad`).
  The `ad {}` body also accepts a `video_responsive_ad` block (a
  `video` reference to a `google_ads_youtube_video_asset` plus optional
  `headlines` / `long_headlines` / `descriptions` / `call_to_actions`
  string lists) or a `demand_gen_video_responsive_ad` block (the ad type
  a `DEMAND_GEN` campaign carries — a `videos` list of
  `google_ads_youtube_video_asset` refs plus optional `headlines` /
  `long_headlines` / `descriptions` / `call_to_actions` / `breadcrumb1`
  / `breadcrumb2` / `business_name`) as an alternative to
  `responsive_search_ad` — an `ad`
  carries at most one creative, enforced at validate time. `pull` selects
  both video creatives and the `YOUTUBE_VIDEO` asset table, so an
  existing video / Demand Gen campaign round-trips through `export`
  (headlines, long headlines, descriptions, breadcrumbs, business name,
  and the video-asset refs;
  `call_to_actions` come back from the API as asset references rather than
  inline text, so they do not populate on a round-trip)
- `provider "google_ads"` (`customer_id` optional — resolved from
  `bidsmith.toml` / env / global credentials when omitted, so `.bid`
  files can be account-agnostic; `login_customer_id` optional —
  overridable via `--login-customer-id` / `--customer-id` on `export`)
- Type system: `string`, `integer`, `number`, `bool`, `enum<…>`,
  `ref<targets>`, `list<T>` (recurses into each element)
- Two-pass validation: collect addresses, then walk each block.
- Lints (warning severity, do not affect exit code): missing `status`
  on campaign / ad_group / ad_group_ad / criterion blocks; responsive
  search ad with `< 3` headline blocks or `< 2` description blocks;
  phone-number-like patterns (7+ digits with phone separators) inside
  any headline/description `text` attribute; headline text over 30
  chars or description text over 90 chars; `path1` / `path2` over 15
  chars or containing characters outside `[a-z0-9-]`; suspicious
  `language_constant` values (currently just `languageConstants/1045`,
  Afar — a near-universal typo for `1030`, Polish); a
  `demand_gen_video_responsive_ad` missing `business_name` or setting
  `call_to_actions` (either one stops `apply` from creating the ad).

**CLI verbs**:

| Verb       | Status  | Purpose                                              |
|------------|---------|------------------------------------------------------|
| `fmt`      | partial | Canonicalize `.bid` files (in-place; `--check` for CI). `--minimal` also strips optional attributes left at their schema default — the form `refresh` / `export` emit — while `always_emit` compliance fields stay |
| `mv`       | working | Rename a resource address in source: rewrites the `resource` block label and every reference that resolves to it, across all `.bid` files under `--path` (default `.`). Addresses are `<type>.<name>`, or `<module>.<type>.<name>` to disambiguate a name shared across files. **Bulk mode** `--from-file <path>` (or `-` for stdin) renames a whole batch from a `<from> <to>`-per-line file (arrow optional, `#` comments) applied atomically against one snapshot — rejects missing sources, occupied targets, duplicate sources/targets, and rename chains (`a→b`,`b→c`); any bad rule writes nothing. Format-preserving (only the renamed identifiers change; comments and layout are byte-preserved). Refuses when the rename would raise the project's validation-error count above its pre-rename baseline (so it can still tidy a not-yet-fully-valid tree). **Source-only by design**: because the planner matches live resources by content (name / keyword / geo / …), not by address or label, an address rename is invisible to the account — no delete+create, no lost history or ad review. Once labels become identity (Phase 3 v2), a move will additionally rewrite the live `bidsmith:address` label; until then `mv` is the complete mechanism and `moved` blocks are deferred |
| `validate` | partial | Syntax + schema + references + lint warnings (local only). `--var NAME=VALUE` (repeatable) supplies values for `variable` blocks; `BIDSMITH_VAR_<name>` env vars are the fallback |
| `export`   | partial | Render a fmt-canonical `.bid` file from flat bidsmith JSON (`--from-json`) or raw Google Ads SearchStream JSON (`--from-gads-search-response`); always emits the compact form (one `google_ads_ad_group_criterion` per `(ad_group, match_type)` group with N `keyword {}` sub-blocks, one negatives resource per ad-group / campaign with N `negative_keyword {}` sub-blocks, RSAs as `headlines = [...]` / `descriptions = [...]` lists). Also **folds repeated structure** (issue #57): ad bodies shared across ≥ 2 ads become a top-level `ad_template` (URL-variant bodies collapse onto one URL-agnostic template + per-instance `final_urls` / `path1` / `path2` overrides), RSA arrays used by ≥ 2 sites and campaign negative lists shared by ≥ 2 campaigns become `locals`. Folding is source-only — the tree round-trips through `validate` / `plan` identically to the verbose form. Drops REMOVED resources unless `--include-removed`; `--login-customer-id` / `--customer-id` (or env vars `GOOGLE_ADS_LOGIN_CUSTOMER_ID` / `GOOGLE_ADS_CUSTOMER_ID`) override the provider block |
| `plan`     | partial | Diff `.bid` vs live, validateOnly batch via googleAds:mutate; emits `+ create` / `~ update` / `~ adopt` / `- destroy` / `no-op` per resource. Campaigns and ad groups match by their `bidsmith:address` label first, then by content (name) to adopt an unlabeled live resource; ads match by body; keywords by text. `- destroy` rows are orphaned criteria members **and** whole labeled resources (campaign / ad_group / ad_group_ad) dropped from the `.bid`; an unlabeled UI-created resource is never destroyed. `~ adopt` rows are first-run label writes onto an already-matching resource. Operations the account can never accept are caught locally, before anything is sent (issue #116): a create or update on the read-only VIDEO channel **blocks** the plan (exit `1`, nothing submitted), while a removal of a labeled VIDEO resource the file no longer declares is **skipped** with a warning and counted as `N to destroy (M skipped)`. A rejected batch separates operations that drew their own error (`rejected`) from those that only went down with the atomic batch (`blocked by those failures`). Reuses cached SearchStream batches from `.bidsmith/cache/` when fresh (15-min TTL); `--refresh-state` forces a re-pull; `--offline` skips OAuth and the validateOnly mutate, diffing against the cache only (errors if no fresh cache). `--var NAME=VALUE` (repeatable) and `BIDSMITH_VAR_<name>` env vars supply values for `variable` blocks. `--format markdown` renders the diff as a PR-comment table (`Resource \| Action \| Result`) instead of the default aligned `text` listing; `--detailed-exitcode` makes a non-empty diff exit `2` (terraform-style) while keeping `1` for errors, so CI can distinguish "changes pending" from "plan failed" |
| `apply`    | partial | Shows the validateOnly diff first, then prompts for `yes` (or skips the prompt with `--auto-approve`) before mutating. Refuses to prompt when stdin is not a TTY. Reuses the same cached live state as `plan`; invalidates the cache after a successful real mutate. Executes `- destroy` removes (orphaned criteria members and whole labeled resources) through the same prompt — no separate `--allow-destroy` flag. Writes `bidsmith:address=…` identity labels on created / adopted campaigns, ad groups, and ads (reusing an existing label by name) and reconciles stale associations on rename. Same `--var` / `BIDSMITH_VAR_<name>` plumbing as `plan` |
| `pull`     | partial | Dump live state as raw SearchStream JSON (`-o PATH` or stdout). Reuses the same query list `plan --read-live` issues; output is the exact shape `export --from-gads-search-response` consumes, so the pair round-trips an account into a `.bid` |
| `refresh`  | partial | Bootstrap-mode import of live state into `.bid` (no `-o`/`-d` → stdout, `-o PATH` → single file, `-d DIR` → split into `<DIR>/account.bid` for conversion actions / call assets / customer assets / shared sets and `<DIR>/campaigns.bid` for everything campaign-scoped). Shares the `export` renderer, so it emits the same **folded** form (issue #57): repeated ad bodies → `ad_template`, repeated RSA arrays and shared campaign negative lists → `locals`. Folding is source-only and round-trips identically, so a re-`refresh` no longer re-explodes a hand-folded tree. `--in-place` is reconcile mode: label-first matching writes drifted scalars back into the files you maintain (`--check` previews), loading the tree through the same `Program` path `validate` / `plan` use so `module` templates resolve as instance scopes, with the same `--var` / `BIDSMITH_VAR_<name>` plumbing |
| `query`    | partial | Read-only GAQL passthrough; `--format table` (default), `json`, or `tsv`; uses the same OAuth + customer envelope as `plan` / `apply` |
| `keyword-ideas` | partial | Read-only Keyword Planner research (`KeywordPlanIdeaService.GenerateKeywordIdeas`). Takes seed keywords and/or a landing-page `--url`, plus repeatable `--location` and a `--language` (same human-readable codes as a campaign's `locations` / `languages`, resolved via `src/targeting.rs`), and returns related keywords with average monthly searches, competition, and top-of-page bid estimates. `--format table` (default) / `json` / `tsv`, `--limit N` (most-searched first, `0` = all). Not a declarative resource — no `.bid`, no schema entry, no plan/apply; the imperative-research analog of `query`. Same OAuth + customer envelope as `plan` / `apply` |
| `schema`   | partial | Dump the resource + provider schema as JSON (`-o PATH` or stdout). Powers the docs site's auto-generated reference under `website/src/content/docs/resources/`; `website/src/data/schema.json` is a build artifact regenerated by the docs site's `prebuild` / `predev` npm scripts, so it cannot drift from `src/schema.rs` |
| `design-doc` | working | Generate the Google Ads API Basic-Access design document for an applicant to attach to their application. Two subcommands: `init` writes a commented `design-doc.toml` template; `render` reads the filled-in TOML plus bidsmith's own internals (API version, GAQL query list, RMF mapping) and emits `design-doc.html` for the user to print to PDF |
| `auth`     | working | Sign in to Google Ads and manage saved credentials. `login` runs a browser OAuth loopback + PKCE flow, then writes `~/.bidsmith/credentials.toml` (`0600`) — prompts for the developer token + MCC id when not passed, and ends by listing the accounts `listAccessibleCustomers` returns; `status` shows which credentials resolve and verifies them live; `logout` clears the sign-in (keeps the developer-token + MCC "team profile" unless `--all`); `profile` emits that shareable team blob. Uses the bundled OAuth client when present, else `--client-id`/`--client-secret` |
| `init`     | working | Scaffold a GitOps project skeleton into a directory (default `.`): a fmt-canonical starter `campaigns.bid` (everything `PAUSED`), a `bidsmith.toml` for the account ids, a `.github/workflows/bidsmith.yml` (plan on PRs → sticky comment, apply on merge to `main`), `.gitignore`, and a README setup checklist. Per-file idempotent — an existing file is reported and skipped unless `--force`. Templates live in `templates/init/` (`include_str!`'d) and are guarded offline by the CI checklist (`init` → `validate` → `fmt --check`) so the starter can't drift from the schema/formatter |
| `graph`    | —       | (later) Visualize resource graph                     |
| `import`   | —       | (later) Adopt an unlabeled existing resource         |

## References

- **Seed module**: `/Users/chrmod/Projects/github.com/chrmod/rezolutnie.com/ads/`
  - `ads/README.md` — operational runbook + known issues (read before
    designing auth / apply)
  - `ads/matrix.py` — data-shape inspiration (dataclasses, declensions)
  - `ads/copy.py` — content generation patterns
  - `ads/create.py` — atomic mutate, temp resource names trick (avoids
    the `criterion error UNKNOWN` race)
- **HCL2 spec**: https://github.com/hashicorp/hcl/blob/main/hclsyntax/spec.md
- **Google Ads API**: https://developers.google.com/google-ads/api/docs/start
- **`hcl-edit` docs**: https://docs.rs/hcl-edit
- **`miette` docs**: https://docs.rs/miette
- **`clap` docs**: https://docs.rs/clap
