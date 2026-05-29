# Bidsmith

Manage Google Ads campaigns as code. Version-controlled, reviewable,
replayable.

You write what your campaigns should look like in plain-text files, you
keep those files in GitHub like any other shared document, and bidsmith
makes Google Ads match. Every change is a Git commit. Every change is
reviewable as a pull request. Every change can be previewed before a
single euro moves.

If you've used Google Ads Editor for bulk edits, bidsmith solves the
same problem — but with full history, peer review, and an undo button
that actually works.

Full docs and walkthroughs: **[chrmod.github.io/bidsmith](https://chrmod.github.io/bidsmith/)**.

## Why bidsmith

- **Every change is a pull request.** Edit a file, open a PR, have a
  teammate approve. "Who paused that campaign last Friday?" — Git
  tells you.
- **Preview before you spend.** `bidsmith plan` shows you exactly what
  will change in Google Ads before anything ships. No surprises.
- **One source of truth.** Your `.bid` files describe what should
  exist. bidsmith reconciles. Run it twice — the second run does
  nothing.
- **Roll back cleanly.** `git revert` → `bidsmith apply`. Five seconds.

## Status

Pre-alpha, but usable end-to-end against a real Google Ads account.
Validate, plan, and apply all work today.

| Command    | What it does                                                                 |
|------------|------------------------------------------------------------------------------|
| `validate` | Check that your `.bid` files are well-formed                                 |
| `plan`     | Show the diff between your `.bid` files and what's live in Google Ads        |
| `apply`    | Make Google Ads match the `.bid` files (prompts before changing anything)    |
| `pull`     | Snapshot the live account                                                    |
| `refresh`  | Pull the live account down into `.bid` files (so you can adopt bidsmith without starting from scratch) |
| `export`   | Render a `.bid` file from a JSON description                                 |
| `fmt`      | Tidy up `.bid` formatting                                                    |

Covered today: campaigns (Search), ad groups, responsive search ads,
keywords and negatives, shared keyword lists, budgets,
location/language/proximity targeting, conversion actions, and call /
customer assets. The full reference is on the
[docs site](https://chrmod.github.io/bidsmith/resources/).

## A `.bid` file looks like this

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

Check it:

```sh
$ bidsmith validate examples/basic
OK: 1 file(s) valid.
```

When something is wrong, errors point at the exact line:

```
  × invalid value "TURBO"; expected one of [STANDARD, ACCELERATED]
   ╭─[examples/broken/schema.bid:8:21]
 7 │   amount_micros   = "ten million"
 8 │   delivery_method = "TURBO"
   ·                     ───────
 9 │ }
   ╰────
```

### Short forms for repetitive resources

Real campaigns accumulate dozens of keywords and negatives. The bulk
and list forms keep the file readable:

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

A 1025-line one-resource-per-keyword campaign comes out at 177 lines
in this form. See [`examples/bulk/main.bid`](examples/bulk/main.bid).

### Hoist repeated values with `locals`

A `locals` block lets you name a constant once and reference it
everywhere else in the file:

```hcl
locals {
  daily_budget_micros = 10000000
  city_radius_km      = 15
  polish              = "languageConstants/1030"
}

resource "google_ads_campaign_budget" "shared" {
  amount_micros = local.daily_budget_micros
  ...
}
```

References like `local.daily_budget_micros` resolve at validate / plan
/ apply time, so type checks and the validateOnly mutate see the
substituted value. See [`examples/locals/main.bid`](examples/locals/main.bid).

### Pivot the same `.bid` with `variable`

A `variable` block declares a typed input the same file can pivot on,
fed from `--var name=value` on the command line or `BIDSMITH_VAR_<name>`
in the environment:

```hcl
variable "city_radius_km" {
  type    = number
  default = 15
}

resource "google_ads_campaign_criterion" "warsaw_proximity" {
  proximity {
    radius       = var.city_radius_km
    radius_units = "KILOMETERS"
    ...
  }
}
```

```sh
bidsmith plan campaigns.bid --var city_radius_km=25
BIDSMITH_VAR_city_radius_km=25 bidsmith plan campaigns.bid
```

Allowed types are `string`, `number`, and `bool`. If a variable has
no `default` and no input is supplied, `validate` says so. See
[`examples/variable/main.bid`](examples/variable/main.bid).

### Instantiate the same shape with `module`

A `module` block points at a parameterized `.bid` file and supplies
its variables. Repeat the block to repeat the shape — one instance
per city, market, wave, A/B variant:

```hcl
module "warsaw" {
  source = "./modules/city-campaign.bid"

  city_name = "[W1] Warsaw — Search"
  latitude  = 52.229675
  longitude = 21.012228
}

module "krakow" {
  source = "./modules/city-campaign.bid"

  city_name           = "[W1] Krakow — Search"
  latitude            = 50.064650
  longitude           = 19.944980
  radius_km           = 20
  daily_budget_micros = 8000000
}
```

The source file (`modules/city-campaign.bid`) declares `variable`
blocks for each input it needs — `city_name`, `latitude`,
`longitude`, etc. — and produces a full campaign-budget-adgroup-ad
graph. Resources inside `module "warsaw"` get the address prefix
`warsaw.<type>.<name>`. Each instance is an isolation boundary:
locals and variables inside it stay private. See
[`examples/modules/`](examples/modules/).

## Install

```sh
brew install chrmod/tap/bidsmith
```

macOS and Linux (`x86_64` / `aarch64`) are supported. Windows works
through WSL — [install guide](https://chrmod.github.io/bidsmith/before-you-start/install/).

## Get started

The full walkthrough — including how to connect to your Google Ads
account — lives at **[chrmod.github.io/bidsmith](https://chrmod.github.io/bidsmith/)**.

The short version:

1. [Install bidsmith](https://chrmod.github.io/bidsmith/before-you-start/install/) — about 2 minutes.
2. [Set up GitHub](https://chrmod.github.io/bidsmith/before-you-start/set-up-github/) — about 5 minutes if you've never used Git.
3. [Connect to Google Ads](https://chrmod.github.io/bidsmith/before-you-start/connect-google-ads/) — about 15 minutes, one-time.
4. [Run your first plan](https://chrmod.github.io/bidsmith/before-you-start/first-ten-minutes/).

## Multiple files in one folder

Each `.bid` file's name acts as a folder for its resources. Two files
in the same directory can each declare
`google_ads_campaign_criterion.broad_wikipedia` without colliding —
they're addressed as
`nadarzyn.google_ads_campaign_criterion.broad_wikipedia` and
`warszawa.google_ads_campaign_criterion.broad_wikipedia`. See
[`examples/multi/`](examples/multi/) for a worked two-file example.

This makes the one-campaign-per-file workflow practical: shared
negatives that appear across every campaign live in different files
and don't conflict.

## How a change flows

```
your laptop                              GitHub                            Google Ads
─────────────────                        ──────────────                    ──────────────
.bid files (edited)  ─── git push ──►   .bid files (canon)
                                              │
                                              │  pull request
                                              ▼
                                         reviewed + merged
                                              │
                                              │
bidsmith plan        ◄── git pull ───   .bid files (canon)
bidsmith apply       ──────────────────────────────────────►   campaigns, ad groups,
                                                                keywords, budgets
```

The `.bid` files in GitHub are the source of truth. Your laptop is
where you edit and where you run `apply`. Google Ads is where the
result shows up.

State lives on Google Ads itself — managed resources carry a
`bidsmith:address=<…>` label. There is no separate state file to
back up or share.

## Build from source

If you'd rather build the binary yourself instead of installing via
Homebrew:

```sh
cargo build --release
./target/release/bidsmith --help
```

Requires Rust 1.89+. The release binary is around 1.3 MB.

## License

Bidsmith is licensed under the [Mozilla Public License 2.0](LICENSE) (MPL-2.0).

Using bidsmith carries no obligations — run it anywhere, including in CI pipelines and for managing clients' accounts, personal or commercial. MPL's copyleft is narrow and file-level: only if you modify bidsmith's own source files *and* distribute the result must those modified files stay open under MPL. It does not extend to your `.bid` files, your scripts, or anything that merely invokes bidsmith.
