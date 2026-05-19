#!/usr/bin/env python3
"""Dump one Google Ads campaign as a bidsmith-compatible SearchStream JSON.

Designed to be copied into the rezolutnie repo as ``ads/dump_campaign.py``
so it can reuse ``ads.config.google_ads_config()`` for OAuth.

Usage (run from the rezolutnie project root):

    python -m ads.dump_campaign --campaign-id 1234567890 -o /tmp/w1.json

The output is a JSON array of ``SearchGoogleAdsStreamResponse`` batches with
camelCase field names and ``resourceName`` references — the exact shape
expected by ``bidsmith export --from-gads-search-response``.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from google.ads.googleads.client import GoogleAdsClient
from google.protobuf.json_format import MessageToDict

from ads.config import CUSTOMER_ID, google_ads_config


CAMPAIGN_QUERY = """
    SELECT
      campaign.resource_name,
      campaign.id,
      campaign.name,
      campaign.status,
      campaign.advertising_channel_type,
      campaign.campaign_budget,
      campaign.manual_cpc.enhanced_cpc_enabled,
      campaign.network_settings.target_google_search,
      campaign.network_settings.target_search_network,
      campaign.network_settings.target_content_network,
      campaign.network_settings.target_partner_search_network
    FROM campaign
    WHERE campaign.id = {campaign_id}
"""

CAMPAIGN_BUDGET_QUERY = """
    SELECT
      campaign_budget.resource_name,
      campaign_budget.id,
      campaign_budget.name,
      campaign_budget.amount_micros,
      campaign_budget.delivery_method,
      campaign_budget.explicitly_shared
    FROM campaign_budget
    WHERE campaign_budget.id IN ({budget_ids})
"""

AD_GROUP_QUERY = """
    SELECT
      ad_group.resource_name,
      ad_group.id,
      ad_group.name,
      ad_group.campaign,
      ad_group.status,
      ad_group.type,
      ad_group.cpc_bid_micros
    FROM ad_group
    WHERE campaign.id = {campaign_id}
"""

AD_GROUP_AD_QUERY = """
    SELECT
      ad_group_ad.resource_name,
      ad_group_ad.ad_group,
      ad_group_ad.status,
      ad_group_ad.ad.id,
      ad_group_ad.ad.name,
      ad_group_ad.ad.final_urls,
      ad_group_ad.ad.responsive_search_ad.headlines,
      ad_group_ad.ad.responsive_search_ad.descriptions,
      ad_group_ad.ad.responsive_search_ad.path1,
      ad_group_ad.ad.responsive_search_ad.path2
    FROM ad_group_ad
    WHERE campaign.id = {campaign_id}
"""

# Note: headlines / descriptions on responsive_search_ad expand into
# AdTextAsset objects ({text, pinnedField}) automatically. The select
# above pulls them whole. Selecting `.headlines.text` (etc.) would
# strip pinnedField — don't do that.

AD_GROUP_CRITERION_QUERY = """
    SELECT
      ad_group_criterion.resource_name,
      ad_group_criterion.ad_group,
      ad_group_criterion.status,
      ad_group_criterion.negative,
      ad_group_criterion.cpc_bid_micros,
      ad_group_criterion.keyword.text,
      ad_group_criterion.keyword.match_type
    FROM ad_group_criterion
    WHERE campaign.id = {campaign_id}
      AND ad_group_criterion.type = KEYWORD
"""

CAMPAIGN_CRITERION_QUERY = """
    SELECT
      campaign_criterion.resource_name,
      campaign_criterion.campaign,
      campaign_criterion.status,
      campaign_criterion.negative,
      campaign_criterion.keyword.text,
      campaign_criterion.keyword.match_type,
      campaign_criterion.location.geo_target_constant,
      campaign_criterion.language.language_constant,
      campaign_criterion.proximity.geo_point.latitude_in_micro_degrees,
      campaign_criterion.proximity.geo_point.longitude_in_micro_degrees,
      campaign_criterion.proximity.radius,
      campaign_criterion.proximity.radius_units
    FROM campaign_criterion
    WHERE campaign.id = {campaign_id}
      AND campaign_criterion.type IN (KEYWORD, LOCATION, LANGUAGE, PROXIMITY)
"""


def stream_to_batches(ga_service, customer_id: str, query: str) -> list[dict]:
    batches = []
    for batch in ga_service.search_stream(customer_id=customer_id, query=query):
        batches.append(MessageToDict(batch._pb, preserving_proto_field_name=False))
    return batches


def extract_budget_ids(batches: list[dict]) -> list[str]:
    ids: set[str] = set()
    for batch in batches:
        for row in batch.get("results", []):
            rn = row.get("campaign", {}).get("campaignBudget", "")
            if rn:
                ids.add(rn.rsplit("/", 1)[-1])
    return sorted(ids)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--customer-id",
        default=CUSTOMER_ID,
        help="Google Ads customer ID (default: GOOGLE_ADS_CUSTOMER_ID from .env).",
    )
    parser.add_argument(
        "--campaign-id",
        required=True,
        help="Campaign ID to dump.",
    )
    parser.add_argument(
        "-o", "--output",
        default="-",
        help="Where to write the JSON dump (default: stdout).",
    )
    args = parser.parse_args()

    client = GoogleAdsClient.load_from_dict(google_ads_config())
    ga = client.get_service("GoogleAdsService")

    all_batches: list[dict] = []

    print("→ campaign", file=sys.stderr)
    campaign_batches = stream_to_batches(
        ga, args.customer_id, CAMPAIGN_QUERY.format(campaign_id=args.campaign_id)
    )
    if not any(b.get("results") for b in campaign_batches):
        print(
            f"no campaign with id={args.campaign_id} in customer {args.customer_id}",
            file=sys.stderr,
        )
        return 1
    all_batches.extend(campaign_batches)

    budget_ids = extract_budget_ids(campaign_batches)
    if budget_ids:
        print(
            f"→ campaign_budget ({len(budget_ids)} id(s): {', '.join(budget_ids)})",
            file=sys.stderr,
        )
        all_batches.extend(
            stream_to_batches(
                ga,
                args.customer_id,
                CAMPAIGN_BUDGET_QUERY.format(budget_ids=", ".join(budget_ids)),
            )
        )

    queries = (
        ("ad_group", AD_GROUP_QUERY),
        ("ad_group_ad", AD_GROUP_AD_QUERY),
        ("ad_group_criterion", AD_GROUP_CRITERION_QUERY),
        ("campaign_criterion", CAMPAIGN_CRITERION_QUERY),
    )
    for label, q in queries:
        print(f"→ {label}", file=sys.stderr)
        all_batches.extend(
            stream_to_batches(ga, args.customer_id, q.format(campaign_id=args.campaign_id))
        )

    blob = json.dumps(all_batches, indent=2, ensure_ascii=False)
    if args.output == "-":
        sys.stdout.write(blob)
        sys.stdout.write("\n")
    else:
        Path(args.output).write_text(blob)
        print(f"wrote {args.output} ({len(blob):,} bytes)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
