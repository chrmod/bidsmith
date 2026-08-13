---
name: bidsmith
description: Authoring, validating, and applying .bid files with the bidsmith CLI — declarative Google Ads (HCL2-syntax resources, plan/apply against the live account). Use when the user mentions bidsmith, edits or creates .bid files, asks to install or upgrade bidsmith, needs to validate/export/plan/apply Google Ads campaigns, wants to set up a Google Ads GitOps repo (CI that plans on pull requests and applies on merge), or wants campaign performance data (stats, metrics, search terms) from an account bidsmith manages. Covers the Homebrew install (`chrmod/tap/bidsmith`), `bidsmith init` project scaffolding, the .bid file shape, the prompt-before-apply convention, the GitHub Actions GitOps flow, and read-only GAQL reporting via `bidsmith query`.
allowed-tools: Bash, Read, Write, Edit
---

# bidsmith

A declarative Google Ads CLI. `.bid` files (HCL2 syntax, Terraform-like)
describe desired campaigns, budgets, and ad groups. `bidsmith` validates,
diffs (`plan`), and reconciles (`apply`) against the live account.

## Install / upgrade

On macOS or any normal shell, use Homebrew:

```sh
brew install chrmod/tap/bidsmith     # first time
brew upgrade chrmod/tap/bidsmith     # later
bidsmith --version                   # confirm
```

The Homebrew tap is `chrmod/homebrew-tap`. The formula ad-hoc signs the
binary on install (no Apple Developer ID involved).

### Running inside Cowork (Linux sandbox)

Cowork runs this skill in an **ephemeral Linux sandbox**, not on the
macOS host: `brew` is not the path there, and a host-installed `bidsmith`
is a macOS build that won't run in the sandbox. Three sandbox facts shape
the install:

- **Per-session**: the sandbox home is wiped between sessions, so an
  install into `~/.local/bin` does not survive. Only the user's **mounted
  folders** persist (e.g. `~/.bidsmith`, the campaigns repo).
- **Each bash call is independent**: `PATH`/env do not carry over, so
  every call must put `bidsmith` on `PATH` itself.
- **It is Linux** — usually `aarch64`, sometimes `x86_64`. Pick the
  matching release target.

**One-time install in a session** — download the matching release
tarball, verify its checksum, put it on `PATH`:

```sh
case "$(uname -m)" in
  aarch64|arm64) tgt=aarch64-unknown-linux-gnu ;;
  x86_64|amd64)  tgt=x86_64-unknown-linux-gnu  ;;
esac
url="https://github.com/chrmod/bidsmith/releases/latest/download/bidsmith-$tgt.tar.gz"
curl -fsSL "$url" -o /tmp/bidsmith.tar.gz
echo "$(curl -fsSL "$url.sha256" | awk '{print $1}')  /tmp/bidsmith.tar.gz" | sha256sum -c -
tar xzf /tmp/bidsmith.tar.gz -C /tmp
mkdir -p "$HOME/.local/bin"
install -m 0755 /tmp/bidsmith "$HOME/.local/bin/bidsmith"
export PATH="$HOME/.local/bin:$PATH"
bidsmith --version
```

`github.com` release-download URLs work; direct `curl` to
`api.github.com` may be proxy-blocked, so don't depend on it. If the
network is fully blocked, `cargo install --git
https://github.com/chrmod/bidsmith` is the last resort (needs Rust).
Never fall back to hand-editing HCL the validator would reject.

**Persist across sessions (recommended)** — cache the binary in a
**mounted** folder so later sessions skip the download. `~/.bidsmith`
already persists (it's where bidsmith reads `credentials.toml`), so it's
a natural home. The repo ships `scripts/cowork-bootstrap.sh`: copy it to
`~/.bidsmith/bin/bootstrap.sh` once, then **source it at the top of each
bash call** — it puts `bidsmith` on `PATH`, downloading only on a cache
miss:

```sh
source ~/.bidsmith/bin/bootstrap.sh
```

### Credentials in the sandbox

bidsmith reads `~/.bidsmith/credentials.toml` automatically. As long as
that folder is **mounted**, `bidsmith auth status`, read-only
`bidsmith query`, `plan`, and `apply` work with no extra setup — no
`source .env`. The browser `bidsmith auth login` flow **cannot run in the
sandbox**; mint/refresh the token on the host (or any machine with a
browser) and rely on the mounted `credentials.toml`.

If neither the mounted `~/.bidsmith/credentials.toml` nor `GOOGLE_ADS_*`
env vars are present, do **not** attempt a live command — author /
`validate` / `fmt` / `init` offline and open a pull request so CI runs
`plan` / `apply` (see "GitOps" below), or hand the live command to the
user. `bidsmith plan --whoami` exits `0` only when credentials resolve.

## Commands

```
bidsmith init [path]                            # scaffold a GitOps project (config, starter .bid, CI workflow)
bidsmith validate [path]                        # parse + schema-check .bid files
bidsmith fmt [path]                             # canonicalize formatting
bidsmith export --from-json input.json [-o out.bid]
bidsmith plan                                   # diff .bid vs. live Google Ads
bidsmith plan --format markdown --detailed-exitcode  # PR-comment table; exit 2 on a non-empty diff (CI)
bidsmith apply                                  # apply the plan; prompts unless --auto-approve
bidsmith drift                                  # settings plan never compares, and which are set live
bidsmith refresh [-d DIR | -o FILE]             # import live state into fresh .bid files
bidsmith import ADDRESS RESOURCE_NAME           # adopt one live resource into a chosen .bid address
bidsmith query "GAQL" [--format table|json|tsv] # read-only stats/reporting passthrough
bidsmith pull [-o FILE]                         # dump raw live state as SearchStream JSON
bidsmith auth <login|status|logout|profile>     # browser sign-in + credential management
bidsmith schema                                 # dump the resource schema as JSON
```

This list is a snapshot; the installed binary is the source of truth.
When this document and `bidsmith --help` disagree (new verbs, changed
flags), trust the binary: `bidsmith <command> --help` carries examples
and flag details, and `bidsmith schema` dumps every resource type and
attribute as JSON.

`validate` is the most-used verb during authoring. Errors are
source-mapped (miette) and point at the offending span — show them
verbatim to the user rather than paraphrasing.

## `.bid` file shape

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

Cross-references use `<type>.<name>.id`. Resources currently supported:
`google_ads_campaign_budget`, `google_ads_campaign` (SEARCH channel with
`manual_cpc` / `network_settings`), `google_ads_ad_group`,
`google_ads_ad_group_ad` (with `ad` → `responsive_search_ad` →
headline/description assets), `google_ads_ad_group_criterion`,
`google_ads_campaign_criterion` (keyword/location/language/proximity),
`google_ads_shared_set` + `google_ads_campaign_shared_set` (reusable
negative-keyword lists shared across campaigns),
`google_ads_conversion_action`, `google_ads_call_asset`,
`google_ads_customer_asset`.

`amount_micros` is the budget in millionths of the account currency
(`10000000` = 10 currency units). `status` for campaigns and ad groups:
`ENABLED` | `PAUSED` | `REMOVED`.

A budget that runs for a fixed flight says `period = "CUSTOM_PERIOD"`
and `total_amount_micros` (the whole run) instead of `amount_micros`
(per day) — exactly one of the two, and `period` / `type` can never be
changed once the budget exists.

## Compact forms for repetitive resources

A real campaign accumulates dozens of negative keywords and many RSA
assets. Prefer the bulk / list-attribute forms over one-resource-per-
keyword sprawl — they're semantically identical and validated /
formatted / planned the same way, but cut a 1000-line file by ~80%.

**Bulk ad-group keywords**: repeating `keyword {}` and
`negative_keyword {}` sub-blocks in one `google_ads_ad_group_criterion`
expand to N criteria at import time. Parent attrs (`status`,
`cpc_bid_micros`) apply to every expanded criterion.

```hcl
resource "google_ads_ad_group_criterion" "warszawa_phrase" {
  ad_group = google_ads_ad_group.warszawa.id
  status   = "ENABLED"

  keyword { text = "klimatyzacja Warszawa",        match_type = "PHRASE" }
  keyword { text = "klimatyzator inwerterowy Wwa", match_type = "PHRASE" }
}
```

**Bulk campaign negatives**: same shape on
`google_ads_campaign_criterion` (`negative_keyword {}` sub-blocks
only — positives don't live here).

**Shared negative-keyword sets**: define once with
`google_ads_shared_set` (bulk `negative_keyword {}` sub-blocks
inside), then attach to as many campaigns as needed with
`google_ads_campaign_shared_set`. Reuse beats per-campaign copy-paste
when the same competitor-brand or informational-query negatives apply
across cities/services.

```hcl
resource "google_ads_shared_set" "competitor_brands" {
  name   = "Klima — competitor brands"
  type   = "NEGATIVE_KEYWORDS"   # default; can omit
  status = "ENABLED"

  negative_keyword { text = "samsung", match_type = "BROAD" }
  negative_keyword { text = "lg",      match_type = "BROAD" }
  negative_keyword { text = "daikin",  match_type = "BROAD" }
}

resource "google_ads_campaign_shared_set" "warszawa_brands" {
  campaign   = google_ads_campaign.warszawa.id
  shared_set = google_ads_shared_set.competitor_brands.id
  status     = "ENABLED"
}
```

**List-attribute RSA assets**: `headlines = [...]` and
`descriptions = [...]` accept bare strings (un-pinned) or
`{ text, pin = "HEADLINE_1" }` object literals. Equivalent to the
verbose `headline {}` / `description {}` blocks — both forms can
coexist inside one `responsive_search_ad`. Use the list form when
the asset count is high.

```hcl
responsive_search_ad {
  headlines = [
    { text = "Klimatyzacja Warszawa",    pin = "HEADLINE_1" },
    { text = "Certyfikowany instalator", pin = "HEADLINE_2" },
    "Toshiba Mitsubishi Rotenso",
    "Cicha praca, niski prąd",
    { text = "Bezpłatna wycena", pin = "HEADLINE_3" },
  ]

  descriptions = [
    "Montaż klimatyzacji split i multi w Warszawie.",
    "Działamy w Warszawie i okolicach.",
  ]
}
```

See [`examples/bulk/main.bid`](../../examples/bulk/main.bid) for a full
campaign that uses all three compact forms.

## Files are modules

Each `.bid` file's basename is its implicit module name. The full
address of a resource is `<module>.<type>.<name>`. Two files in one
directory can each declare `google_ads_campaign_criterion.broad_wikipedia`
without conflict — they live in different modules
(`nadarzyn.google_ads_…` vs. `warszawa.google_ads_…`).

References inside a file resolve same-module first, then fall back to a
global search across all files:
- 0 matches → `reference to undeclared resource '<addr>'`.
- 1 match (in some other module) → resolved.
- 2+ matches → `ambiguous reference to '<addr>'; declared in modules
  [a, b] — rename one of the resources so each is unique within its
  module`.

Practical implications:
- Per-campaign dumps coexist in one directory with no manual
  namespacing. The typical loop is `for c in $campaigns; do bidsmith
  export --from-gads-search-response /tmp/$c.json -o
  ads-bid/$c.bid; done && bidsmith validate ads-bid/`.
- Account-scoped resources (`google_ads_conversion_action`,
  `google_ads_call_asset`, `google_ads_customer_asset`) belong in their
  own file and are referenced cross-module from each campaign file.
  Don't duplicate them per campaign — that creates ambiguity and a
  diff that wants to create N copies.
- `plan` / `apply` display strips the module prefix when every diff
  line is in the same module, so single-file projects keep their bare
  `<type>.<name>` UX.

## Authoring workflow

1. Edit `.bid` files.
2. `bidsmith fmt` (optional, keeps diffs clean).
3. `bidsmith validate` — fix any source-mapped errors before continuing.
4. `bidsmith plan` — review the diff against live state.
5. `bidsmith apply` — read the printed plan, type `yes` at the prompt.

Never run `bidsmith apply --auto-approve` without first showing the user
the `plan` output. State lives on Google Ads itself (no local `.tfstate`);
`plan` and `refresh` match live resources against `.bid` declarations by
name.

`plan` compares the fields bidsmith models, so an `unchanged` row is a
statement about those fields only — never describe a clean plan as "the
account matches the repo". `bidsmith drift` reports the rest: which live
settings fall outside the comparison and which of them are actually set.
Reach for it when a number on the account disagrees with the repo and the
plan is green anyway.

## GitOps: repos with CI

The intended home for `.bid` files is a user-controlled GitHub repo.
`bidsmith init` scaffolds exactly that: a starter `campaigns.bid`, a
`bidsmith.toml` (the account ids), `.gitignore`, a README, and a
`.github/workflows/bidsmith.yml` that runs `plan` on every pull request
(posting the diff as a sticky comment) and `apply` on merge to `main`.
**The merge is the approval gate.**

When you're working in a repo that already has that workflow — look for
`.github/workflows/bidsmith.yml` alongside a `bidsmith.toml` — change the
flow:

- Make edits on a branch, run `bidsmith validate` and `bidsmith plan`
  locally to check your work, then open a pull request. Let CI post the
  authoritative plan and let a human merge to apply.
- **Do not run `bidsmith apply` locally in this setup.** Apply belongs to
  CI on merge; running it by hand bypasses the review gate and can race
  the workflow. (If the user explicitly asks for a one-off local apply,
  fall back to the normal prompt-before-apply rule.)
- `apply --auto-approve` is correct *only* inside the CI job, where the
  merge already supplied the approval — never as a local shortcut.

To set a repo up from scratch, run `bidsmith init`, then point the user
at the generated `README.md` for the one-time steps: fill in
`bidsmith.toml`, and add the four `GOOGLE_ADS_*` values as GitHub Actions
secrets (Settings → Secrets and variables → Actions). `bidsmith auth
login` mints the refresh token; `bidsmith auth profile --with-client`
prints the values to paste.

### Opening pull requests from a sandbox

In a sandbox (e.g. Claude Cowork) there is **no GitHub plugin** — drive
GitHub through the `gh` CLI or a GitHub MCP server:

- **`gh` CLI**: install it in-session the same way as the bidsmith binary
  (pin a `gh` release URL on `github.com` — `api.github.com` may be
  proxy-blocked), then authenticate non-interactively with `GH_TOKEN` set
  to a fine-grained, repo-scoped Personal Access Token, and use
  `gh pr create` / `gh pr view` / `gh pr comment`. A repo-scoped PAT is a
  much smaller blast radius than the Google Ads credentials — keep them
  separate.
- **GitHub MCP server**: add it as a remote connector; it carries its
  own auth, so no token sits in the sandbox env.

Plain `git` against the mounted repo also works for branch + commit if a
remote credential is present; `gh` / MCP is the path for the PR itself.

## Reading performance data

`bidsmith query` is the supported way to read stats from the account.
It sends one GAQL query through bidsmith's own credentials and prints
the rows — GAQL is SELECT-only, so this verb cannot change anything.
Use `--format json` when the output feeds further analysis, the default
`table` when showing the user, `tsv` for spreadsheets.

```sh
# Campaign performance, last 30 days
bidsmith query "SELECT campaign.name, metrics.impressions, metrics.clicks, metrics.cost_micros, metrics.conversions FROM campaign WHERE segments.date DURING LAST_30_DAYS" --format json

# What people actually searched before seeing an ad
bidsmith query "SELECT search_term_view.search_term, metrics.clicks, metrics.conversions FROM search_term_view WHERE segments.date DURING LAST_30_DAYS ORDER BY metrics.clicks DESC LIMIT 50"

# Per-keyword cost and conversions
bidsmith query "SELECT ad_group_criterion.keyword.text, metrics.average_cpc, metrics.cost_micros, metrics.conversions FROM keyword_view WHERE segments.date DURING LAST_30_DAYS"
```

Money fields (`metrics.cost_micros`, `metrics.average_cpc`,
`cpc_bid_micros`) are micros — divide by 1,000,000 for the account
currency. Date ranges: `DURING LAST_7_DAYS` / `LAST_30_DAYS` /
`THIS_MONTH`, or `segments.date BETWEEN '2026-05-01' AND '2026-05-31'`.
Full grammar: https://developers.google.com/google-ads/api/docs/query/overview

The optimization loop is: `query` the stats, decide what to change,
edit the `.bid` files, then `plan` → show the user → `apply`. Stats
flow one way — insights never justify mutating outside plan/apply.

## Conventions

- The `.bid` file extension is provisional but stable for now.
- One `provider "google_ads"` block per repository/directory. Its
  optional `owns` list (`["sitelinks", "callouts",
  "structured_snippets", "calls"]`) makes account-level extensions
  exhaustive: a live `customer_asset` of a listed kind that no block
  declares is destroyed on the next apply. Adding an entry there
  changes what every campaign in the account serves — propose it, never
  add it unprompted.
- Declaring is claiming. Once a campaign or ad group declares one
  sitelink (or callout, or snippet), a live one of that kind it does
  not declare plans as `- destroy`. Adopt what should survive with
  `bidsmith import` *before* adding the first block of a kind.
- Resource addresses are stable identifiers; renaming a resource means
  Google Ads sees a delete + create. The full address is
  `<module>.<type>.<name>` (module = file basename), but inside the
  file you write `<type>.<name>` and the same form is used in
  references.
- Renaming a `.bid` file is also a rename of its module, which
  reshuffles every fully-qualified address inside it — treat it like a
  resource rename, not a no-op refactor.
- Validation errors are batched: fix everything `validate` reports in one
  pass rather than running it after each edit.

## When something doesn't work or a feature is missing

Bidsmith is pre-alpha — stubs, unsupported resource types, and missing
fields are expected. Treat gaps as something to report, not work around.

If `bidsmith` errors with "not yet implemented", complains about an
unknown resource type or field, crashes, or produces clearly wrong output:

1. Show the user the exact failing command and error output verbatim.
2. Classify it:
   - **Bug** — bidsmith ran but the result is wrong.
   - **Feature request** — bidsmith doesn't yet cover what the user
     needs (new resource type, new field, new verb, etc.).
3. Offer to file an issue at `chrmod/bidsmith`. If the user agrees and
   `gh` is authenticated, run:
   ```sh
   gh issue create --repo chrmod/bidsmith \
     --title "<concise summary>" \
     --body  "<command, error output, minimal .bid repro, expected vs. actual>"
   ```
   Otherwise share `https://github.com/chrmod/bidsmith/issues/new` and a
   pre-drafted title + body the user can paste.

**Do not** paper over the gap by hand-editing HCL the validator rejects,
by calling the Google Ads API directly, or by silently substituting other
tooling. Read-only questions go through `bidsmith query`; writes only
through `plan`/`apply`. Surfacing the missing capability is more valuable
than producing output that bypasses the engine.
