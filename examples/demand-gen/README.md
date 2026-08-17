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
