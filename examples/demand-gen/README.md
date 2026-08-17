# Demand Gen audience example

A Demand Gen campaign whose ad group targets a **grouped audience**: one
`google_ads_audience` resource holding the segments and demographics, and
one ad-group criterion pointing at it.

## Why the audience is a resource of its own

A Demand Gen ad group runs in grouped-audience mode. You cannot attach a
segment or a demographic to it directly — Google answers that with:

> Audience segment attachment is not allowed when use audience grouped
> bit is set to true.

Everything about *who* to reach goes inside the `google_ads_audience`
instead: in-market and affinity segments, life events, detailed
demographics, a custom audience you built yourself, plus age, gender,
parental status, and household income. The ad group then targets that one
audience.

`bidsmith validate` warns if it sees a segment or demographic attached
straight to a Demand Gen ad group, and names the grouped form to use.

## The setting you cannot change later

```hcl
audience_setting {
  use_audience_grouped = true
}
```

Google fixes this when the **ad group** is created. Without it, the
audience criterion is rejected; with it, direct segment criteria are.
`validate` requires it on any ad group whose criterion targets an
audience, and `plan` warns when a live ad group carries the other value —
at that point the only fix is a new ad group.

## Demographics: one vocabulary, two shapes

The API states an audience's age dimension in years and the criterion
version as an enum. `.bid` files use the enum both places:

```hcl
age_ranges = ["AGE_RANGE_35_44", "AGE_RANGE_45_54"]
```

`AGE_RANGE_UNDETERMINED` (and `UNDETERMINED` /
`INCOME_RANGE_UNDETERMINED` on the other axes) means "include people
Google could not classify" — the API's `include_undetermined` flag.

## The creative needs four things, not two

A Demand Gen video ad is rejected on create unless it carries **all** of:

| | |
|---|---|
| `videos` | a YouTube video already published on your channel |
| `logo_images` | 1–5 square logos, at least 128×128, 1:1 |
| `business_name` | your advertiser/brand name |
| headlines + descriptions | the copy |

`validate` warns when `business_name` or `logo_images` is missing, so you
find out before `plan` talks to Google.

## Which assets bidsmith creates, and which it only references

`google_ads_youtube_video_asset` and `google_ads_call_to_action_asset`
are **created** by `apply` — a video id and a button type are all the API
needs.

`google_ads_image_asset` is a **reference**. The Google Ads API accepts
image bytes on the way in but never hands them back, so bidsmith would
have no way to tell an image it uploaded from one it didn't. Upload the
logo once in Google Ads (**Assets → Images**), then name it here:

```hcl
resource "google_ads_image_asset" "logo" {
  name = "Brand logo 1200x1200"
}
```

If no image in the account carries that name, `plan` stops and says so
rather than sending an asset op that cannot work. Two images with the
same name? Add `asset_id = "123456789"` to pin one.

## The button is an asset, not a phrase

`call_to_actions` takes references, not text — Google renders "Learn
more" in the viewer's language from the type you pick:

```hcl
resource "google_ads_call_to_action_asset" "learn_more" {
  call_to_action = "LEARN_MORE"
}
```

`LEARN_MORE`, `SHOP_NOW`, `SIGN_UP`, `BOOK_NOW`, `DOWNLOAD`,
`GET_QUOTE`, `SUBSCRIBE`, `CONTACT_US`, `APPLY_NOW`, `BUY_NOW`,
`DONATE_NOW`, `ORDER_NOW`, `PLAY_NOW`, `SEE_MORE`, `START_NOW`,
`VISIT_SITE`, and `WATCH_NOW` are the whole vocabulary.

## Two things Demand Gen refuses

`target_cpm` — the campaign here bids with `target_spend`, because a
Demand Gen create carrying `target_cpm` comes back *"The operation is not
allowed for the given context."* naming no field. Live Demand Gen
campaigns bid with `TARGET_SPEND` or `TARGET_CPC`.

`use_audience_grouped` on anything but a Demand Gen (or App) ad group —
a `SEARCH` ad group declaring it is rejected with `trigger: 'SEARCH'`.

## Segment constants are customer-scoped

`user_interest`, `life_event`, and `detailed_demographic` take
`customers/{customer_id}/userInterests/{id}` and friends — **not**
`userInterestConstants/{id}`, which is not a resource name in any API
version. Only topics, geo targets, and languages are account-free
(`topicConstants/{id}`, `geoTargetConstants/{id}`,
`languageConstants/{id}`).

## Language and location

They stay on the **ad group** here. Google creates every Demand Gen
campaign with ad-group-level targeting unless
`demand_gen_campaign_settings { upgraded_targeting = false }` says
otherwise at creation time.
