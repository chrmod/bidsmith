# Bidsmith — Decisions

Settled choices and a snapshot of what currently exists. The
forward-looking plan lives in [ROADMAP.md](ROADMAP.md).

## Vision

Declarative, AI-friendly tooling for Google Ads campaigns. Think
**Terraform for Google Ads**: HCL2 config files, modules, validate /
plan / apply / refresh. The engine is deterministic; AI sits **on top**
— authoring `.bid` files, reviewing PRs, recommending optimizations.
Distribution: a Rust-compiled CLI binary (~1.3 MB release),
GitHub-native workflows for collaboration and continuous tuning.

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
- **AI is outside the engine**: skills/agents author and review; engine
  is deterministic. Engine behavior must not depend on a model version.
- **Plan = live validate**: use Google Ads API's `validate_only` flag
  for free server-side validation (auth, references, policy, length).
- **Apply requires `--confirm`**: codify the dry-run + `--confirm`
  pattern from `rezolutnie/ads`. Never auto-apply.
- **Module distribution**: Git refs
  (`source = "github.com/org/repo//path?ref=v1"`). No central registry
  in v1.
- **Single platform first**: Google Ads only. Meta / LinkedIn after a
  second installer exists.

## Current state

```
bidsmith/
├── .gitignore
├── Cargo.toml            # hcl-edit, miette, clap, thiserror, serde, serde_json
├── Cargo.lock
├── DECISIONS.md          # this file
├── ROADMAP.md            # forward-looking plan
├── README.md
├── src/
│   ├── main.rs           # clap dispatcher, subcommands
│   ├── parser.rs         # hcl-edit wrapper: parse_file → ParsedFile
│   ├── schema.rs         # resource-type registry + validator
│   ├── diagnostics.rs    # miette Diag type
│   └── commands/
│       ├── mod.rs        # shared stub helper
│       ├── export.rs     # render .bid from a JSON source description
│       └── validate.rs   # parse + validate orchestration
└── examples/
    ├── basic/main.bid          # provider, budget, campaign, ad group
    ├── broken/
    │   ├── schema.bid          # schema/type/ref errors
    │   └── syntax.bid          # parse error
    └── exports/
        └── basic.json          # input for `bidsmith export` (mirrors basic/main.bid)
```

Verified locally:
- `cargo build` clean (no warnings)
- `cargo build --release` → 1.3 MB binary
- `cargo run -- validate examples/basic` → `OK: 1 file(s) valid.`
- `cargo run -- validate examples/broken` → 8 source-mapped miette
  diagnostics (parse failure, type mismatch, enum violation, dangling
  reference, unknown attribute at two depths, unknown resource type,
  missing required field).
- `cargo run -- export --from-json examples/exports/basic.json`
  round-trips through `validate` cleanly (`-o out.bid` then
  `validate out.bid` → OK).

Validator covers (so far):
- `google_ads_campaign_budget`, `google_ads_campaign` (SEARCH with
  `manual_cpc` / `network_settings`), `google_ads_ad_group`
- `provider "google_ads"`
- Type system: `string`, `integer`, `bool`, `enum<…>`, `ref<targets>`
- Two-pass validation: collect addresses, then walk each block.

**CLI verbs**:

| Verb       | Status  | Purpose                                              |
|------------|---------|------------------------------------------------------|
| `fmt`      | stub    | Canonicalize `.bid` files                            |
| `validate` | partial | Syntax + schema + references + lint (local only)     |
| `export`   | partial | Render a `.bid` file from a JSON description of a campaign (testing/seed) |
| `plan`     | stub    | Diff `.bid` vs live, server-validated via API        |
| `apply`    | stub    | Execute mutates after `--confirm`                    |
| `refresh`  | stub    | Import live state into `.bid` files                  |
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
