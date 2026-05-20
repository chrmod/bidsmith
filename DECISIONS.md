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
  resources.
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
- **AI is outside the engine**: skills/agents author and review; engine
  is deterministic. Engine behavior must not depend on a model version.
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
- **Auth**: same env vars the rezolutnie `ads/` scripts already use
  (`GOOGLE_ADS_DEVELOPER_TOKEN`, `GOOGLE_ADS_CLIENT_ID`,
  `GOOGLE_ADS_CLIENT_SECRET`, `GOOGLE_ADS_REFRESH_TOKEN`,
  `GOOGLE_ADS_CUSTOMER_ID`, `GOOGLE_ADS_LOGIN_CUSTOMER_ID`). bidsmith
  exchanges the refresh token for an access token on every run; no
  local credential caching. `--customer-id` / `--login-customer-id`
  flags override the env when needed.
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
│   │   ├── client.rs     # reqwest::blocking wrapper; googleAds:mutate + :searchStream
│   │   ├── live_state.rs # six GAQL queries → populated ExportInput
│   │   ├── import.rs     # AST → ExportInput (round-trip with the renderer)
│   │   ├── diff.rs       # declared vs live → Create / NoOp / Update(fields)
│   │   └── mutate.rs     # ExportInput + DiffReport → googleAds:mutate body
│   └── commands/
│       ├── mod.rs        # module declarations + small shared helpers
│       ├── adapt.rs      # SearchStream JSON → ExportInput (used by export + live_state)
│       ├── apply.rs      # prepare + plan display + prompt + real mutate (--auto-approve skips the prompt)
│       ├── export.rs     # render .bid from a JSON source description
│       ├── fmt.rs        # canonical re-emitter (in-place / --check)
│       ├── plan.rs       # parse + validate + import + diff + validateOnly batch
│       └── validate.rs   # parse + validate orchestration
└── examples/
    ├── basic/main.bid          # provider, budget, campaign, ad group, ad, criteria
    ├── broken/
    │   ├── schema.bid          # schema/type/ref errors
    │   └── syntax.bid          # parse error
    ├── bulk/main.bid           # bulk keyword sub-blocks, shared sets,
    │                           # RSA headlines/descriptions list attributes
    ├── lint/
    │   └── warnings.bid        # valid syntax/schema but trips every lint rule
    ├── multi/                  # two campaigns in one dir with colliding bare
    │   ├── nadarzyn.bid        # criterion names — addresses disambiguate via
    │   └── warszawa.bid        # the file-stem module prefix
    ├── exports/
    │   ├── basic.json          # flat bidsmith input for `export --from-json`
    │   └── raw.json            # SearchStream-shaped input for `export --from-gads-search-response`
    └── trial/
        ├── README.md           # end-to-end runbook against the rezolutnie account
        └── dump_campaign.py    # Python helper that produces raw.json-shaped dumps
```

Verified locally:
- `cargo build` clean (no warnings)
- `cargo build --release` → ~4 MB binary (the jump from ~1.5 MB is
  the reqwest + rustls TLS stack added for the live API client)
- `cargo run -- validate examples/basic` → `OK: 1 file(s) valid.`
- `cargo run -- validate examples/multi` → `OK: 2 file(s) valid.` —
  both files declare `google_ads_campaign_criterion.broad_wikipedia`
  and `…broad_olx`; the file-stem module prefix
  (`nadarzyn.…` vs `warszawa.…`) makes the addresses unique.
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
- `bidsmith apply` shows the validateOnly diff first, then prompts
  `Apply these changes? Only 'yes' will be accepted to approve.`
  before flipping `validateOnly: false` and mutating. `--auto-approve`
  skips the prompt (required when stdin is not a TTY).

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
  import time),
  `google_ads_campaign_criterion` (single negative keyword, location,
  language, proximity with flat `latitude` / `longitude` in decimal
  degrees plus `radius` + `radius_units`; the adapter rounds to the
  API's micro-degree integers at the wire boundary), plus a bulk
  syntactic-sugar form where repeating `negative_keyword { text,
  match_type }` sub-blocks in one resource expand into N individual
  negative criteria at import time,
  `google_ads_shared_set` (named negative-keyword set with a bulk
  `negative_keyword { text, match_type }` sub-block form; type
  defaults to `NEGATIVE_KEYWORDS` at mutate time),
  `google_ads_campaign_shared_set` (links a `google_ads_shared_set`
  to a `google_ads_campaign`),
  `google_ads_conversion_action`
  (`type`, `category`, lookback windows, optional `value_settings`
  sub-block with default value / currency / always-use flag),
  `google_ads_call_asset` (country code + phone number, optional
  `call_conversion_reporting_state` and reference to a
  `google_ads_conversion_action`), `google_ads_customer_asset` (links
  a call asset to the account via `field_type = "CALL"`)
- `provider "google_ads"` (`customer_id` required, `login_customer_id`
  optional — overridable via `--login-customer-id` / `--customer-id` on
  `export`)
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
| `validate` | partial | Syntax + schema + references + lint warnings (local only) |
| `export`   | partial | Render a fmt-canonical `.bid` file from flat bidsmith JSON (`--from-json`) or raw Google Ads SearchStream JSON (`--from-gads-search-response`); drops REMOVED resources unless `--include-removed`; `--login-customer-id` / `--customer-id` (or env vars `GOOGLE_ADS_LOGIN_CUSTOMER_ID` / `GOOGLE_ADS_CUSTOMER_ID`) override the provider block |
| `plan`     | partial | Diff `.bid` vs live (name-matched, scalar-level), validateOnly batch via googleAds:mutate; emits `+ create` / `~ update` / `no-op` per resource |
| `apply`    | partial | Shows the validateOnly diff first, then prompts for `yes` (or skips the prompt with `--auto-approve`) before mutating. Refuses to prompt when stdin is not a TTY. Does not yet write `bidsmith:address=…` labels or detect removals (state-tracking is the v2 follow-up) |
| `refresh`  | stub    | Import live state into `.bid` files                  |
| `query`    | partial | Read-only GAQL passthrough; `--format table` (default), `json`, or `tsv`; uses the same OAuth + customer envelope as `plan` / `apply` |
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
