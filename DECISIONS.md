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
  via `--refresh-state` (plan/apply), which a cache hit can never
  satisfy, or `BIDSMITH_NO_CACHE=1` for a full bypass. Cache directory
  is gitignored; token file is written mode `0600`. `apply` invalidates
  the live-state cache on its way *into* the mutate, not on the way out
  — once the request is in flight the snapshot is stale whatever comes
  back, and a lost response or a killed process must not leave one
  behind that still looks fresh.
- **State provenance is always reported** (issue #144): a diff is only
  as true as the snapshot it was computed from, and its rejections are
  checked against the *live* account, so a snapshot the account has
  moved past yields per-resource API errors indistinguishable from a
  genuinely bad `.bid`. Every path that loads live state — fresh fetch,
  cache hit, `--offline` — prints one line of the same shape naming the
  customer, the age, and the source; a plan that is not clean repeats it
  next to the rejections. Rejections whose error code or message says
  "already removed / missing / already present" are additionally called
  out as the shape of stale state, with a pointer to `--refresh-state`.
  The cache is per-project, so an `apply` from CI or another machine
  cannot invalidate a local one — that case is caught by the rejection
  shape, not by invalidation.
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
  overrides are linted like any RSA path. The Option B
  fan-out form remains a deferred follow-up. `variable` blocks
  stay scalar; this is a block-level reuse primitive, not a value one.
- **`ad_template` parameters** (issue #145): the URL-and-path overrides
  above cover the measured duplication but not an A/B pair, which differs
  in a headline or a tracking slug and otherwise duplicates the whole
  body. A template body may reference `input.<name>` anywhere an
  expression is allowed, and a use site binds them with
  `inputs = { name = value }`. A template's parameter list is exactly the
  `input.` names its body uses — declaring them separately would be a
  second list to keep in sync — so both a missing binding and a surplus
  one are validate-time errors naming what the template takes. Binding
  happens at import time like the overrides, so the mutate is identical
  to writing the body inline. Because a placeholder has no type at the
  declaration, `input.` expressions are skipped when the template body is
  validated and the *bound* body is validated at each use site instead —
  which is where a wrong value is actually fixable.
- **`final_url_suffix` / `custom_parameters`** (issue #145): both are
  native Google Ads fields on campaign, ad group, and ad, and neither
  was modelled — so a UTM convention lived inline in every `final_urls`
  string (measured: 63 UTM query strings across the tree, 7 inside one
  campaign file). Modelled on all three levels; `custom_parameters` is
  an HCL map rather than the API's repeated key/value message, sorted by
  name on both sides so a map — which has no inherent order — diffs
  deterministically. The update mask translates `custom_parameters` to
  the API's `url_custom_parameters`. On an ad the pair is **updatable**,
  unlike the creative: the API mutates it in place, and recreating an ad
  to change a UTM slug would discard its performance history for a
  string the visitor never sees. Omitted stays unmanaged, as everywhere
  else; a declared empty map is an explicit clear. Docs flag that this
  is a live-behaviour change rather than pure syntax — the suffix is
  appended by Google at click time and never appears in the display URL.
- **Asset attachment sugar** (issue #145): attaching an asset took three
  layers of ceremony — a `field_type` that is 1:1 derivable from the
  asset's resource type, one attachment resource per asset, and a
  one-attribute `callout_asset` whose whole content is its text
  (measured: ~130 lines in one campaign file, 33 `campaign_asset`
  resources across the tree). Three changes, cheapest first.
  `field_type` is now **optional and inferred** from the referenced
  asset (sitelink → SITELINK, callout → CALLOUT, structured_snippet →
  STRUCTURED_SNIPPET, call → CALL); a declared value still wins, and one
  that contradicts the asset is a validate-time error rather than an API
  rejection that sinks the batch. An attachment takes **`assets = [...]`**
  and fans out one attachment per entry, the precedent
  `keywords { texts = [...] }` set; a single `asset` keeps the
  resource's own address, so only adopting the list form re-addresses
  anything. A campaign declares **`callouts = [...]`** and
  **`structured_snippet { header, values }`** inline, and bidsmith
  synthesizes the asset plus its attachment; an **ad group takes the
  same two**, for extensions scoped to one keyword theme. Each level
  synthesizes its own asset, so the same text declared on a campaign and
  on one of its ad groups is two assets — which is what the account gets
  either way, since neither level can borrow the other's attachment.
  Shared assets keep the resource form — it is the one thing the inline
  spelling cannot say — and `export` / `refresh` fold only assets a
  single owner is the sole user of, leaving anything attached twice or
  to the account alone. All of this is address-level only: assets are
  matched by content, so adopting an account that already has them
  plans as a no-op.
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
  error pointing at the offending expression. A map value that is an
  object exposes its fields as `each.value.<field>`, so one entry can
  carry a whole record (a sitelink's text, url, and descriptions)
  rather than a single scalar; a field lookup on a scalar entry, or a
  name the entry does not have, is an error listing what it does have.
  A generated instance is referenceable by key —
  `google_ads_callout_asset.co[each.key].id` — because a string index
  folds into the segment it subscripts, matching the generated
  `co["howto"]` label verbatim. Together these mean an asset *and* its
  attachment can fan out from the same key set (issue #145). Adopting `for_each` for
  existing hand-written resources is live-neutral for content-matched
  types (criteria, assets, keywords); labelable types re-adopt via
  content fallback and show visible `~ adopt (label only)` rows as their
  `bidsmith:address` label moves to the keyed form. `refresh --in-place`
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
  (duplicate → error citing both files). **A module body sees the root
  tree's `defaults` blocks** (issue #148): a `defaults` block is a
  type-scoped shell, not a value, so factoring campaigns into
  `templates/` must not force the shell to be written out once per
  template — the duplication `defaults` exists to remove. The module's
  own block for the same `(type, name)` shadows the inherited one, a
  block declared inside a template stays private to it, and the
  inherited block is validated once at its declaration rather than once
  per instance. This is the one thing that crosses the boundary;
  `locals` and `variable` values still don't (those are the module's
  interface, and a module gets them through its inputs). The
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
  version pinned via `BIDSMITH_API_VERSION` env var (default `v25`);
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
  partitions keyword polarity (positive/negative) and criterion shape
  (location / language / proximity / audience / demographic / …) on both
  criterion resources, so declaring negatives never deletes positives a
  user manages in the UI, and declaring one axis never prunes another. Ownership of a category
  is claimed two ways: the file declares ≥1 member of it, or the live
  parent carries a `bidsmith:owns=<category>` label written by a
  previous apply (issue #88: without the persisted claim, removing the
  *last* declared member-resource of a category closed the gate and
  silently orphaned the live members). `apply` writes the claim
  association when a category gains its first declared member and
  releases it — in the same batch as the member destroys — when the
  category's last declared member goes away; `plan` shows the work as
  `~ claim (label only; claims negative keywords)` /
  `~ claim (label only; releases negative keywords)` rows.
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
  A `REMOVED` resource is likewise not something the file can be
  *matched* against, which is the same rule read from the other end
  (issue #161): Google auto-removes a non-shared budget with the last
  campaign that used it, and matching that corpse by name planned the
  declared budget as `unchanged`, so re-creating the campaign pointed at
  a dead resource name and the API refused the whole atomic batch. The
  `campaign_budget` query filters `status != 'REMOVED'` like every other
  resource query, and selects the status so a row that reaches the
  matcher can still be judged rather than trusted.
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
- **Demand Gen targeting level** (issue #168): Google fixes *where* a
  Demand Gen campaign's language/location targeting lives at creation —
  every new campaign defaults to ad-group-level ("upgraded") targeting,
  and `Campaign.demand_gen_campaign_settings.upgraded_targeting` (an
  immutable, create-only bool) is the opt-out that keeps it on the
  campaign. Targeting declared at the other level is rejected with an
  error code **published in no API version** (v22–v25 all decode it to
  "The error code is not in this version.", trigger
  `OWNED_AND_OPERATED`), so a version bump buys nothing and the guard
  has to live client-side. Modelled as a
  `demand_gen_campaign_settings { upgraded_targeting }` block on
  `google_ads_campaign`, with a **validate-time level check in both
  directions**: campaign-level `languages` / `locations` (or explicit
  positive language/location campaign criteria) on a Demand Gen campaign
  without `upgraded_targeting = false` is an error, and ad-group-level
  language/location criteria under a campaign that declared the opt-out
  is the same error from the other side. The field is create-only: the
  update mask never carries it, a declared-vs-live mismatch is a plan
  warning in the `campaign_immutable_warnings` family (fix the file or
  recreate the campaign), and `refresh` / `export` render the block only
  when live says `false` — `true` is Google's create-default and a block
  restating it would say nothing (the adapter still keeps both values,
  because the live-state diff needs them). Two rider fixes verified in
  the same probes: ad-group criterion creates **no longer pin a temp
  composite `resourceName`** — a constant-backed criterion (language,
  location, demographics, …) must carry the constant's own id there, so
  the pin was rejected as `INCONSISTENT_FIELD_VALUES` on every channel,
  not just Demand Gen — and plan rejections now append the error's
  `trigger` string, which for an unpublished error code is the only
  diagnosis Google sends. All verified live (`validateOnly` mutates
  against a dedicated test account): the opt-out plus campaign criteria
  is accepted on v22, the upgraded default plus ad-group criteria is
  accepted once the pin is gone, and each level is rejected under the
  other's setting.
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
- **`fmt` preserves comments** (issue #118): the canonical re-emitter
  carries every comment through, attached to the node the parser
  attached it to — leading comments stay above their block / attribute,
  end-of-line comments stay on their line, and a comment with nothing
  after it (before a closing brace, at end of file, inside a list) is
  re-emitted at that position. This matches `terraform fmt`, which is
  what anyone reading HCL expects. Before this, `fmt` silently deleted
  every comment, which pushed the *why* behind a number — a budget's
  `amount_micros`, a bid's rationale — out into a sibling `README.md`
  or a commit message, where it drifted out of sync with the value it
  explained. Placement is normalized, not byte-preserved: comments are
  re-indented to their node's depth, blank-line separation around a
  comment group is kept (one blank line max), and a multi-line
  `/* … */` keeps its internal shape via a dedent-and-reindent. Blank
  lines between comment-free attributes are still collapsed — that rule
  is unchanged. `fmt --minimal` **keeps** a default-valued attribute
  that carries a comment: dropping the line would delete the
  explanation with it, which is the same silent loss this closes.
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
  - **Removal is scoped to what the run read** (issue #160). Absence is
    the instruction, so it only means something against a complete input:
    a run that read *some* of the project's `.bid` files may only destroy
    resources whose address names a module it read. `apply one-file.bid`
    on a per-campaign tree otherwise reads "not declared here" as "not
    declared anywhere" — it destroyed five live campaigns and their
    budgets before the loop was stopped. Completeness is decided against
    the project root (the nearest ancestor with `bidsmith.toml`, else the
    target directory), so a whole-project run keeps pruning what a
    *deleted file* used to declare — the gesture that would otherwise
    have no way to be expressed. What a partial run skips is counted in
    the summary and named in a warning, because the failure this replaces
    was silent in the other direction.
  - **Writes**: `apply` emits a `Label` create (reusing an existing
    label by name — a duplicate name is an API error) plus the
    association op, wiring temp ids; a relabel also removes the stale
    association. First-run adoption of an already-matching resource
    surfaces as a visible `~ adopt (label only)` row and counts as a pending
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
- **`lifecycle { create = false }` — declarable adopt-only** (issue
  #115): a `resource` may carry a `lifecycle` meta-block whose one
  attribute, `create`, defaults to `true`. `create = false` says the
  resource must be adopted, never created; if the content match finds
  nothing live, `plan` raises a **blocker** naming the key it matched on
  (`by name "GH_YouTube_FR Instream 11.08.2026"`) and sends nothing,
  instead of degrading into a create. That degradation is the failure
  this closes: for a VIDEO campaign the create is refused by Google, and
  the atomic batch takes every unrelated operation down with it.
  - **A meta-block, not a resource attribute.** It says what bidsmith
    may do, not what the resource *is*, so it lives in no type's schema
    and is validated separately — which also keeps it off the
    auto-generated resource reference pages, where it would read as a
    Google Ads field.
  - **Rejected on criterion types** (`ad_group_criterion`,
    `campaign_criterion`, `shared_criterion`) rather than ignored:
    Google creates criteria freely, and one criterion resource can fan
    out into several live members, so `create = false` would have
    nothing single to point at.
  - **Only creation.** Drift on an adopted VIDEO campaign still raises
    the read-only-channel blocker; `create = false` is not a mute
    button. The two never double-report — the sharper adopt-only message
    replaces the channel one for the create case.
  - Declared-side only: `export` / `refresh` render from live state,
    where everything already exists, so they emit no `lifecycle` block.
    `fmt` (including `--minimal`) preserves a hand-written one.
  - `adopt_match = "name"` from the issue was **not** implemented — the
    match key is already label-first-then-name for every type, so the
    attribute would have exactly one legal value. The blocker message
    names the key instead, which is what the knob was for.
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
  the `AdGroupAd` with a `DemandGenVideoResponsiveAdInfo` pointing at it,
  in one atomic batch (the asset op is emitted ahead of the ad so the temp
  resource name resolves). `VideoResponsiveAdInfo` used to create the same
  way, but API v24 made `business_name` and `logo_images` Required on it
  and the `video_responsive_ad` block models neither, so that block is
  adopt-only now: a create is a `PlanBuildError` naming adoption, the same
  shape as `video_ad`. Modelling those two fields there was left undone
  deliberately — the VIDEO channel refuses every create and update anyway,
  so there is nothing to reopen. Assets are content-addressed, not
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
- **Demand Gen creatives apply, button and logo included** (issue #170):
  the same mutate path covers `demand_gen_video_responsive_ad`, and the
  three asset lists it carries — `videos`, `logo_images`,
  `call_to_actions` — are all references to declared resources, resolved
  together by `resolve_ad_assets` and emitted as `{"asset": rn}` (the
  shape `AdVideoAsset` / `AdImageAsset` / `AdCallToActionAsset` share).
  `business_name` and `logo_images` are REQUIRED by the API on that ad
  type but only *optional* in the schema — a required attribute would fail
  `validate` on every `.bid` written before they existed — so a create
  missing either is a `PlanBuildError` and `validate` warns. Because live
  CTAs and logos come back as asset ids that `asset_match` reconciles with
  the declared addresses, both are part of `ad_body_key`: an adopted
  Demand Gen ad matches on its whole creative, and swapping the button or
  the logo plans as a new ad the way any other body edit does. That has a
  one-time upgrade cost, taken deliberately: a `.bid` written before this
  landed describes an adopted Demand Gen ad without its logos, so the
  body no longer matches and `plan` stops on "needs logo_images to be
  created" rather than reporting a no-op. The alternative — keeping both
  out of the key, the way live-CTA-as-text forced before — buys a quiet
  plan at the price of a *silent* one: editing the logo or the button
  would do nothing at all. A file that cannot describe the whole creative
  cannot recreate the ad either, so completing the description (via
  `refresh -d` / `pull`, which now render both) is the state worth
  landing in.
  `call_to_actions` used to be a string list, which no create could ever
  use; typing it as a reference makes `call_to_actions = ["Install"]` a
  validate error naming the asset type instead of a plan-time surprise.
  Chosen over inventing implicit CALL_TO_ACTION assets at mutate time:
  every asset in bidsmith is an explicit declared resource.
- **Image assets are referenced, never uploaded** (issue #170): a
  `google_ads_image_asset` names an image that is *already in the
  account's asset library* — `name` (the asset library's label, and the
  match key) plus an optional `asset_id` that pins one image when several
  share a name. `apply` never creates one. `ImageAsset.data` is
  mutate-only: the API takes bytes on the way in and never hands them
  back, so a declared image has no content to match live state against and
  nothing stops a re-run from uploading the same logo again — the same
  no-idempotent-upload argument that keeps video files out (there is no
  `.tfstate`, and an asset carries no `bidsmith:address` label). The
  consequence is that a declared image with no live match is a **blocker**
  rather than a create: `plan` stops and names the upload step (Assets →
  Images) instead of emitting an asset op with no bytes in it, the same
  shape the adopt-only blocker takes. Duplicate names adopt the first and
  warn, like every other content-addressed asset. Image and
  call-to-action assets are deliberately absent from `ASSET_TYPES`, so no
  `customer_asset` / `campaign_asset` / `ad_group_asset` link can point at
  one: `field_type_for_asset` is 1:1 by design and an image can be a
  `LOGO`, a `MARKETING_IMAGE`, a `SQUARE_MARKETING_IMAGE`, … — the link
  side is a separate decision, not a lockstep obligation of this one.
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
- **A budget's amount is whichever one its `period` selects** (issue
  #131). `amount_micros` is a daily rate, `total_amount_micros` is a
  lifetime cap on a `period = "CUSTOM_PERIOD"` budget, and the API
  treats them as mutually exclusive — so `amount_micros` stopped being
  a schema-level required attribute and `validate` enforces the pairing
  instead (exactly one, matching the period). The diff compares only the
  selected one; the other is whatever the account happens to carry and
  writing it is an API error. `period` and `type` are **immutable**, so
  they are modelled and warned about but never diffed into an update —
  same treatment as a campaign's `advertising_channel_type`, and for the
  same reason: a file describing a daily budget that adopts a lifetime
  one would otherwise report a clean plan while Google ignored the
  amount it declared. `export` renders only the amount the period uses,
  since a live custom-period budget can still carry a stale
  `amount_micros` and rendering both produces a file `validate` rejects.
- **`network_settings` models every network, not most of them** (issue
  #132). A partially-modelled block is a worse failure than an absent
  one: the `.bid` carries what reads as a complete statement of where a
  campaign's money goes, and for the missing fields it is not. All six
  fields (`target_youtube` and `target_google_tv_network` included) come
  from one `NETWORK_SETTINGS_FIELDS` list in `src/schema.rs` that the
  schema, importer, adapter, differ, mutate builder, renderer, and
  `refresh` all iterate — a seventh field is one line, not seven. Each
  one is independently **unmanaged when omitted**, like `start_date` and
  `geo_target_type_setting`: an update mask naming a field the body
  leaves out is how Google Ads reads a *clear*, so modelling a network
  must never become a reason to switch it off on a campaign whose file
  never mentioned it. `drift` names blocks it reads in part on their own
  line, whether or not the missing fields carry a value today, because
  the gap is in the file's apparent meaning rather than in the values.
- **A video campaign's format is declarable** (issue #133). Two fields
  together say what a video campaign *is*, and neither was modelled:
  `advertising_channel_sub_type` (which variant — `VIDEO_NON_SKIPPABLE`
  and friends) and `video_campaign_settings.video_ad_inventory_control`
  (which YouTube inventory it may serve on — in-stream, in-feed, Shorts,
  non-skippable in-stream). For a repo running format experiments that
  is the one property the `.bid` most needs to pin down. The four
  inventory fields come from one `VIDEO_AD_INVENTORY_FIELDS` list in
  `src/schema.rs` that every stage iterates, and each is independently
  **unmanaged when omitted**, like `network_settings`. The sub-type is
  **immutable after create**, so it is treated like
  `advertising_channel_type` and the budget's `period` / `type`:
  compared but never updated, with a mismatch against the campaign a
  file adopted raised as a plan warning rather than reported as a clean
  match. Since a VIDEO campaign can only be adopted, inventory drift on
  one hits the existing channel blocker — the plan refuses rather than
  sending an update Google would reject. `video_campaign_settings` is
  modelled as a container holding only `video_ad_inventory_control`;
  `video_ad_sequence` and `video_ad_format_control` remain unread, and
  `drift` reports them as the ordinary unmodelled fields they are.
- **`targeting_setting` is managed as a whole list, and its defaults are
  never written down** (issue #135). A campaign or ad group declares
  `targeting_setting { target_restriction { targeting_dimension,
  bid_only } … }` to say, per dimension, whether its criteria restrict
  who is eligible (`bid_only = false`, "Targeting") or only inform
  bidding (`true`, "Observation"). Three choices worth keeping:
  - **The block owns the whole list**, unlike `network_settings`' field
    at a time, because that is the only update the API offers: "you must
    reconstruct and pass the entire `TargetingSetting` object back to
    Google Ads. Google assumes that any `target_restrictions` missing
    from the `TargetingSetting` should be removed"
    (developers.google.com/google-ads/api/docs/targeting/targeting-settings).
    Managing one dimension while claiming to leave the others alone would
    be a promise the wire format cannot keep. Omitting the block is still
    how a file leaves the field unmanaged.
  - **A restriction that says `bid_only = false` is dropped on both
    sides**, since the API reads a dimension nobody names as targeting
    anyway (`DEFAULT_BID_ONLY`). Google fills in entries nobody asked
    for, so without this every ad group would read back a block of
    boilerplate, and every plan would propose a write that changes
    nothing. An all-defaults live setting reads as *absent*.
  - **`KEYWORD` is not a declarable dimension.** Keywords always
    restrict, so the API rejects `bid_only = true` on them and `false`
    says nothing; live entries naming it (or any dimension this schema
    doesn't model) are dropped on read, like the geo `UNKNOWN` sentinel.

  Declaring the block at both levels is a **warning, not an error**:
  Google Ads refuses to *write* an ad group's setting while its campaign
  has one, but an account can carry both, so `export` has to be able to
  render what it read.
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
- **A video creative is adopted, never created — and the tracking URL is
  the reason to model it at all** (issue #136). `Ad.video_ad` is what a
  UI-built VIDEO campaign actually carries (`video_responsive_ad` is the
  responsive format, and the account's 21 in-stream ads use neither), so
  until it was modelled those ads could not be *declared* — which meant
  the `final_urls` they are measured on, UTM slug and all, existed only
  as a Google Ads UI setting: unversioned, unreviewed, and absent from
  every diff. `ad {}` therefore gained `display_url` and
  `final_mobile_urls`, `video_responsive_ad` gained `breadcrumb1` /
  `breadcrumb2`, and `video_ad { video }` became a fourth creative
  block. It models the video and nothing else: the format oneof
  (in-stream / bumper / in-feed) is not writable, so a block offering it
  would be the partially-modelled failure #132 is about.
  **Adoption works, creation does not**, and both halves are now
  enforced rather than rediscovered per campaign: `plan` blocks a
  creative it would have to create under a declared VIDEO campaign — the
  same check the campaign and ad group already had, extended to the ad,
  since an ad carries no channel of its own — and the mutate builder
  refuses a `video_ad` create outright with a message naming adoption as
  the path. Without those, the create reached Google, was refused, and
  took every unrelated operation in the atomic batch with it.
  The two URL fields join `ad_body_key` but deliberately *not* the
  creative-less carve-out, which still matches on `final_urls` alone
  unless the file names them: a `.bid` that declares no creative is not
  asserting anything about a `display_url` it never mentions, and
  tightening that would have re-planned every already-adopted UI-built
  ad as a create the channel refuses anyway.
- **A `resource_name` is an identity, not a setting** (issue #136). The
  discovery document leaves it writable, because that is how a mutate
  addresses an existing resource, so the `readOnly` filter alone did not
  catch it — and `ad_group_ad.ad.resource_name` came back as the
  single most-set "unmodelled field" on the account, burying the gaps
  that were real ones. `api::catalog` now drops it at any depth, which
  removes it from both sides of the coverage ratio rather than only from
  the sightings list: no `.bid` chooses a resource name, so counting it
  as a field bidsmith fails to model was never honest.
- **Bidding is one block, chosen from a fixed set** (issues #104, #134):
  `Campaign.campaign_bidding_strategy` is a protobuf `oneof`, so the
  campaign takes at most one of `manual_cpc`, `manual_cpm`,
  `manual_cpv`, `target_cpm`, `target_cpv`, `target_impression_share`,
  `target_spend` — declaring two is a validate error, and a
  `defaults "google_ads_campaign"` bidding block
  is suppressed wholesale (not per-name) once the resource picks its
  own, so a video campaign can opt out of a shared `manual_cpc`. The
  video strategies model nothing bidsmith writes — three are
  empty messages in the API and `target_cpm`'s one field
  (`target_frequency_goal`) is deferred — so their blocks carry no
  attributes: picking one is the whole declaration and the bid amount
  lives on the ad group, and the block is a *read* surface (see the
  VIDEO entry above). The search strategies are the ones bidsmith
  genuinely writes: `manual_cpc` (with `enhanced_cpc_enabled`),
  `target_impression_share` (`location`, `location_fraction_micros`,
  `cpc_bid_ceiling_micros` — all three required, matching the API) and
  `target_spend` (optional `cpc_bid_ceiling_micros`), so the CPC
  ceiling that decides what a search click costs is set in a reviewed
  file rather than the UI (issue #134). The two automated search
  strategies need no conversion tracking, which is what makes them
  modellable now. The empty video messages are unreadable through GAQL
  — there is no leaf field to select — so the
  live side derives them from `campaign.bidding_strategy_type`; the
  strategies with fields come off their own leaves, with
  `bidding_strategy_type` as the fallback when every field is unset and
  the API returns no message object at all (`manual_cpc` still comes
  off `manual_cpc.enhanced_cpc_enabled`,
  because enhanced CPC reports as `ENHANCED_CPC` rather than
  `MANUAL_CPC`). A strategy switch sends the desired member alone —
  setting one member of a `oneof` clears the others — but **masks it by
  its subfields, not by name** (issue #120): Google Ads refuses an
  update mask that names a message field carrying subfields, even when
  the operation leaves every one of them unset, so `manual_cpc` goes out
  as `manual_cpc.enhanced_cpc_enabled`, `target_cpm` as
  `target_cpm.target_frequency_goal`, and the two search strategies as
  the full list of their subfields. Only the field-less messages can be
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
- **Which locations a campaign targets and how they are read are two
  separate declarations** (issue #114): `geo_target_type_setting`
  models `positive_geo_target_type` / `negative_geo_target_type`, so a
  campaign can say it means people *in* a market rather than people
  anywhere who are interested in it. Google's default is the generous
  `PRESENCE_OR_INTEREST`, which silently confounds any geo-segmented
  test — the "DE test" and the "FR test" can both be partly serving the
  same third-country audience — and while the setting was unmodelled a
  UI flip could never surface in a plan. Each side is managed on its
  own and an omitted one is unmanaged, matching the rest of the
  campaign. The live side reports a type it has no value for as
  `UNKNOWN`, which is a report and not a setting: it maps to `None`, so
  adoption never renders a value the validator would reject or drift
  that no file could resolve. The deprecated `SEARCH_INTEREST` is not
  modelled.
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
- **A plan row says what it writes, and an update says what the value
  becomes** (issue #112): `~ update (name, status)` named the fields but
  not the change, and `~ adopt (label; +locations)` read like new
  targeting on a campaign that was already spending — when in fact an
  adopt / claim row writes bidsmith's own labels and nothing else. Both
  now spell it out. `Action::Update` carries a `FieldChange`
  (`field` / `live` / `desired`) per field instead of a bare name, so
  rows render `status: "PAUSED" -> "ENABLED"`; strings are quoted, an
  absent value reads `(unset)`, whole-set fields (`frequency_caps`,
  audience `members`) render their contents, and anything over 60 chars
  elides. `field_names()` supplies the `updateMask`, which is why the
  field string stays the raw API path. Claims are verbs
  (`claims locations` / `releases locations`) behind a `label only`
  marker, and a one-off note under the listing says that every field on
  such a row already matches live. The channel is the one declared
  field the diff can't reconcile (creation-only), so a file matching a
  live campaign on another channel now **warns** rather than reporting a
  clean adoption. Chosen over a per-field detail block under each row:
  a first-run adoption can carry hundreds of rows, and the answer a
  reviewer needs ("does merging this change what is serving?") fits on
  the row itself.
- **A schema gap is a reportable fact, not a silent one** (issue #111).
  An unmodelled field is not merely unmanaged, it is **undiffed**: `plan`
  never fetches it, never compares it, and counts the resource as
  `unchanged` — which reads to a reviewer as "the repo matches live" when
  the true statement is "the repo matches live on the fields bidsmith
  models". The failure is quiet and directional; it always reports *more*
  agreement than exists. Two changes make the gap visible, and they are
  deliberately different in kind. `plan` now prints one line under any
  summary that leans on the word (`` `unchanged` compares the fields
  bidsmith models. Run `bidsmith drift` … ``) — no API call, no count that
  could be wrong, just the scope of the claim it already made. And
  `bidsmith drift` answers the question that line raises, by asking the
  API rather than a table someone has to maintain: `GoogleAdsFieldService`
  for what each resource exposes, the public discovery document for which
  of those a mutate could write, `live_state::QUERIES` for what bidsmith
  reads. A bundled catalog was rejected outright — a stale one
  under-reports in exactly the direction this issue is about.
  The report is grouped **by field, not by resource**, and only names a
  field once it is actually set on something bidsmith manages: `campaign`
  alone carries ~90 unmodelled fields, most of them settings for channels
  the account doesn't run, so a per-resource dump would bury the two that
  govern a live auction under hundreds that govern nothing (`--all` for
  the full list). Output-only fields are excluded because the discovery
  document flags them, not because their names look derived — the
  `effective_*` / `primary_status` / `*_source` heuristic the issue
  suggested is wrong in both directions, and `campaign.bidding_strategy_type`
  (output-only, no such prefix) is the counterexample bidsmith already
  reads. A field the catalog offers but a `SELECT` refuses is *reported*
  rather than dropped, so the coverage ratio never quietly flatters
  itself. The verb is read-only and heavier than `plan` — one pass per
  batch of unmodelled fields — so it is a verb you run, not a flag on the
  hot path; the catalog caches for a week.
- **What an account *reports* is as declarable as what it serves**
  (issue #175): the four conversion-action fields that decide what lands
  in the Conversions column — `primary_for_goal`,
  `include_in_conversions_metric`, `phone_call_duration_seconds`, and
  `attribution_model_settings.attribution_model` — are modelled, read,
  diffed and written. None of them makes anything serve, which is how
  they stayed outside the "live state == `.bid`" goal of issue #153; but
  an auto-imported GA4 action at `category = PAGE_VIEW` and
  `primary_for_goal = true` puts pageviews in the column Smart Bidding
  optimises toward, so the number a budget conversation runs on stops
  measuring leads. `drift` already read all four — the gap was the schema
  and the write path. **No schema default is pinned on
  `primary_for_goal`** even though Google documents the create-default as
  `true`: pinning it would make an upgrade silently re-promote every
  action someone had deliberately demoted. Omitted means unmanaged, as
  everywhere else — and the same guard now covers `counting_type`, the
  lookback windows and `value_settings.*`, which until now diffed as a
  change whenever a file left them out and then sent an update mask
  naming a field the payload omitted. `validate` warns when a `PAGE_VIEW`
  action is primary *or* says nothing, since Google's default is primary
  and silence is the reported bug. The retired heuristic attribution
  models stay in the enum: an account can still report one, and refusing
  it would break the round-trip on exactly the accounts that need it.
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
  a `~ claim (label only; claims frequency caps)` row); dropping the
  last block on a claimed campaign plans the clear and releases the
  claim. A campaign
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
- **Grouped audiences are their own resource, in the atomic batch**
  (issue #169): a Demand Gen ad group runs in *grouped-audience* mode, where
  Google refuses a segment attached straight to it ("Audience segment
  attachment is not allowed when use audience grouped bit is set to true")
  and targeting has to go through the unified `Audience` resource.
  Modelled as `google_ads_audience`, the fourth alternative in the
  criterion `audience {}` block — `audience { audience =
  google_ads_audience.<name>.id }` — which is exactly the extension the
  issue-#105 entry above anticipated. **Unlike a custom audience it rides
  the unified batch**: `MutateOperation` *does* have an
  `audience_operation` member, so a new audience and the criterion
  attaching it commit in one transaction through a temp resource name, with
  no second call and no cross-call `deferred` row. The one inherited wait is
  a `segment { custom_audience = … }` pointing at an audience the
  pre-batch service has not created yet — under `validateOnly` that has no
  name either, so the grouped audience defers with the criterion that
  targets it. Matched live **by name**, like `custom_audience` and
  `shared_set` (audiences carry no labels), and **never destroyed**:
  `AudienceOperation` has no `remove` and `status` is output-only, so an
  audience dropped from the file is left alone.
  **Shape.** Google's `dimensions` is a repeated oneof, one entry per axis,
  which flattens with no loss: `segment {}` blocks (exactly one of
  `user_interest` / `user_list` / `life_event` / `detailed_demographic` /
  `custom_audience`) collect into the single `audience_segments` dimension,
  and `age_ranges` / `genders` / `parental_statuses` / `income_ranges` are
  list attributes standing for one demographic dimension each. Axes
  intersect, values within an axis are alternatives. Each is one repeated
  API field, so each diffs as a whole set and the update mask names only
  the two paths an audience has (`dimensions`, `exclusion_dimension`) — the
  same whole-set rule `frequency_caps` and custom-audience `members`
  follow. `excluded_user_lists` is the `exclusion_dimension`, user lists
  only, because the API has no other exclusion segment. **Demographics
  speak the criterion's vocabulary, not the API's**: `AgeDimension` counts
  in years, every other age field in Google Ads is the `AGE_RANGE_*` enum,
  and the bands map one-to-one — so `.bid` files use the enum in both
  places (`src/schema::AGE_RANGE_BOUNDS` is the single source of truth for
  the translation) and `AGE_RANGE_UNDETERMINED` / `UNDETERMINED` /
  `INCOME_RANGE_UNDETERMINED` in a list *is* the dimension's
  `include_undetermined` flag rather than a list entry. `scope` and
  `asset_group` are deliberately absent — `ASSET_GROUP` scope means
  something only for a Performance Max asset group, which bidsmith does not
  model, so every audience is account-scoped.
  **The ad group's half.** `AdGroup.audience_setting.use_audience_grouped`
  is modelled too, because Google's docs are explicit that audience
  targeting *fails* without it and it is **immutable after creation**:
  `validate` errors when a criterion targets an audience under an ad group
  that does not declare `use_audience_grouped = true` (the file cannot see
  the live value, and getting it wrong is a rejected apply), the mutate
  builder sends it on creates only, and a declared-vs-live mismatch is a
  plan warning in the same immutable-field family as
  `advertising_channel_type` / `upgraded_targeting` — the only fix is a new
  ad group. Direct segment / demographic criteria on a Demand Gen ad group
  are a **warning, not an error**: `use_audience_grouped` is per ad group
  and invisible from the file, so an older Demand Gen ad group created
  outside grouped mode still accepts them. Exclusions (`negative = true`)
  are left alone — an exclusion is not an attachment.
  **Not the cause of the demographics half of the issue.** The reported
  `age_range` rejection ("The field's contents don't match another field
  that represents the same data") was `INCONSISTENT_FIELD_VALUES` from the
  temp composite `resourceName` ad-group criterion creates used to pin —
  already fixed by issue #168, on every channel, before this work.
  **All verified live** (`validateOnly` mutates against a live account):
  the whole chain — budget, Demand Gen campaign, ad group with
  `use_audience_grouped = true`, audience, and the criterion attaching it
  — is accepted in one batch with nothing rejected, which is the proof
  that `audience_operation` rides `GoogleAdsService.Mutate` *and* that the
  criterion resolves the audience's temp resource name. `Audience` is the
  first resource bidsmith creates through a service-specific operation
  inside the unified batch rather than a call of its own.
- **Segment constants are customer-scoped, not global** (issue #169): the
  `UserInterest`, `LifeEvent`, and `DetailedDemographic` resources are
  named `customers/{customer_id}/userInterests/{id}` (and `/lifeEvents/`,
  `/detailedDemographics/`) — there is **no** `userInterestConstants`
  resource in any API version, and passing one is rejected with "Resource
  name '…' is malformed". Only `topicConstants/{id}`,
  `geoTargetConstants/{id}`, and `languageConstants/{id}` are genuinely
  account-free, which is why the three of them read differently from every
  other targeting constant. This had been wrong since the `user_interest`
  criterion shipped — in its reference page, the video-audience recipe,
  and the test fixtures, none of which the offline tests could catch
  because the value is an opaque string to everything but Google. Found by
  the live probe for the grouped audience and corrected everywhere.
- **Demand Gen refuses `target_cpm`** (issue #169): a Demand Gen campaign
  create carrying it comes back "The operation is not allowed for the
  given context." naming no field; live Demand Gen campaigns bid with
  `TARGET_SPEND` / `TARGET_CPC`, so `examples/demand-gen` uses
  `target_spend`. Recorded rather than validated client-side: the
  channel × strategy matrix is large, Google changes it, and a hard-coded
  table would go stale in a way a plan rejection does not. Related:
  `use_audience_grouped` is refused on a `SEARCH` ad group with
  `trigger: 'SEARCH'` — grouped mode is a Demand Gen / App notion, which
  is why the validate-time check keys on a criterion targeting an audience
  rather than on the channel.
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
- **Adoption writes source, not a label** (issue #150): bringing a
  pre-bidsmith resource under management is `bidsmith import <address>
  <resource-name>`, which declares it in a `.bid` file — **not** a mutate that
  stamps `bidsmith:address` onto the live resource. The API only accepts labels
  on campaigns, ad groups, ads, and ad-group criteria, and those four already
  adopt a content-matching live resource by themselves on the next `apply`. The
  types that actually strand an account with pre-bidsmith history — assets, the
  `customer_asset` / `campaign_asset` / `ad_group_asset` links, criteria — carry
  no label and never will, so for them "managed" can only mean "declared", and
  `plan` reaches them by content the moment a block exists. That makes `import`
  a source-writing sibling of `refresh --in-place` (same `Program` load, same
  re-validate-before-writing guard) rather than a new mutate path, and keeps the
  adoption itself reviewable as a diff in git instead of as an account write.
  Chosen over `refresh --in-place --adopt`, which would have had to guess which
  live resource a declaration meant; `import` names it outright, and the
  guessing that remains — content matching in `plan` — now reports when a
  declaration fits more than one live resource instead of silently taking one.
  Automatically-created assets are left out of the declared set (they belong to
  the automation settings, not to the file) and are a matter for prune.
- **Prune is scoped by ownership, not by a flag** (issue #151): a live asset
  link the file does not declare is destroyed — but only inside a
  `(parent, field type)` bidsmith owns, the same partition criteria already
  prune in. A campaign / ad group proves ownership exactly as it does for
  criteria: ≥1 declared link of that field type, or a
  `bidsmith:owns=asset_sitelink` (`…_callout`, `…_structured_snippet`,
  `…_call`) claim label from a previous apply — so dropping the *last* declared
  sitelink still prunes the live ones instead of stranding them (the #88 shape,
  in the asset half). Declaring sitelinks never touches callouts, and a campaign
  that declares neither behaves exactly as before. Account-level
  `customer_asset` links have nowhere to carry a claim (the API has no customer
  label bidsmith can write) and they attach to **every** campaign, so ownership
  there is declared in the file instead: `provider "google_ads" { owns =
  ["sitelinks", "callouts", "structured_snippets", "calls"] }`. Nothing
  account-wide is pruned until that list says so — declaring a `customer_asset`
  block is not by itself a claim, because the blast radius is the whole account
  and the claim has to be reviewable in the diff. Chosen over a
  `plan --prune` / `apply --prune` flag: a flag makes one tree mean two
  different things depending on how CI invoked it, which is the opposite of
  "the `.bid` is the source of truth". A link hanging off the read-only VIDEO
  channel is dropped before the batch rather than sent, because the batch is
  atomic and one doomed operation rejects every unrelated one with it
  (issue #116); it warns and counts into the `N to destroy (M skipped)` clause,
  as does the undeletable device criterion that already skipped silently.
  Assets themselves are never destroyed, only unlinked: the API has no remove
  on `AssetService`.
- **What Google's automation attached is paused, not destroyed** (issue #153).
  A link whose `source` is `AUTOMATICALLY_CREATED` is in scope on the same
  terms as any other, but ends `PAUSED` rather than removed, and is counted in
  its own `N to pause` clause. `source` is output-only, so such a link is not
  bidsmith's to recreate and Google reattaches what it made: a destroy would
  come back as the same destroy on the next plan and the file would never read
  as applied. A paused link stays where it is and stops serving, which is the
  whole of what "nothing serves that we did not declare" asks for, and it is a
  fixed point — a link already paused is not proposed again. Chosen over
  `AutomaticallyCreatedAssetRemovalService`, which despite the name removes only
  final-URL-expansion assets and so cannot reach the sitelinks, business name,
  or logo that this is actually about.
- **The claim over automation assets is named, not inferred** (issue #153).
  Everything else bidsmith owns is proved by a declaration; an automation asset
  cannot be declared at all, so within a `(parent, field type)` the file already
  owns — the campaign declares sitelinks, so its sitelinks are the file's —
  an automation link of that type is in scope with nothing more said. For the
  field types no block could ever declare (`BUSINESS_NAME`, `LOGO`, …) there is
  nothing to infer from, so the campaign says it outright:
  `owns = ["automatically_created_assets"]`, which also reaches its ad groups
  (Google picks the level it attaches to, and no ad-group block could have named
  one either). Account-wide links are the `provider` block's same token, kept
  separate because they reach every campaign at once. Until something claims
  them, they are reported on every plan and nothing is written.
- **Asset automation is declared per campaign; the account-level switch is
  reported, never written** (issue #152). "Automatically created assets" is two
  features wearing one name, and only one of them has an API. A campaign's own
  automation — text customization, final-URL expansion, and on Performance Max
  the image and video generators — is `Campaign.asset_automation_settings`, so
  it becomes an `asset_automation_settings` block whose five attributes are the
  `AssetAutomationType` values verified against a live account, each `OPTED_IN`
  / `OPTED_OUT`. (The enum carries ten; the rest read as ad-level in Google's
  own descriptions, and are preserved rather than declared — see below.) The account-level switch that invents dynamic sitelinks,
  callouts, a business name and a logo has **no field anywhere in v22** —
  not on `customer`, not on `campaign` — so no `.bid` can turn it off. Three
  choices follow:
  - **Compared per automation, written as a whole list.** Each attribute is
    independently unmanaged when omitted, like `network_settings` — Google
    reports a setting for every type it has an opinion about, so reading the
    unnamed ones as drift would make a block naming one of five propose the
    same write on every plan and never converge. The *write* is still the
    whole list, since that is all the API takes, so an automation the file
    does not name goes back to Google's default the moment a named one drifts.
    That is why both sides of the plan row render whole — and, alone among
    plan values, unelided: a reviewer is being asked to approve exactly what
    an elision would hide. An automation type *this build* has no attribute
    for is the sharper case, because there is no name to render it under and
    so no way for the row to show its loss at all. Those ride along with the
    write unchanged, read off the live campaign, and render under their API
    name; anything else would make declaring one automation quietly reset
    every automation newer than the binary.
  - **The unreachable half is reported until something claims it.** One warning
    counts the `AUTOMATICALLY_CREATED` links on campaigns and ad groups
    bidsmith manages that no ownership rule reaches, and names the claim that
    would reach them. The switch behind them is still not in the API, so it
    cannot be turned *off* from a `.bid` — but what it produces can be paused,
    which is what the reader actually wants, and the repeating warning is what
    catches someone flipping the switch back on in the UI.
  - **A block on the wrong channel is a lint, not a blocker.** Google Ads
    carries these settings on Search and Performance Max campaigns only;
    `validate` warns, and the API rejection stays the authority — the channel
    lists move with Google's product, and a hard local blocker on a moving list
    would refuse plans that the account would have accepted.
- **Dynamic search ads are declarable, and an undeclared live one is reported**
  (issue #159). DSA is the broadest of the "Google writes the ad" switches: it
  crawls the advertiser's own site and generates a headline and a landing page
  per search, and `use_supplied_urls_only = false` puts the whole site in scope.
  `dynamic_search_ads_setting { domain_name, language_code,
  use_supplied_urls_only? }` makes it a campaign setting like any other. Two
  choices:
  - **Both identifiers are required, by the API and so by the schema.** A domain
    with no language to read it in is not a scope, and half a message is an API
    rejection rather than a partial setting.
  - **Omitted still means unmanaged — but no longer means silent.** The issue
    asks for the absence of a block to *be* drift, which would invert the rule
    every other setting follows. What it gets instead is a warning naming the
    domain, on every plan, for a managed campaign whose live setting the file
    never mentions. That covers the risk the issue is actually about — a paused
    campaign nobody has looked at, where `unchanged` said nothing about what
    decides its copy — without making bidsmith clear a setting nobody declared.
- **AI Max is declared as ordinary scalar settings, one on the campaign and one
  on the ad group** (issue #158). AI Max broadens which queries a Search
  campaign matches and lets Google write creative for what it finds, which is
  the same fence `asset_automation_settings` puts up — but the API shape is not
  the same. `Campaign.ai_max_setting.enable_ai_max` and
  `AdGroup.ai_max_ad_group_setting.disable_search_term_matching` are plain
  booleans inside a message, not a repeated list, so each goes out under its own
  leaf path in the update mask and neither drags a sibling field with it. Three
  choices follow:
  - **Omitted stays unmanaged, as everywhere else.** The gap the issue reports
    is that the setting is *unset* on the campaigns that matter, so what AI Max
    does there is whatever Google's default is on a given day. Declaring the
    block is what pins it; a file that says nothing keeps saying nothing.
  - **`bundling_required` is not modelled.** The API marks it `readOnly` — it is
    Google's report on whether the campaign's AI Max features come as a set, not
    a switch anyone throws — so declaring it would send a field the API refuses.
  - **A block off its channel is a lint, for the same reason as above.** AI Max
    is a Search-campaign feature and the ad-group half applies to search ad
    groups; `validate` warns and the API stays the authority.

- **The mutate keeps the API's deadline, and stays one atomic batch**
  (issue #162). An apply of ~1300 operations failed two ways on
  consecutive runs — Google's own `DEADLINE_EXCEEDED` annotated onto
  every operation, and a client-side send failure — while batches of
  100–550 from the same account went through. Three choices:
  - **A mutate waits as long as Google will work.** The 60s read timeout
    applied to writes too, so bidsmith hung up minutes before the API's
    deadline on a batch the server was still willing to finish. Writes
    now use the method's own deadline, which means bidsmith never gives
    up first: if the work really is too much, the answer is Google's
    `DEADLINE_EXCEEDED` rather than an ambiguous local timeout.
  - **`validate_only` is also the retry policy.** A validate-only mutate
    commits nothing, so the pass `plan` makes — and the one `apply` runs
    before prompting — retries transient failures like any read. The
    real write still never retries: it is not idempotent, and a timed-out
    one may well have committed.
  - **No chunking.** Splitting the batch would trade the atomicity
    guarantee ("either every operation commits, or none") for a size
    limit, and a half-applied account is a worse failure than a refused
    one — particularly since a partial apply is exactly what an atomic
    batch exists to prevent. What a too-large batch gets instead is a
    plan line saying nothing was written and that applying one file at a
    time is the reliable way through, which the removal scoping from
    issue #160 now makes safe.

- **API upgraded v22 → v25 by verification, not migration guide.** Every
  field path the live-state queries SELECT, every enum value the schema
  or a GAQL WHERE names, every JSON key a mutate body writes, and every
  REST method bidsmith calls was checked against the v22 and v25 public
  discovery documents; two breaks surfaced and were absorbed at the API
  boundary. (1) v23 replaced `campaign.start_date` / `end_date` with
  `start_date_time` / `end_date_time` ("yyyy-MM-dd HH:mm:ss") — the
  `.bid` surface stays daily (`start_date = "2026-09-01"`): reads keep
  the date part (the open-ended sentinel is now `2037-12-30 23:59:59`),
  writes pin the time to the day edges (`00:00:00` / `23:59:59`) per the
  API's daily-granularity convention. (2) v24 made `business_name` and
  `logo_images` Required on `VideoResponsiveAdInfo`, so that creative is
  adopt-only now (see the YouTube video ads decision). Sub-daily flight
  times are deliberately not modelled: a marketer's flight is a date
  range, and a UI-set intraday time reads back as its date and never
  diffs.

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
│       ├── import.rs     # adopt one live resource into a chosen .bid address
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
  `OK: 5 file(s) valid.` — exercises `for_each` on a `module` block.
  `examples/modules-for-each/main.bid` instantiates
  `templates/preroll-campaign.bid` three times (one per `for_each`
  entry) with a shared `geo` and per-entry `campaign_name` / `final_url`;
  resources get addresses `ghostery_search.<key>.<type>.<name>`
  (`ghostery_search.privacy.google_ads_campaign.search`, …). The
  template's campaign shell comes from a `defaults.search_shell` block
  in `shared.bid` at the root, which is what a module inheriting the
  caller's `defaults` buys (issue #148).
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
- `cargo run -- validate examples/comments` → `OK: 1 file(s) valid.` —
  a campaign annotated the way the docs recommend (leading comments,
  end-of-line comments, a `/* … */` block, a `//` line, comments inside
  a `texts = [...]` list, and one dangling before a closing brace).
  Both `fmt --check examples/comments` and `fmt --minimal --check
  examples/comments` are no-ops, which is the regression guard for
  issue #118 — a formatter that dropped or moved a comment would fail
  the offline checklist.
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
  single-line form exceeds 80 chars or carry a comment, comments kept
  and re-indented to their node's depth).
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
- `bidsmith plan --read-live` prints the account currency plus
  per-resource-type counts for the customer (one
  `googleAds:searchStream` call per type bidsmith models, plus one for
  the `customer` row itself).
- Every `plan` / `apply` footer carries a `Budget:` line — committed
  daily spend before and after the changeset, the delta, and how many
  campaigns end up `ENABLED` (issue #117). Only budgets backing a
  post-apply `ENABLED` campaign count, a shared budget counts once, and
  the whole account is totalled, not just the declared slice — so three
  sibling PRs each adding EUR 20/day show the running total the third
  one would otherwise hide. Computed in `src/api/spend.rs` from the
  diff plus live state; amounts render in the account currency read by
  the `customer` GAQL query. Custom-period budgets commit a lifetime
  total rather than a rate, so they get their own continuation line
  (`plus 2 custom-period budgets totalling 140.00 EUR over their
  lifetime`) instead of being summed into a figure labelled `/day`
  (issue #131).
- `bidsmith plan examples/basic` against the rezolutnie account
  validates 8 CREATE operations on the live API and prints
  `8 accepted, 0 rejected (validateOnly)`. Proves every resource
  bidsmith models is API-faithful end-to-end.
- `bidsmith plan /tmp/w1.bid` against the rezolutnie account (where
  the campaign already exists) prints `Plan: 0 to create, 0 to
  update, 0 to destroy, 97 unchanged. (no API call needed)` once the .bid is
  in-sync with live. Editing any scalar in the file produces a
  single `~ update (field: was -> becomes)  ok` row + 96 no-ops on the
  next run.
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
  / `network_settings.*` / `geo_target_type_setting.*`, ad_group,
  ad_group_ad, conversion_action,
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
- `google_ads_campaign_budget` (`amount_micros` *or*
  `total_amount_micros`, whichever the immutable `period` selects, plus
  the immutable `type`), `google_ads_campaign` (one bidding
  block out of `manual_cpc` / `manual_cpm` / `manual_cpv` /
  `target_cpm` / `target_cpv` / `target_impression_share` /
  `target_spend`, plus `network_settings` and the required
  `contains_eu_political_advertising` enum — defaults to
  `DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING` at mutate time when the
  attribute is omitted, since Google Ads rejects new campaigns that
  don't declare it; plus optional `start_date` / `end_date` as
  `YYYY-MM-DD` dates validated as real calendar dates locally; plus
  optional inline `languages = [...]` /
  `locations = [...]` list attributes that each expand to one positive
  campaign criterion at import time, resolving human-readable codes
  (`"en"`, `"US"`) — or raw `languageConstants/NNNN` /
  `geoTargetConstants/NNNN` strings — to the API constants, plus an
  optional `geo_target_type_setting { positive_geo_target_type?,
  negative_geo_target_type? }` block deciding whether those locations
  mean presence or presence-or-interest, plus an optional immutable
  `advertising_channel_sub_type` naming which variant of the channel the
  campaign is, plus an optional `video_campaign_settings {
  video_ad_inventory_control { allow_in_stream?, allow_in_feed?,
  allow_shorts?, allow_non_skippable_in_stream? } }` block declaring
  which YouTube inventory it may serve on, plus an optional
  `asset_automation_settings { text_asset_automation?,
  final_url_expansion_text_asset_automation?,
  generate_image_enhancement?, generate_image_extraction?,
  generate_enhanced_youtube_videos? }` block (each `OPTED_IN` /
  `OPTED_OUT`) saying which assets Google may invent for the campaign,
  managed as a whole list once declared, plus an optional
  `ai_max_setting { enable_ai_max? }` block saying whether Google may
  broaden what the campaign matches and write creative for it, plus an
  optional `demand_gen_campaign_settings { upgraded_targeting }` block
  saying where a Demand Gen campaign's language/location targeting lives
  (`false` = campaign level, `true` = ad-group level, immutable after
  creation — see the Demand Gen targeting-level decision), plus an
  optional `dynamic_search_ads_setting { domain_name, language_code,
  use_supplied_urls_only? }` block naming the site Google may crawl to
  write the campaign's ads, plus a
  optional `owns = ["automatically_created_assets"]` claiming what
  Google's automation attached to the campaign and its ad groups (paused
  on apply, since such a link is not bidsmith's to recreate), plus a
  repeatable `frequency_caps { event_type, time_unit, time_length,
  cap, level? }` block managed as a whole set once declared, with a
  validate-time guard against declaring the same axis both inline and as
  an explicit positive criterion resource; plus a
  `targeting_setting { target_restriction { targeting_dimension,
  bid_only } … }` block saying whether each dimension restricts
  eligibility or only informs bidding), `google_ads_ad_group`
  (with every settable `AdGroup` bid field — `cpc_bid_micros`,
  `cpv_bid_micros`, `cpm_bid_micros`, `target_cpa_micros`,
  `target_cpm_micros`, `target_cpv_micros`, `percent_cpc_bid_micros`,
  `fixed_cpm_micros` — of which the campaign's strategy decides which
  one carries the bid, plus the same `targeting_setting` block the
  campaign carries, plus an optional `ai_max_ad_group_setting
  { disable_search_term_matching? }` block declaring the ad group's half
  of AI Max, plus an optional `audience_setting
  { use_audience_grouped? }` block — immutable, create-only — saying
  whether the ad group targets through a `google_ads_audience` rather
  than segment criteria of its own, which Demand Gen requires, plus an
  optional `demand_gen_ad_group_settings { channel_controls { … } }`
  block saying where a Demand Gen ad group's ads may serve —
  `channel_controls` is a `oneof`: either `channel_strategy`
  (`ALL_CHANNELS` / `ALL_OWNED_AND_OPERATED_CHANNELS`) or a
  `selected_channels { youtube_in_stream?, youtube_in_feed?,
  youtube_shorts?, gmail?, discover?, display?, maps? }` block with at
  least one channel on; validate rejects both arms at once, an all-false
  selection, and the block on a non-`DEMAND_GEN` campaign's ad group,
  while the output-only `channel_config` stays undeclarable and only
  disambiguates reads),
  `google_ads_ad_group_ad`
  (with `ad` → `responsive_search_ad` → repeating
  `headline { text, pin? }` / `description { text, pin? }` blocks,
  plus an equivalent list-attribute form `headlines = [...]` /
  `descriptions = [...]` whose items are either bare strings or
  `{ text, pin? }` object literals — both forms can coexist, and
  `final_urls` still uses `list<string>`; alongside it the `ad {}` body
  takes `final_mobile_urls` and `display_url`, and the creative may be a
  `video_responsive_ad` (with `breadcrumb1` / `breadcrumb2`), a
  `video_ad { video }`, or a `demand_gen_video_responsive_ad`; in place
  of an inline `ad {}`
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
  coexist in one resource. `fmt` does not fold between the forms; plus
  the non-keyword targeting axes from issue #110 — `audience`,
  `user_interest`, `youtube_channel`, `youtube_video`, `topic`,
  `placement { url }`, `age_range`, `gender`, `parental_status`,
  `income_range`, `location`, `language` — each usable as an exclusion
  via `negative = true` and bid-adjustable via `bid_modifier`. A
  criterion resource targets one thing, so mixing a keyword block with
  another axis is rejected at import. Ad-group `location` / `language`
  *intersect* with the campaign's own targeting rather than overriding
  it, which is what lets one campaign hold a cohort or a market per ad
  group instead of fanning out into one campaign each. The ad-group
  `audience {}` block takes a fourth alternative the campaign one has no
  API field for — `audience = google_ads_audience.<name>.id`, the grouped
  form Demand Gen requires),
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
  remove op that would sink the whole atomic batch; plus an
  `ad_schedule { day_of_week, start_hour, start_minute, end_hour,
  end_minute }` block for dayparting (issue #171) — minutes are the
  API's `ZERO` / `FIFTEEN` / `THIRTY` / `FORTY_FIVE` enums, hours plain
  numbers; all five fields are the match key, so editing a window plans
  as a create plus a prune of the old one, and `bid_modifier` rides
  along as the per-window bid adjustment; repeated `ad_schedule` blocks
  in one resource fan out to one criterion each, like `negative_keyword`
  blocks (issue #179 — they used to collapse silently to the last
  block, taking the campaign dark outside that one window); accepted by
  Google on the
  `DEMAND_GEN` channel as well as `SEARCH` — verified live via
  `validateOnly` mutates against a production account, since Google's
  own docs are silent on Demand Gen dayparting; plus the video
  targeting axes from issue #99 — `youtube_channel { channel_id }`,
  `youtube_video { video_id }`, `topic { topic_constant }`,
  `user_interest { user_interest_category }`, `age_range { type }`,
  `gender { type }`, and `audience { custom_audience | user_list |
  combined_audience }` taking exactly one of three — each usable as an
  exclusion via `negative = true`), `google_ads_custom_audience`
  (`name`, `description`, creation-only `type` = `AUTO` / `INTEREST` /
  `PURCHASE_INTENT` / `SEARCH`, `status`, and repeatable
  `member { keyword | url | place_category | app }` blocks managed as a
  whole set; matched to live by name like `shared_set`),
  `google_ads_audience` (the unified grouped audience: `name`,
  `description`, repeatable `segment { user_interest | user_list |
  life_event | detailed_demographic | custom_audience }` blocks taking
  exactly one of five, and `age_ranges` / `genders` /
  `parental_statuses` / `income_ranges` / `excluded_user_lists` list
  attributes — one `AudienceDimension` each, every one a whole set, with
  the axis's `UNDETERMINED` member standing for the API's
  `include_undetermined` flag; at least one dimension is required, and
  matched to live by name), plus a bulk
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
  (`type`, `category`, lookback windows, `primary_for_goal`,
  `include_in_conversions_metric`, `phone_call_duration_seconds`,
  optional `value_settings` sub-block with default value / currency /
  always-use flag, optional `attribution_model_settings` sub-block),
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
  `youtube_video_title`; the creative side of a `video_responsive_ad`),
  `google_ads_image_asset` (a reference to an image already in the
  account's asset library — `name` required, optional `asset_id` to pin
  one when several share a name; never created, see the locked decision
  above), `google_ads_call_to_action_asset` (the button on a Demand Gen
  creative — one `call_to_action` enum out of `LEARN_MORE` / `SHOP_NOW`
  / … , created like any other asset).
  The `ad {}` body also accepts a `video_responsive_ad` block (a
  `video` reference to a `google_ads_youtube_video_asset` plus optional
  `headlines` / `long_headlines` / `descriptions` / `call_to_actions`
  string lists) or a `demand_gen_video_responsive_ad` block (the ad type
  a `DEMAND_GEN` campaign carries — a `videos` list of
  `google_ads_youtube_video_asset` refs, a `logo_images` list of
  `google_ads_image_asset` refs, a `call_to_actions` list of
  `google_ads_call_to_action_asset` refs, plus optional `headlines` /
  `long_headlines` / `descriptions` / `breadcrumb1`
  / `breadcrumb2` / `business_name`) as an alternative to
  `responsive_search_ad` — an `ad`
  carries at most one creative, enforced at validate time. `pull` selects
  both video creatives and the `YOUTUBE_VIDEO` / `IMAGE` /
  `CALL_TO_ACTION` asset tables, so an
  existing video / Demand Gen campaign round-trips through `export`
  (headlines, long headlines, descriptions, breadcrumbs, business name,
  and every asset ref the creative carries)
- `provider "google_ads"` (`customer_id` optional — resolved from
  `bidsmith.toml` / env / global credentials when omitted, so `.bid`
  files can be account-agnostic; `login_customer_id` optional —
  overridable via `--login-customer-id` / `--customer-id` on `export`;
  `owns` optional — a list out of `sitelinks` / `callouts` /
  `structured_snippets` / `calls` naming the account-level asset kinds
  the tree owns exhaustively, plus `automatically_created_assets` for
  the account-level links Google attached, see the prune decision above)
- `lifecycle { create }` on any `resource` except the three criterion
  types — a meta-block belonging to no resource schema, validated
  separately; see the locked decision above
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
| `fmt`      | partial | Canonicalize `.bid` files (in-place; `--check` for CI). Comments survive: leading ones stay above their node, end-of-line ones stay on their line, dangling ones (before a closing brace, at end of file, inside a list) keep their position (issue #118). `--minimal` also strips optional attributes left at their schema default — the form `refresh` / `export` emit — while `always_emit` compliance fields and any attribute carrying a comment stay |
| `mv`       | working | Rename a resource address in source: rewrites the `resource` block label and every reference that resolves to it, across all `.bid` files under `--path` (default `.`). Addresses are `<type>.<name>`, or `<module>.<type>.<name>` to disambiguate a name shared across files. **Bulk mode** `--from-file <path>` (or `-` for stdin) renames a whole batch from a `<from> <to>`-per-line file (arrow optional, `#` comments) applied atomically against one snapshot — rejects missing sources, occupied targets, duplicate sources/targets, and rename chains (`a→b`,`b→c`); any bad rule writes nothing. Format-preserving (only the renamed identifiers change; comments and layout are byte-preserved). Refuses when the rename would raise the project's validation-error count above its pre-rename baseline (so it can still tidy a not-yet-fully-valid tree). **Source-only by design**: because the planner matches live resources by content (name / keyword / geo / …), not by address or label, an address rename is invisible to the account — no delete+create, no lost history or ad review. Once labels become identity (Phase 3 v2), a move will additionally rewrite the live `bidsmith:address` label; until then `mv` is the complete mechanism and `moved` blocks are deferred |
| `validate` | partial | Syntax + schema + references + lint warnings (local only). `--var NAME=VALUE` (repeatable) supplies values for `variable` blocks; `BIDSMITH_VAR_<name>` env vars are the fallback |
| `export`   | partial | Render a fmt-canonical `.bid` file from flat bidsmith JSON (`--from-json`) or raw Google Ads SearchStream JSON (`--from-gads-search-response`); always emits the compact form (one `google_ads_ad_group_criterion` per `(ad_group, match_type)` group with N `keyword {}` sub-blocks, one negatives resource per ad-group / campaign with N `negative_keyword {}` sub-blocks, RSAs as `headlines = [...]` / `descriptions = [...]` lists). Also **folds repeated structure** (issue #57): ad bodies shared across ≥ 2 ads become a top-level `ad_template` (URL-variant bodies collapse onto one URL-agnostic template + per-instance `final_urls` / `path1` / `path2` overrides), RSA arrays used by ≥ 2 sites and campaign negative lists shared by ≥ 2 campaigns become `locals`. Folding is source-only — the tree round-trips through `validate` / `plan` identically to the verbose form. Drops REMOVED resources unless `--include-removed`; `--login-customer-id` / `--customer-id` (or env vars `GOOGLE_ADS_LOGIN_CUSTOMER_ID` / `GOOGLE_ADS_CUSTOMER_ID`) override the provider block |
| `plan`     | partial | Diff `.bid` vs live, validateOnly batch via googleAds:mutate; emits `+ create` / `~ update` / `~ adopt` / `- destroy` / `no-op` per resource. An `~ update` row names each changed field with the value live holds and the value the file asserts (`status: "PAUSED" -> "ENABLED"`); `~ adopt` / `~ claim` rows are marked `label only` and followed by a note saying every field they declare already matches live (issue #112). Campaigns and ad groups match by their `bidsmith:address` label first, then by content (name) to adopt an unlabeled live resource; ads match by body; keywords by text. `- destroy` rows are orphaned criteria members, orphaned asset links inside a `(parent, field type)` the file owns (account-wide `customer_asset` links only when the `provider` block's `owns` list claims them), **and** whole labeled resources (campaign / ad_group / ad_group_ad) dropped from the `.bid`; an unlabeled UI-created resource is never destroyed. `~ pause` rows are asset links Google's automation attached inside an owned scope: they end `PAUSED` rather than removed (the API would only reattach them) and carry their own `N to pause` clause. `~ adopt` rows are first-run label writes onto an already-matching resource. Operations the account can never accept are caught locally, before anything is sent (issue #116): a create or update on the read-only VIDEO channel **blocks** the plan (exit `1`, nothing submitted), while a removal the API refuses is **skipped** with a warning and counted as `N to destroy (M skipped)` — a labeled VIDEO resource the file no longer declares, an asset link on a VIDEO campaign, or an undeclared device criterion. A rejected batch separates operations that drew their own error (`rejected`) from those that only went down with the atomic batch (`blocked by those failures`). Reuses cached SearchStream batches from `.bidsmith/cache/` when fresh (15-min TTL); `--refresh-state` forces a re-pull; `--offline` skips OAuth and the validateOnly mutate, diffing against the cache only (errors if no fresh cache). `--var NAME=VALUE` (repeatable) and `BIDSMITH_VAR_<name>` env vars supply values for `variable` blocks. `--format markdown` renders the diff as a PR-comment table (`Resource \| Action \| Result`) instead of the default aligned `text` listing; `--detailed-exitcode` makes a non-empty diff exit `2` (terraform-style) while keeping `1` for errors, so CI can distinguish "changes pending" from "plan failed" |
| `apply`    | partial | Shows the validateOnly diff first, then prompts for `yes` (or skips the prompt with `--auto-approve`) before mutating. Refuses to prompt when stdin is not a TTY. Reuses the same cached live state as `plan`; invalidates the cache after a successful real mutate. Executes `- destroy` removes (orphaned criteria members and whole labeled resources) through the same prompt — no separate `--allow-destroy` flag. Writes `bidsmith:address=…` identity labels on created / adopted campaigns, ad groups, and ads (reusing an existing label by name) and reconciles stale associations on rename. Same `--var` / `BIDSMITH_VAR_<name>` plumbing as `plan` |
| `drift`    | working | Report the surface `plan` is silent about (issue #111). Asks `GoogleAdsFieldService` which fields each audited resource exposes, reads the public discovery document to keep only the ones a mutate could **write** (output-only fields carry `readOnly` there), subtracts every path `live_state::QUERIES` names in a `SELECT`, then reads the remainder off the account so an unmodelled field that is merely possible reads differently from one that is set on a campaign you are running. Audits exactly the resources `plan` makes a claim about — a row it would create has nothing live to audit. Rows are grouped by field (`campaign.tracking_url_template  3 resource(s)  e.g. …`), not by resource, because the question is which *settings* fall outside the guarantee. `--all` also lists unmodelled fields nothing has set; `--format markdown` renders a PR-comment table; `--detailed-exitcode` exits `2` when an unmodelled field carries a value. The field catalog caches for 7 days (`--refresh-catalog`) |
| `pull`     | partial | Dump live state as raw SearchStream JSON (`-o PATH` or stdout). Reuses the same query list `plan --read-live` issues; output is the exact shape `export --from-gads-search-response` consumes, so the pair round-trips an account into a `.bid` |
| `refresh`  | partial | Bootstrap-mode import of live state into `.bid` (no `-o`/`-d` → stdout, `-o PATH` → single file, `-d DIR` → split into `<DIR>/account.bid` for conversion actions / call assets / customer assets / shared sets and `<DIR>/campaigns.bid` for everything campaign-scoped). Shares the `export` renderer, so it emits the same **folded** form (issue #57): repeated ad bodies → `ad_template`, repeated RSA arrays and shared campaign negative lists → `locals`. Folding is source-only and round-trips identically, so a re-`refresh` no longer re-explodes a hand-folded tree. `--in-place` is reconcile mode: label-first matching writes drifted scalars back into the files you maintain (`--check` previews), loading the tree through the same `Program` path `validate` / `plan` use so `module` templates resolve as instance scopes, with the same `--var` / `BIDSMITH_VAR_<name>` plumbing |
| `query`    | partial | Read-only GAQL passthrough; `--format table` (default), `json`, or `tsv`; uses the same OAuth + customer envelope as `plan` / `apply` |
| `keyword-ideas` | partial | Read-only Keyword Planner research (`KeywordPlanIdeaService.GenerateKeywordIdeas`). Takes seed keywords and/or a landing-page `--url`, plus repeatable `--location` and a `--language` (same human-readable codes as a campaign's `locations` / `languages`, resolved via `src/targeting.rs`), and returns related keywords with average monthly searches, competition, and top-of-page bid estimates. `--format table` (default) / `json` / `tsv`, `--limit N` (most-searched first, `0` = all). Not a declarative resource — no `.bid`, no schema entry, no plan/apply; the imperative-research analog of `query`. Same OAuth + customer envelope as `plan` / `apply` |
| `schema`   | partial | Dump the resource + provider schema as JSON (`-o PATH` or stdout). Powers the docs site's auto-generated reference under `website/src/content/docs/resources/`; `website/src/data/schema.json` is a build artifact regenerated by the docs site's `prebuild` / `predev` npm scripts, so it cannot drift from `src/schema.rs` |
| `design-doc` | working | Generate the Google Ads API Basic-Access design document for an applicant to attach to their application. Two subcommands: `init` writes a commented `design-doc.toml` template; `render` reads the filled-in TOML plus bidsmith's own internals (API version, GAQL query list, RMF mapping) and emits `design-doc.html` for the user to print to PDF |
| `auth`     | working | Sign in to Google Ads and manage saved credentials. `login` runs a browser OAuth loopback + PKCE flow, then writes `~/.bidsmith/credentials.toml` (`0600`) — prompts for the developer token + MCC id when not passed, and ends by listing the accounts `listAccessibleCustomers` returns; `status` shows which credentials resolve and verifies them live; `logout` clears the sign-in (keeps the developer-token + MCC "team profile" unless `--all`); `profile` emits that shareable team blob. Uses the bundled OAuth client when present, else `--client-id`/`--client-secret` |
| `init`     | working | Scaffold a GitOps project skeleton into a directory (default `.`): a fmt-canonical starter `campaigns.bid` (everything `PAUSED`), a `bidsmith.toml` for the account ids, a `.github/workflows/bidsmith.yml` (plan on PRs → sticky comment, apply on merge to `main`), `.gitignore`, and a README setup checklist. Per-file idempotent — an existing file is reported and skipped unless `--force`. Templates live in `templates/init/` (`include_str!`'d) and are guarded offline by the CI checklist (`init` → `validate` → `fmt --check`) so the starter can't drift from the schema/formatter |
| `import`   | working | Adopt one live resource into a `.bid` address you name (`bidsmith import <address> <resource-name-or-id>`). Scoped to the types Google Ads refuses to label — sitelink / callout / structured-snippet / call / YouTube-video assets, the `customer_asset` / `campaign_asset` / `ad_group_asset` links, and campaign / ad-group criteria — because the labelable kinds already adopt a content-matching live resource on the next `apply`. The address's module segment picks the file (`account.google_ads_customer_asset.x` → `account.bid`); an unqualified address is only allowed when the tree is a single file. Anything the block needs and the tree doesn't declare (the asset behind a link) is written alongside it with an export-style name; references point at existing declarations wherever the plan already matches one. Refuses an occupied address, a resource `plan` already manages, a resource name from the wrong collection, and a criterion whose parent isn't declared. `--check` prints the blocks without writing; `--offline` reads the cached snapshot; the mutated tree is re-validated before anything reaches disk |
| `graph`    | —       | (later) Visualize resource graph                     |

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
