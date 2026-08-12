# Video campaign example

A YouTube in-stream campaign: a video asset, a custom audience, the
campaign and its targeting, a TrueView ad group, and the creative.

## Read this before you `apply`

Google Ads does **not** let any API client create or change a campaign
whose `advertising_channel_type` is `"VIDEO"` — see
[Video campaigns](https://developers.google.com/google-ads/api/docs/video/overview):

> You cannot create new Video campaigns or update existing ones using the
> Google Ads API.

Every such operation comes back `MUTATE_NOT_ALLOWED`, and because `apply`
sends one atomic batch, a single video change rejects every unrelated
operation with it. `bidsmith plan` warns before the request goes out.

So this file is a **shape reference and an adopt target**, not something
to apply into an empty account. Build the campaign in the Google Ads UI,
then let bidsmith adopt it by name and keep it here as the record of
what's live — where it plans as a no-op and any UI drift shows up in
review.

The campaign says so in the file:

```hcl
lifecycle {
  create = false
}
```

Without it, a live campaign whose name doesn't match the `.bid` plans as
a **create**, which Google then rejects — taking every unrelated
operation in the batch with it. With it, `plan` stops and names the
campaign it was looking for.

`manual_cpv {}` (and `target_cpv` / `target_cpm` / `manual_cpm`) records
which strategy the live campaign bids with. The max-CPV bid itself lives
on the ad group.

For YouTube inventory you can create and manage end to end from a `.bid`,
use a Demand Gen campaign (`advertising_channel_type = "DEMAND_GEN"`)
with a `demand_gen_video_responsive_ad`.
