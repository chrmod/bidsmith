# Bidsmith trial against a real rezolutnie campaign

This is the Phase 1 exit criterion for [bidsmith](../../). Pull one real
Google Ads campaign out of the rezolutnie account, push it through
`bidsmith export --from-gads-search-response`, then `validate` and
`fmt --check` the resulting `.bid`. Report back what fails.

Default target: the `[W1] magazyny-energii` campaign in the rezolutnie
customer. Substitute any other campaign id if W1 is gone.

You are running these steps **from the rezolutnie project root**
(`/Users/chrmod/Projects/github.com/chrmod/rezolutnie.com/`). Bidsmith
itself lives at `/Users/chrmod/Projects/github.com/chrmod/bidsmith/`
and is not modified during the trial.

## Optional: auth sanity check

Before any new trial run, especially the first one after this branch:

```
bidsmith plan --whoami
```

This exchanges the refresh token from your `.env` for an access token,
prints how long the token is good for, and confirms the customer_id /
login_customer_id / developer_token envelope is complete. It makes
**no** Google Ads API call yet — only the OAuth token endpoint.

Expected good output:
```
plan: refresh-token exchange succeeded.
  access token       : ya29.a0…XYZA (240 chars)
  expires_in         : 3599s
  customer_id        : 1428011099
  login_customer_id  : 5196446033
  developer_token    : set
```

If any of those reports missing, fix `.env` before running the real
plan step (next checkpoint).

## 0. Prerequisites

- The rezolutnie `.env` is populated and `python -m ads._debug` already
  works for you (i.e. OAuth is set up). If not, run
  `python -m ads.oauth_flow` first — see `ads/README.md`.
- Python deps from `ads/requirements.txt` are installed
  (`google-ads>=24.1.0`, `python-dotenv`).
- The bidsmith release binary exists at
  `/Users/chrmod/Projects/github.com/chrmod/bidsmith/target/release/bidsmith`.
  If it doesn't, build it:
  ```
  cd /Users/chrmod/Projects/github.com/chrmod/bidsmith
  cargo build --release
  ```

Optional but convenient — alias it for this session:
```
alias bidsmith=/Users/chrmod/Projects/github.com/chrmod/bidsmith/target/release/bidsmith
```

## 1. Drop the dump script into rezolutnie

```
cp /Users/chrmod/Projects/github.com/chrmod/bidsmith/examples/trial/dump_campaign.py \
   ads/dump_campaign.py
```

It imports `ads.config.google_ads_config` so it picks up your existing
OAuth without any new config.

## 2. Find the campaign id

```
python -m ads._debug
```

You'll get a list of campaigns. Copy the id of `[W1] magazyny-energii`
(or whichever campaign you're trialling).

## 3. Dump the campaign

```
python -m ads.dump_campaign --campaign-id <ID> -o /tmp/w1.json
```

You should see progress lines on stderr (`→ campaign`, `→ campaign_budget
(1 id(s): ...)`, `→ ad_group`, ...) and a JSON file at `/tmp/w1.json`
that's roughly 10–100KB.

The script pulls:
- `responsive_search_ad.headlines` / `.descriptions` as
  `AdTextAsset` objects (`{text, pinnedField}`) — pinning is preserved.
- `campaign_criterion.type IN (KEYWORD, LOCATION, LANGUAGE, PROXIMITY)`
  — proximity targets (lat/lng + radius + units) round-trip.

Sanity check:
```
jq '. | length' /tmp/w1.json          # number of batches
jq '[.[] | .results | length] | add' /tmp/w1.json   # total rows
```

## 4. Adapt + render with bidsmith

```
bidsmith export --from-gads-search-response /tmp/w1.json -o /tmp/w1.bid
head -40 /tmp/w1.bid
```

The rezolutnie `.env` is already loaded by `ads/dump_campaign.py`;
`bidsmith export` then picks up `GOOGLE_ADS_LOGIN_CUSTOMER_ID` (and
`GOOGLE_ADS_CUSTOMER_ID`) from the same environment, so the `provider`
block ends up wired to the right MCC automatically. Run with `env -i …`
if you want to confirm the env-free path produces no `login_customer_id`.

Expected: a recognizable `.bid` file with a `provider` block (carrying
both customer ids), one `google_ads_campaign_budget`, one
`google_ads_campaign`, one or more `google_ads_ad_group`, the ads with
pinned `headline { text, pin }` / `description { text, pin }` blocks,
and the criteria (keyword / location / language / proximity).

Useful flags / overrides:
- `--include-removed` to also export `status = "REMOVED"` resources
  (default: drop them, which makes for a much cleaner round-trip).
- `--customer-id <ID>` / `--login-customer-id <ID>` to override the env
  vars without re-exporting them.

## 5. Validate

```
bidsmith validate /tmp/w1.bid
```

Three possible outcomes:

| Outcome                                  | Meaning                                  |
|------------------------------------------|------------------------------------------|
| `OK: 1 file(s) valid.`                   | Everything bidsmith models is covered.   |
| `OK: 1 file(s) valid (N warnings).`      | Lints fired (status / RSA / phone) — fine. |
| `K errors, M warnings in 1 files.`       | Genuine schema/type/ref gap. Report it.  |

Save the error output if non-zero:
```
bidsmith validate /tmp/w1.bid 2> /tmp/w1.validate.txt; echo $?
```

## 6. Confirm canonical form

```
bidsmith fmt --check /tmp/w1.bid
```

Expected: `fmt: 1 file(s) already canonical.` The export renderer pipes
its output through the same emitter `fmt` uses, so this step should be
a hard no-op. If it ever says `would reformat`, that's a real
regression — please include the `diff` against the previous run in your
report.

## 7. Report back

Open a fresh chat (or update the bidsmith ROADMAP.md `Open decisions`
section directly) with:

1. Whether step 5 passed cleanly, or the validate diagnostics it produced.
2. The first ~80 lines of `/tmp/w1.bid` so the schema author can eyeball
   what bidsmith ingested.
3. Anything visibly wrong vs the Google Ads UI for that campaign —
   missing nested blocks, dropped extensions, criterion types that
   silently disappeared, wrong references, etc.
4. The total counts: `wc -l /tmp/w1.bid`, criterion count
   (`grep -c '^resource "google_ads_.*_criterion"' /tmp/w1.bid`), etc.

That feedback drives the next bidsmith session. Known follow-ups
already prioritised (ROADMAP.md "Next session"):

- Account-scoped resources: `asset` (CallAsset for the PL phone),
  `customer_asset`, `conversion_action` (Lead, Phone). This is what
  unlocks the policy fix — move the phone out of RSA copy and into a
  proper call extension.
- Campaign label support so `[W1]` is a real label, not a name prefix.

Anything else not on that list (resource types we missed, enum values
we haven't enumerated, ergonomics like fmt-vs-export alignment) is fair
game to add.

## Clean up

The dump script imports `ads.config`, so it lives naturally under
`ads/`. Leave it in place or delete it — either is fine. The script
performs no mutations; it's read-only on the Google Ads side.
