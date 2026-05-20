# Bidsmith

Declarative tooling for Google Ads campaigns. Think Terraform, but pointed at
Google Ads instead of cloud infrastructure: HCL2 config files, schema
validation, plan/apply against the live account.

The engine is deterministic. AI sits **on top** — authoring `.bid` files,
reviewing PRs, recommending optimizations — and the engine's behavior never
depends on a model version.

## Status

Pre-alpha. Phase 1 (local parsing & validation) is partially done; nothing
talks to the Google Ads API yet.

| Verb       | Status  | What it does                                                  |
|------------|---------|---------------------------------------------------------------|
| `validate` | partial | Parse `.bid`, check schema and references                     |
| `export`   | partial | Render a `.bid` from a JSON description (testing aid)         |
| `fmt`      | stub    | Canonicalize `.bid` files                                     |
| `plan`     | stub    | Diff `.bid` vs. live, validate-only via API                   |
| `apply`    | partial | Show the plan, prompt for `yes`, then mutate (`--auto-approve` skips the prompt) |
| `refresh`  | stub    | Pull live state into `.bid`                                   |

Resource coverage today: `provider "google_ads"`,
`google_ads_campaign_budget`, `google_ads_campaign` (SEARCH with
`manual_cpc` / `network_settings`), `google_ads_ad_group`. See
[ROADMAP.md](ROADMAP.md) for what's queued next.

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
bidsmith fmt | plan | apply | refresh            # not yet implemented
```

`validate` walks a directory recursively for `.bid` files (or accepts a
single file). Parse errors and schema errors are batched and printed
together with source context.

`export` reads a JSON description of a campaign — see
[`examples/exports/basic.json`](examples/exports/basic.json) — and emits
the equivalent `.bid`. The renderer is the same one `refresh` will use
once the API client is in place; growing it now seeds the test corpus and
forces schema completeness.

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
