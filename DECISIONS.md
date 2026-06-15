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
  module instances of the same source file don't collide. v1
  limitations: single-file local sources only, no `for_each`, no
  outputs, no directory or GitHub sources, no nested modules (a
  module source containing its own `module` block fails fast). The
  larger pieces (`for_each`, `output`, directory + GitHub sources)
  are tracked under "Open decisions" as "Module composition v2."
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
  `validateOnly=false`. Mutates are sent in dependency order (budgets
  → campaigns → ad_groups → ads → criteria) inside one atomic batch.
  If validateOnly rejects anything, the real mutate is skipped.
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
│   │   ├── diff.rs       # declared vs live → Create / NoOp / Update(fields)
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
    ├── variable/main.bid       # `variable "x" { type, default }` plus var.<name>
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
- `cargo run -- validate examples/basic` → `OK: 1 file(s) valid.`
- `cargo run -- validate examples/multi` → `OK: 2 file(s) valid.` —
  both files declare `google_ads_campaign_criterion.broad_wikipedia`
  and `…broad_olx`; the file-stem module prefix
  (`nadarzyn.…` vs `warszawa.…`) makes the addresses unique.
- `cargo run -- validate examples/locals` → `OK: 1 file(s) valid.` —
  exercises the `locals { ... }` block plus `local.<name>` references
  for budget micros, default cpc, language constant, and proximity
  radius; `fmt --check examples/locals` is a no-op.
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
  update, 97 unchanged. (no API call needed)` once the .bid is
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
  no flag writes the concatenation to stdout. Bootstrap-only — it
  overwrites existing files; reconcile-in-place needs the Phase 3
  v2 labels first.
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
- `google_ads_campaign_budget`, `google_ads_campaign` (SEARCH with
  `manual_cpc` / `network_settings` and the required
  `contains_eu_political_advertising` enum — defaults to
  `DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING` at mutate time when the
  attribute is omitted, since Google Ads rejects new campaigns that
  don't declare it), `google_ads_ad_group`, `google_ads_ad_group_ad`
  (with `ad` → `responsive_search_ad` → repeating
  `headline { text, pin? }` / `description { text, pin? }` blocks,
  plus an equivalent list-attribute form `headlines = [...]` /
  `descriptions = [...]` whose items are either bare strings or
  `{ text, pin? }` object literals — both forms can coexist, and
  `final_urls` still uses `list<string>`),
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
  API's micro-degree integers at the wire boundary), plus a bulk
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
  round-trip), `google_ads_customer_asset` (links
  a call asset to the account via `field_type = "CALL"`)
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
  Afar — a near-universal typo for `1030`, Polish).

**CLI verbs**:

| Verb       | Status  | Purpose                                              |
|------------|---------|------------------------------------------------------|
| `fmt`      | partial | Canonicalize `.bid` files (in-place; `--check` for CI) |
| `mv`       | working | Rename a resource address in source: rewrites the `resource` block label and every reference that resolves to it, across all `.bid` files under `--path` (default `.`). Addresses are `<type>.<name>`, or `<module>.<type>.<name>` to disambiguate a name shared across files. **Bulk mode** `--from-file <path>` (or `-` for stdin) renames a whole batch from a `<from> <to>`-per-line file (arrow optional, `#` comments) applied atomically against one snapshot — rejects missing sources, occupied targets, duplicate sources/targets, and rename chains (`a→b`,`b→c`); any bad rule writes nothing. Format-preserving (only the renamed identifiers change; comments and layout are byte-preserved). Refuses when the rename would raise the project's validation-error count above its pre-rename baseline (so it can still tidy a not-yet-fully-valid tree). **Source-only by design**: because the planner matches live resources by content (name / keyword / geo / …), not by address or label, an address rename is invisible to the account — no delete+create, no lost history or ad review. Once labels become identity (Phase 3 v2), a move will additionally rewrite the live `bidsmith:address` label; until then `mv` is the complete mechanism and `moved` blocks are deferred |
| `validate` | partial | Syntax + schema + references + lint warnings (local only). `--var NAME=VALUE` (repeatable) supplies values for `variable` blocks; `BIDSMITH_VAR_<name>` env vars are the fallback |
| `export`   | partial | Render a fmt-canonical `.bid` file from flat bidsmith JSON (`--from-json`) or raw Google Ads SearchStream JSON (`--from-gads-search-response`); always emits the compact form (one `google_ads_ad_group_criterion` per `(ad_group, match_type)` group with N `keyword {}` sub-blocks, one negatives resource per ad-group / campaign with N `negative_keyword {}` sub-blocks, RSAs as `headlines = [...]` / `descriptions = [...]` lists); drops REMOVED resources unless `--include-removed`; `--login-customer-id` / `--customer-id` (or env vars `GOOGLE_ADS_LOGIN_CUSTOMER_ID` / `GOOGLE_ADS_CUSTOMER_ID`) override the provider block |
| `plan`     | partial | Diff `.bid` vs live (name-matched, scalar-level), validateOnly batch via googleAds:mutate; emits `+ create` / `~ update` / `no-op` per resource. Reuses cached SearchStream batches from `.bidsmith/cache/` when fresh (15-min TTL); `--refresh-state` forces a re-pull; `--offline` skips OAuth and the validateOnly mutate, diffing against the cache only (errors if no fresh cache). `--var NAME=VALUE` (repeatable) and `BIDSMITH_VAR_<name>` env vars supply values for `variable` blocks |
| `apply`    | partial | Shows the validateOnly diff first, then prompts for `yes` (or skips the prompt with `--auto-approve`) before mutating. Refuses to prompt when stdin is not a TTY. Reuses the same cached live state as `plan`; invalidates the cache after a successful real mutate. Does not yet write `bidsmith:address=…` labels or detect removals (state-tracking is the v2 follow-up). Same `--var` / `BIDSMITH_VAR_<name>` plumbing as `plan` |
| `pull`     | partial | Dump live state as raw SearchStream JSON (`-o PATH` or stdout). Reuses the same query list `plan --read-live` issues; output is the exact shape `export --from-gads-search-response` consumes, so the pair round-trips an account into a `.bid` |
| `refresh`  | partial | Bootstrap-mode import of live state into `.bid` (no `-o`/`-d` → stdout, `-o PATH` → single file, `-d DIR` → split into `<DIR>/account.bid` for conversion actions / call assets / customer assets / shared sets and `<DIR>/campaigns.bid` for everything campaign-scoped). Reconcile-in-place against existing `.bid` and label-based matching wait on the Phase 3 v2 label work |
| `query`    | partial | Read-only GAQL passthrough; `--format table` (default), `json`, or `tsv`; uses the same OAuth + customer envelope as `plan` / `apply` |
| `schema`   | partial | Dump the resource + provider schema as JSON (`-o PATH` or stdout). Powers the docs site's auto-generated reference under `website/src/content/docs/resources/`; `website/src/data/schema.json` is a build artifact regenerated by the docs site's `prebuild` / `predev` npm scripts, so it cannot drift from `src/schema.rs` |
| `design-doc` | working | Generate the Google Ads API Basic-Access design document for an applicant to attach to their application. Two subcommands: `init` writes a commented `design-doc.toml` template; `render` reads the filled-in TOML plus bidsmith's own internals (API version, GAQL query list, RMF mapping) and emits `design-doc.html` for the user to print to PDF |
| `auth`     | working | Sign in to Google Ads and manage saved credentials. `login` runs a browser OAuth loopback + PKCE flow, then writes `~/.bidsmith/credentials.toml` (`0600`) — prompts for the developer token + MCC id when not passed, and ends by listing the accounts `listAccessibleCustomers` returns; `status` shows which credentials resolve and verifies them live; `logout` clears the sign-in (keeps the developer-token + MCC "team profile" unless `--all`); `profile` emits that shareable team blob. Uses the bundled OAuth client when present, else `--client-id`/`--client-secret` |
| `init`     | —       | (later) Bootstrap project skeleton                   |
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
