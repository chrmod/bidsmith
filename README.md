# Bidsmith

Declarative tooling for Google Ads campaigns. Think Terraform, but pointed at
Google Ads instead of cloud infrastructure: HCL2 config files, schema
validation, plan/apply against the live account.

The engine is deterministic. AI sits **on top** — authoring `.bid` files,
reviewing PRs, recommending optimizations — and the engine's behavior never
depends on a model version.

## Status

Pre-alpha. Local parsing/validation (Phase 1), live `plan`
(Phase 2), and `apply` (Phase 3, modulo labels) are landed; the live
client speaks Google Ads REST against a real account.

| Verb       | Status  | What it does                                                  |
|------------|---------|---------------------------------------------------------------|
| `validate` | partial | Parse `.bid`, check schema and references                     |
| `export`   | partial | Render a `.bid` from a JSON description or a SearchStream JSON dump |
| `fmt`      | partial | Canonicalize `.bid` files (in-place / `--check`)              |
| `plan`     | partial | Diff `.bid` vs. live, validate-only via API. Reuses `.bidsmith/cache/` (15-min TTL); `--refresh-state` busts it; `--offline` skips the network entirely |
| `apply`    | partial | Show the plan, prompt for `yes`, then mutate (`--auto-approve` skips the prompt). Invalidates the live-state cache on success |
| `pull`     | partial | Dump the live account as raw SearchStream JSON (round-trips through `export --from-gads-search-response`) |
| `refresh`  | partial | Bootstrap-pull live state into `.bid` (stdout, single `-o FILE`, or split `-d DIR` → `account.bid` + `campaigns.bid`) |
| `schema`   | partial | Dump the resource schema as JSON (drives the docs site's auto-generated reference) |

Resource coverage today: `provider "google_ads"`,
`google_ads_campaign_budget`, `google_ads_campaign` (SEARCH with
`manual_cpc` / `network_settings`), `google_ads_ad_group`,
`google_ads_ad_group_ad` (with `responsive_search_ad`, including a
list-attribute form for `headlines` / `descriptions`),
`google_ads_ad_group_criterion` (single keyword *or* bulk
`keyword {}` / `negative_keyword {}` sub-blocks),
`google_ads_campaign_criterion` (keyword / location / language /
proximity, plus bulk `negative_keyword {}` sub-blocks),
`google_ads_shared_set` and `google_ads_campaign_shared_set` for
reusable negative-keyword lists shared across campaigns,
`google_ads_conversion_action`, `google_ads_call_asset`,
`google_ads_customer_asset`. See [ROADMAP.md](ROADMAP.md) for
what's queued next.

## Example

A minimal `.bid` file:

```hcl
provider "google_ads" {
  customer_id       = "1234567890"
  login_customer_id = "9876543210"
}

resource "google_ads_campaign_budget" "summer" {
  name            = "Summer 2026"
  amount_micros   = 10000000
  delivery_method = "STANDARD"
}

resource "google_ads_campaign" "summer_search" {
  name                     = "Summer 2026 — Search"
  status                   = "PAUSED"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.summer.id

  manual_cpc {
    enhanced_cpc_enabled = false
  }
}
```

Validate it:

```sh
$ bidsmith validate examples/basic
OK: 1 file(s) valid.
```

When there's a problem, errors are source-mapped (`miette`-rendered):

```
  × invalid value "TURBO"; expected one of [STANDARD, ACCELERATED]
   ╭─[examples/broken/schema.bid:8:21]
 7 │   amount_micros   = "ten million"
 8 │   delivery_method = "TURBO"
   ·                     ───────
 9 │ }
   ╰────
```

### Compact forms for repetitive resources

Real campaigns accumulate dozens of keywords and negatives. Prefer the
bulk and list-attribute forms — they're equivalent to the one-resource-
per-keyword shape but a lot shorter:

```hcl
resource "google_ads_ad_group_criterion" "warszawa_phrases" {
  ad_group = google_ads_ad_group.warszawa.id
  status   = "ENABLED"

  keyword { text = "klimatyzacja Warszawa",          match_type = "PHRASE" }
  keyword { text = "montaż klimatyzacji Warszawa",   match_type = "PHRASE" }
  keyword { text = "klimatyzator inwerterowy Wwa",   match_type = "PHRASE" }
}

resource "google_ads_shared_set" "competitor_brands" {
  name   = "Klima — competitor brands"
  status = "ENABLED"

  negative_keyword { text = "samsung", match_type = "BROAD" }
  negative_keyword { text = "lg",      match_type = "BROAD" }
  negative_keyword { text = "daikin",  match_type = "BROAD" }
}

resource "google_ads_campaign_shared_set" "warszawa_brands" {
  campaign   = google_ads_campaign.warszawa.id
  shared_set = google_ads_shared_set.competitor_brands.id
}

resource "google_ads_ad_group_ad" "warszawa_rsa" {
  ad_group = google_ads_ad_group.warszawa.id

  ad {
    final_urls = ["https://example.com/warszawa/"]

    responsive_search_ad {
      headlines = [
        { text = "Klimatyzacja Warszawa", pin = "HEADLINE_1" },
        "Cicha praca, niski prąd",
        { text = "Bezpłatna wycena", pin = "HEADLINE_3" },
      ]

      descriptions = [
        "Montaż klimatyzacji split i multi w Warszawie.",
        "Działamy w Warszawie i okolicach.",
      ]
    }
  }
}
```

See [`examples/bulk/main.bid`](examples/bulk/main.bid) for the full
campaign — re-encoding a 1025-line one-resource-per-keyword campaign
in these forms gives 177 lines.

## Install

```sh
brew install chrmod/tap/bidsmith
```

The formula ad-hoc signs the binary at install time, so no Apple Developer
ID or notarization is needed. Linux (`x86_64` / `aarch64`) is built by the
same release pipeline.

## Claude Code skill

This repo doubles as a one-plugin Claude Code marketplace. From inside
Claude Code:

```
/plugin marketplace add chrmod/bidsmith
/plugin install bidsmith@bidsmith
```

Once installed, agents auto-discover the skill when the conversation
mentions bidsmith or edits `.bid` files. The skill covers the install
flow, the command set, the `.bid` file shape, and the prompt-before-apply
convention.

## Build

```sh
cargo build --release
./target/release/bidsmith --help
```

Release binary is ~1.3 MB. Requires Rust 1.89+.

## Commands

```sh
bidsmith validate [path]                         # file or directory; default "."
bidsmith export --from-json file.json [-o out]   # stdout if -o omitted
bidsmith pull -o dump.json                       # raw SearchStream JSON from live
bidsmith refresh -d ads-bid/                     # split account.bid + campaigns.bid from live
bidsmith fmt | plan | apply                      # see verbs table above
```

`validate` walks a directory recursively for `.bid` files (or accepts a
single file). Parse errors and schema errors are batched and printed
together with source context.

`export` reads a JSON description of a campaign — see
[`examples/exports/basic.json`](examples/exports/basic.json) — and emits
the equivalent `.bid`. The renderer is the same one `refresh` will use
once the API client is in place; growing it now seeds the test corpus and
forces schema completeness.

## Multiple files in one directory

Each `.bid` file's basename is its implicit module name. Two files in the
same directory can each declare `google_ads_campaign_criterion.broad_wikipedia`
without conflict — their fully-qualified addresses are
`nadarzyn.google_ads_campaign_criterion.broad_wikipedia` and
`warszawa.google_ads_campaign_criterion.broad_wikipedia`. References
inside a file resolve to the same module first, then fall back to a
global search; cross-module matches are accepted when exactly one
resource matches and rejected as `ambiguous reference` otherwise. See
[`examples/multi/`](examples/multi/) for a two-file example.

This makes the dump-per-campaign workflow work in one shot:

```sh
for campaign_id in $campaign_ids; do
  python -m ads.dump_campaign --campaign-id "$campaign_id" -o "/tmp/$campaign_id.json"
  bidsmith export --from-gads-search-response "/tmp/$campaign_id.json" \
    -o "ads-bid/$campaign_id.bid"
done
bidsmith validate ads-bid/
```

Shared negatives (`broad_wikipedia`, …) that appear in every campaign
no longer collide across files — they live in different modules.

## How it works

```
.bid files (HCL2)
   │
   ▼
parser  ─►  hcl-edit AST with source spans
   │
   ▼
schema  ─►  typed resource graph, references resolved
   │
   ▼
planner ─►  mutate ops sent with validate_only=true   (TBD)
   │
   ▼
applier ─►  GoogleAdsService.mutate                   (TBD)
```

State lives on Google Ads itself — managed resources will carry a
`bidsmith:address=<…>` label. There is no `.tfstate` file. `refresh` reads
labeled resources back into `.bid`; drift detection compares declared HCL
against live labeled state.

## Project layout

```
bidsmith/
├── Cargo.toml
├── DECISIONS.md            # settled choices + current state
├── ROADMAP.md              # forward-looking plan (speculative)
├── src/
│   ├── main.rs             # clap dispatcher
│   ├── parser.rs           # hcl-edit wrapper
│   ├── schema.rs           # resource registry + validator
│   ├── diagnostics.rs      # miette Diag type
│   └── commands/
│       ├── export.rs
│       └── validate.rs
└── examples/
    ├── basic/main.bid
    ├── broken/             # schema and syntax errors for testing
    ├── bulk/main.bid       # bulk-keyword + shared-set + RSA-list forms
    ├── lint/               # valid syntax that trips every lint rule
    ├── multi/              # two files in one dir with colliding bare
    │   ├── nadarzyn.bid    # criterion names; resolved via the
    │   └── warszawa.bid    # file-stem module prefix
    └── exports/basic.json  # input for `bidsmith export`
```

## Stack

- **Rust** (edition 2021, MSRV 1.89)
- **[hcl-edit](https://docs.rs/hcl-edit)** — HCL2 parser preserving source spans
- **[miette](https://docs.rs/miette)** — compiler-style error rendering
- **[clap](https://docs.rs/clap)** — CLI parsing
- **serde** / **serde_json** — `export` input

## Roadmap

See [DECISIONS.md](DECISIONS.md) for locked choices and current state,
and [ROADMAP.md](ROADMAP.md) for the six-phase plan (parsing → plan →
apply → refresh → modules → AI integration). The roadmap is speculative
— expected to be revised against real campaign requirements.

## References

- [HCL2 spec](https://github.com/hashicorp/hcl/blob/main/hclsyntax/spec.md)
- [Google Ads API](https://developers.google.com/google-ads/api/docs/start)

## License

MIT
