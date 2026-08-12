use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use hcl_edit::Span;
use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Block, Body, Structure};
use serde::Serialize;

use crate::diagnostics::Diag;
use crate::eval::{EvalCtx, EvalError};
use crate::parser::ParsedFile;

#[derive(Clone)]
pub enum FieldType {
    String,
    /// `YYYY-MM-DD`, the shape every date field in the Google Ads API takes.
    Date,
    Integer,
    Number,
    Bool,
    Enum(&'static [&'static str]),
    Ref(&'static [&'static str]),
    RefOrResourceName(&'static [&'static str]),
    AdTemplateRef,
    List(Box<FieldType>),
    RsaAssetList,
    LanguageList,
    LocationList,
    /// `{ name = "value", … }` — the shape Google Ads calls
    /// `url_custom_parameters`, a repeated key/value message that reads far
    /// better as a map than as a list of two-field objects.
    StringMap,
}

impl FieldType {
    pub fn list_of(inner: FieldType) -> Self {
        FieldType::List(Box::new(inner))
    }
}

/// The Google Ads API's effective create-default for an optional attribute.
/// An omitted attribute that carries one of these is *managed at the default*:
/// `plan` enforces it (a UI flip back surfaces as drift) and `refresh` / minimal
/// `fmt` stop emitting it once the live value matches.
#[derive(Clone)]
pub enum DefaultValue {
    Str(&'static str),
    Bool(bool),
}

// Single source of truth for the values referenced both here and by the
// declared-side fill (`ExportInput::apply_schema_defaults`).
pub const DEFAULT_STATUS: &str = "ENABLED";
pub const DEFAULT_DELIVERY_METHOD: &str = "STANDARD";
pub const DEFAULT_EU_POLITICAL: &str = "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING";
pub const DEFAULT_EXPLICITLY_SHARED: bool = false;
pub const DEFAULT_BUDGET_PERIOD: &str = "DAILY";
pub const DEFAULT_NEGATIVE: bool = false;
pub const DEFAULT_FREQUENCY_CAP_LEVEL: &str = "CAMPAIGN";
pub const DEFAULT_CUSTOM_AUDIENCE_TYPE: &str = "AUTO";

/// `Campaign.campaign_bidding_strategy` is a protobuf `oneof`, so these blocks
/// are alternatives: a campaign declares at most one. Ordered as the plan and
/// the rendered file emit them.
pub const CAMPAIGN_BIDDING_BLOCKS: &[&str] = &[
    "manual_cpc",
    "manual_cpm",
    "manual_cpv",
    "target_cpm",
    "target_cpv",
    "target_impression_share",
    "target_spend",
];

/// The update-mask paths that switch a live campaign onto each bidding block.
/// Google Ads refuses a mask that names a message field carrying subfields —
/// even when the operation leaves every one of them unset — so a strategy that
/// has any is masked by those subfields instead, which reaches the same `oneof`
/// member and clears whatever the campaign was bidding with before (issue #120).
/// Only the strategies the API models as field-less messages can be named
/// outright; the rest list every subfield their message carries as of v22.
pub const CAMPAIGN_BIDDING_MASK_PATHS: &[(&str, &[&str])] = &[
    ("manual_cpc", &["manual_cpc.enhanced_cpc_enabled"]),
    ("manual_cpm", &["manual_cpm"]),
    ("manual_cpv", &["manual_cpv"]),
    ("target_cpm", &["target_cpm.target_frequency_goal"]),
    ("target_cpv", &["target_cpv"]),
    (
        "target_impression_share",
        &[
            "target_impression_share.location",
            "target_impression_share.location_fraction_micros",
            "target_impression_share.cpc_bid_ceiling_micros",
        ],
    ),
    ("target_spend", &["target_spend.cpc_bid_ceiling_micros"]),
];

/// The mask paths that switch a campaign onto `field`, or `None` when `field`
/// names something other than a bidding block.
pub fn campaign_bidding_mask_paths(field: &str) -> Option<&'static [&'static str]> {
    CAMPAIGN_BIDDING_MASK_PATHS
        .iter()
        .find(|(block, _)| *block == field)
        .map(|(_, paths)| *paths)
}

/// Google Ads stores "runs until further notice" as this date rather than an
/// empty field, so it round-trips as an omitted `end_date` (issue #113).
pub const NO_END_DATE: &str = "2037-12-30";

/// Every field of `Campaign.network_settings`, each paired with its Google Ads
/// JSON name. All six are modelled on purpose: a block covering some of the
/// networks a campaign serves on reads as a complete declaration of where the
/// money goes, and is not one (issue #132).
pub const NETWORK_SETTINGS_FIELDS: &[(&str, &str)] = &[
    ("target_google_search", "targetGoogleSearch"),
    ("target_search_network", "targetSearchNetwork"),
    ("target_content_network", "targetContentNetwork"),
    ("target_partner_search_network", "targetPartnerSearchNetwork"),
    ("target_youtube", "targetYoutube"),
    ("target_google_tv_network", "targetGoogleTvNetwork"),
];

/// The four fields of
/// `Campaign.video_campaign_settings.video_ad_inventory_control`, each paired
/// with its Google Ads JSON name. Together they are the whole answer to which
/// YouTube inventory a campaign may serve on, which is the property a format
/// experiment exists to hold still (issue #133).
pub const VIDEO_AD_INVENTORY_FIELDS: &[(&str, &str)] = &[
    ("allow_in_stream", "allowInStream"),
    ("allow_in_feed", "allowInFeed"),
    ("allow_shorts", "allowShorts"),
    ("allow_non_skippable_in_stream", "allowNonSkippableInStream"),
];

/// The two fields of `Campaign.geo_target_type_setting`, each paired with its
/// Google Ads JSON name. They decide whether a targeted location means "people
/// there" or "people there plus people interested in there" (issue #114).
pub const GEO_TARGET_TYPE_FIELDS: &[(&str, &str)] = &[
    ("positive_geo_target_type", "positiveGeoTargetType"),
    ("negative_geo_target_type", "negativeGeoTargetType"),
];

/// Whether a live geo target type is one a `.bid` can declare. Google reports
/// the ones it has no value for as `UNKNOWN`, which is a report, not a setting.
pub fn is_geo_target_type(value: &str) -> bool {
    GEO_TARGET_TYPE.contains(&value)
}

/// Whether a live channel sub-type is one a `.bid` can declare. `UNSPECIFIED`
/// is what Google reports for a campaign that refines its channel no further,
/// and `UNKNOWN` for a sub-type this API version has no name for — neither is
/// something a file could say.
pub fn is_advertising_channel_sub_type(value: &str) -> bool {
    ADVERTISING_CHANNEL_SUB_TYPE.contains(&value)
}

/// The targeting dimensions a `target_restriction` can name. `KEYWORD` is a
/// dimension of the API's enum but not of this one: keywords always restrict, so
/// the API rejects `bid_only = true` on them and `false` says nothing.
pub const TARGETING_DIMENSION: &[&str] = &[
    "AUDIENCE",
    "TOPIC",
    "GENDER",
    "AGE_RANGE",
    "PLACEMENT",
    "PARENTAL_STATUS",
    "INCOME_RANGE",
];

/// Whether a live target restriction names a dimension a `.bid` can declare.
pub fn is_targeting_dimension(value: &str) -> bool {
    TARGETING_DIMENSION.contains(&value)
}

/// The API's reading of a dimension no `target_restriction` names: it targets,
/// i.e. its criteria restrict who is eligible to see the ad. So an entry that
/// says `bid_only = false` and no entry at all are the same statement, and
/// bidsmith keeps the shorter one (issue #135).
pub const DEFAULT_BID_ONLY: bool = false;

/// The settable bid fields on `AdGroup`, each paired with its Google Ads JSON
/// name. Which one carries the live bid depends on the campaign's bidding
/// strategy — a TARGET_CPV video ad group bids through `target_cpv_micros` and
/// leaves `cpc_bid_micros` at zero — so all of them are modelled and the API
/// rejects any that does not match the strategy. The read-only `effective_*`
/// variants are deliberately absent.
pub const AD_GROUP_BID_FIELDS: &[(&str, &str)] = &[
    ("cpc_bid_micros", "cpcBidMicros"),
    ("cpv_bid_micros", "cpvBidMicros"),
    ("cpm_bid_micros", "cpmBidMicros"),
    ("target_cpa_micros", "targetCpaMicros"),
    ("target_cpm_micros", "targetCpmMicros"),
    ("target_cpv_micros", "targetCpvMicros"),
    ("percent_cpc_bid_micros", "percentCpcBidMicros"),
    ("fixed_cpm_micros", "fixedCpmMicros"),
];

/// The Google Ads API is read-only for the VIDEO channel: "You cannot create
/// new Video campaigns or update existing ones using the Google Ads API"
/// (developers.google.com/google-ads/api/docs/video/overview). Every mutate op
/// on one comes back `MUTATE_NOT_ALLOWED`, which — in an atomic batch — takes
/// every unrelated operation down with it.
pub const VIDEO_IS_READ_ONLY: &str =
    "the Google Ads API cannot create or update VIDEO campaigns \
     (see developers.google.com/google-ads/api/docs/video/overview) — make the change in the \
     Google Ads UI, then let bidsmith adopt it";

pub struct AttributeSchema {
    pub name: &'static str,
    pub ty: FieldType,
    pub required: bool,
    pub default: Option<DefaultValue>,
    /// Keep emitting this attribute even when it equals its default — used for
    /// compliance declarations a reviewer wants visible in every file. Omission
    /// still enforces the default; this only affects rendering.
    pub always_emit: bool,
}

impl AttributeSchema {
    fn with_default(mut self, value: DefaultValue) -> Self {
        self.default = Some(value);
        self
    }

    fn always_emit(mut self) -> Self {
        self.always_emit = true;
        self
    }

    /// The default to strip-on-render, if any. `always_emit` attributes never
    /// return one (they stay in the file) even though omission still enforces it.
    pub fn droppable_default(&self) -> Option<&DefaultValue> {
        if self.always_emit {
            None
        } else {
            self.default.as_ref()
        }
    }
}

/// True when `expr` is a literal scalar equal to `default`. Non-literals
/// (references, `var.x`) never match, so they are preserved on render.
pub fn expr_matches_default(expr: &Expression, default: &DefaultValue) -> bool {
    match (expr, default) {
        (Expression::String(s), DefaultValue::Str(d)) => s.as_str() == *d,
        (Expression::Bool(b), DefaultValue::Bool(d)) => *b.as_ref() == *d,
        _ => false,
    }
}

pub struct NestedBlockSchema {
    pub name: &'static str,
    pub schema: BlockSchema,
}

pub struct BlockSchema {
    pub attributes: Vec<AttributeSchema>,
    pub blocks: Vec<NestedBlockSchema>,
}

const STATUS: &[&str] = &["ENABLED", "PAUSED", "REMOVED"];
const KEYWORD_MATCH_TYPE: &[&str] = &["EXACT", "PHRASE", "BROAD"];
const BUDGET_PERIOD: &[&str] = &["DAILY", "CUSTOM_PERIOD"];
const BUDGET_TYPE: &[&str] = &["STANDARD", "FIXED_CPA", "SMART_CAMPAIGN", "LOCAL_SERVICES"];
/// The `period` whose amount is a lifetime cap rather than a daily rate.
pub const CUSTOM_PERIOD: &str = "CUSTOM_PERIOD";
const RSA_PIN: &[&str] = &[
    "HEADLINE_1",
    "HEADLINE_2",
    "HEADLINE_3",
    "DESCRIPTION_1",
    "DESCRIPTION_2",
];
const PROXIMITY_RADIUS_UNITS: &[&str] = &["MILES", "KILOMETERS"];
const DEVICE_TYPE: &[&str] = &["MOBILE", "DESKTOP", "TABLET", "CONNECTED_TV", "OTHER"];

/// The device types Google auto-materializes on a search or display campaign,
/// and therefore the set `devices` takes the complement of. CONNECTED_TV is
/// video-only and OTHER is not settable, so neither is implied by omission —
/// list them in `devices` explicitly to target them.
pub const CORE_DEVICE_TYPES: &[&str] = &["MOBILE", "DESKTOP", "TABLET"];
const AGE_RANGE_TYPE: &[&str] = &[
    "AGE_RANGE_18_24",
    "AGE_RANGE_25_34",
    "AGE_RANGE_35_44",
    "AGE_RANGE_45_54",
    "AGE_RANGE_55_64",
    "AGE_RANGE_65_UP",
    "AGE_RANGE_UNDETERMINED",
];
const GENDER_TYPE: &[&str] = &["MALE", "FEMALE", "UNDETERMINED"];
const PARENTAL_STATUS_TYPE: &[&str] = &["PARENT", "NOT_A_PARENT", "UNDETERMINED"];
/// Household income percentile bands, top-down: `0_50` is the lower half,
/// `90_UP` the top 10%.
const INCOME_RANGE_TYPE: &[&str] = &[
    "INCOME_RANGE_0_50",
    "INCOME_RANGE_50_60",
    "INCOME_RANGE_60_70",
    "INCOME_RANGE_70_80",
    "INCOME_RANGE_80_90",
    "INCOME_RANGE_90_UP",
    "INCOME_RANGE_UNDETERMINED",
];
const CONVERSION_ACTION_TYPE: &[&str] = &[
    "UNKNOWN",
    "AD_CALL",
    "ANDROID_APP_PRE_REGISTRATION",
    "ANDROID_INSTALLS_ALL_OTHER_APPS",
    "CLICK_TO_CALL",
    "FIREBASE_ANDROID_CUSTOM",
    "FIREBASE_ANDROID_FIRST_OPEN",
    "FIREBASE_ANDROID_IN_APP_PURCHASE",
    "FIREBASE_IOS_CUSTOM",
    "FIREBASE_IOS_FIRST_OPEN",
    "FIREBASE_IOS_IN_APP_PURCHASE",
    "FLOODLIGHT_ACTION",
    "FLOODLIGHT_TRANSACTION",
    "GOOGLE_ANALYTICS_4_CUSTOM",
    "GOOGLE_ANALYTICS_4_PURCHASE",
    "GOOGLE_HOSTED",
    "GOOGLE_PLAY_DOWNLOAD",
    "GOOGLE_PLAY_IN_APP_PURCHASE",
    "LEAD_FORM_SUBMIT",
    "SALESFORCE",
    "SEARCH_ADS_360",
    "SMART_CAMPAIGN_AD_CLICKS_TO_CALL",
    "SMART_CAMPAIGN_MAP_CLICKS_TO_CALL",
    "SMART_CAMPAIGN_MAP_DIRECTIONS",
    "SMART_CAMPAIGN_TRACKED_CALLS",
    "STORE_SALES",
    "STORE_SALES_DIRECT_UPLOAD",
    "STORE_VISITS",
    "THIRD_PARTY_APP_ANALYTICS_ANDROID_CUSTOM",
    "THIRD_PARTY_APP_ANALYTICS_ANDROID_FIRST_OPEN",
    "THIRD_PARTY_APP_ANALYTICS_ANDROID_IN_APP_PURCHASE",
    "THIRD_PARTY_APP_ANALYTICS_IOS_CUSTOM",
    "THIRD_PARTY_APP_ANALYTICS_IOS_FIRST_OPEN",
    "THIRD_PARTY_APP_ANALYTICS_IOS_IN_APP_PURCHASE",
    "UPLOAD_CALLS",
    "UPLOAD_CLICKS",
    "WEBPAGE",
    "WEBPAGE_CODELESS",
    "WEBSITE_CALL",
];
const CONVERSION_ACTION_CATEGORY: &[&str] = &[
    "UNKNOWN",
    "DEFAULT",
    "PAGE_VIEW",
    "PURCHASE",
    "SIGNUP",
    "LEAD",
    "DOWNLOAD",
    "ADD_TO_CART",
    "BEGIN_CHECKOUT",
    "SUBSCRIBE_PAID",
    "PHONE_CALL_LEAD",
    "IMPORTED_LEAD",
    "SUBMIT_LEAD_FORM",
    "BOOK_APPOINTMENT",
    "REQUEST_QUOTE",
    "GET_DIRECTIONS",
    "OUTBOUND_CLICK",
    "CONTACT",
    "ENGAGEMENT",
    "STORE_VISIT",
    "STORE_SALE",
    "QUALIFIED_LEAD",
    "CONVERTED_LEAD",
];
const CONVERSION_ACTION_STATUS: &[&str] = &["ENABLED", "REMOVED", "HIDDEN"];
const CONVERSION_COUNTING_TYPE: &[&str] = &["ONE_PER_CLICK", "MANY_PER_CLICK"];
const CALL_CONVERSION_REPORTING_STATE: &[&str] = &[
    "DISABLED",
    "USE_ACCOUNT_LEVEL_CALL_CONVERSION_ACTION",
    "USE_RESOURCE_LEVEL_CALL_CONVERSION_ACTION",
];
/// Every `AdvertisingChannelSubType` a campaign can be created with, in the
/// order the v22 API lists them. `UNSPECIFIED` and `UNKNOWN` are deliberately
/// absent — Google returns them, no file can declare them.
const ADVERTISING_CHANNEL_SUB_TYPE: &[&str] = &[
    "SEARCH_MOBILE_APP",
    "DISPLAY_MOBILE_APP",
    "SEARCH_EXPRESS",
    "DISPLAY_EXPRESS",
    "SHOPPING_SMART_ADS",
    "DISPLAY_GMAIL_AD",
    "DISPLAY_SMART_CAMPAIGN",
    "VIDEO_ACTION",
    "VIDEO_NON_SKIPPABLE",
    "APP_CAMPAIGN",
    "APP_CAMPAIGN_FOR_ENGAGEMENT",
    "LOCAL_CAMPAIGN",
    "SHOPPING_COMPARISON_LISTING_ADS",
    "SMART_CAMPAIGN",
    "VIDEO_SEQUENCE",
    "APP_CAMPAIGN_FOR_PRE_REGISTRATION",
    "VIDEO_REACH_TARGET_FREQUENCY",
    "TRAVEL_ACTIVITIES",
    "YOUTUBE_AUDIO",
];
/// `PositiveGeoTargetType` / `NegativeGeoTargetType` share these two members.
/// The deprecated `SEARCH_INTEREST` is deliberately absent: Google removed it
/// from the UI and it only ever applied to the positive side.
const GEO_TARGET_TYPE: &[&str] = &["PRESENCE_OR_INTEREST", "PRESENCE"];
const TARGET_IMPRESSION_SHARE_LOCATION: &[&str] = &[
    "ANYWHERE_ON_PAGE",
    "TOP_OF_PAGE",
    "ABSOLUTE_TOP_OF_PAGE",
];
const FREQUENCY_CAP_EVENT_TYPE: &[&str] = &["IMPRESSION", "VIDEO_VIEW"];
const FREQUENCY_CAP_TIME_UNIT: &[&str] = &["DAY", "WEEK", "MONTH"];
const FREQUENCY_CAP_LEVEL: &[&str] = &["CAMPAIGN", "AD_GROUP", "AD_GROUP_AD"];
const CUSTOM_AUDIENCE_TYPE: &[&str] = &["AUTO", "INTEREST", "PURCHASE_INTENT", "SEARCH"];
const CUSTOM_AUDIENCE_STATUS: &[&str] = &["ENABLED", "REMOVED"];
const SHARED_SET_TYPE: &[&str] = &["NEGATIVE_KEYWORDS", "ACCOUNT_LEVEL_NEGATIVE_KEYWORDS"];
const SHARED_SET_STATUS: &[&str] = &["ENABLED", "REMOVED"];
const CAMPAIGN_SHARED_SET_STATUS: &[&str] = &["ENABLED", "REMOVED"];

const ASSET_FIELD_TYPE: &[&str] = &[
    "HEADLINE",
    "DESCRIPTION",
    "MANDATORY_AD_TEXT",
    "MARKETING_IMAGE",
    "MEDIA_BUNDLE",
    "YOUTUBE_VIDEO",
    "BOOK_ON_GOOGLE",
    "LEAD_FORM",
    "PROMOTION",
    "CALLOUT",
    "STRUCTURED_SNIPPET",
    "SITELINK",
    "MOBILE_APP",
    "HOTEL_CALLOUT",
    "CALL",
    "PRICE",
    "LONG_HEADLINE",
    "BUSINESS_NAME",
    "SQUARE_MARKETING_IMAGE",
    "PORTRAIT_MARKETING_IMAGE",
    "LOGO",
    "LANDSCAPE_LOGO",
    "VIDEO",
    "CALL_TO_ACTION_SELECTOR",
    "AD_IMAGE",
    "BUSINESS_LOGO",
    "HOTEL_PROPERTY",
    "DISCOVERY_CAROUSEL_CARD",
];

// Every asset resource type an asset-link (`customer_asset` / `campaign_asset` /
// `ad_group_asset`) may point its `asset` reference at.
const ASSET_TYPES: &[&str] = &[
    "google_ads_call_asset",
    "google_ads_sitelink_asset",
    "google_ads_callout_asset",
    "google_ads_structured_snippet_asset",
];

/// Which `field_type` an asset of each resource type can be attached as. The
/// mapping is 1:1 — a sitelink asset is only ever a SITELINK — so declaring it
/// is ceremony that can only ever be wrong (issue #145).
pub fn field_type_for_asset(resource_type: &str) -> Option<&'static str> {
    match resource_type {
        "google_ads_sitelink_asset" => Some("SITELINK"),
        "google_ads_callout_asset" => Some("CALLOUT"),
        "google_ads_structured_snippet_asset" => Some("STRUCTURED_SNIPPET"),
        "google_ads_call_asset" => Some("CALL"),
        _ => None,
    }
}

fn rsa_asset_block(name: &'static str) -> NestedBlockSchema {
    NestedBlockSchema {
        name,
        schema: BlockSchema {
            attributes: vec![
                attr("text", FieldType::String, true),
                attr("pin", FieldType::Enum(RSA_PIN), false),
            ],
            blocks: vec![],
        },
    }
}

/// The tracking-template pair every level of the hierarchy carries. Google
/// appends the suffix to the landing page at click time — it never appears in
/// the displayed URL — and `custom_parameters` supplies the `{_name}`
/// placeholders it can reference.
/// A structured snippet declared where it is used. The resource form stays
/// available for a snippet shared between campaigns or ad groups (issue #145).
fn inline_snippet_block() -> NestedBlockSchema {
    NestedBlockSchema {
        name: "structured_snippet",
        schema: BlockSchema {
            attributes: vec![
                attr("header", FieldType::String, true),
                attr("values", FieldType::list_of(FieldType::String), true),
            ],
            blocks: vec![],
        },
    }
}

fn tracking_attrs() -> Vec<AttributeSchema> {
    vec![
        attr("final_url_suffix", FieldType::String, false),
        attr("custom_parameters", FieldType::StringMap, false),
    ]
}

// The `ad {}` body, shared by `google_ads_ad_group_ad` and the `ad_template` block.
// `final_urls` is required on an inline `ad {}` (an RSA needs a landing page) but
// optional on an `ad_template`, which may be URL-agnostic and let every reference
// supply `final_urls` via an override.
fn ad_block(final_urls_required: bool) -> NestedBlockSchema {
    NestedBlockSchema {
        name: "ad",
        schema: BlockSchema {
            attributes: vec![
                attr("name", FieldType::String, false),
                attr(
                    "final_urls",
                    FieldType::list_of(FieldType::String),
                    final_urls_required,
                ),
                attr(
                    "final_mobile_urls",
                    FieldType::list_of(FieldType::String),
                    false,
                ),
                attr("display_url", FieldType::String, false),
            ]
            .into_iter()
            .chain(tracking_attrs())
            .collect(),
            blocks: vec![
                NestedBlockSchema {
                    name: "responsive_search_ad",
                    schema: BlockSchema {
                        attributes: vec![
                            attr("path1", FieldType::String, false),
                            attr("path2", FieldType::String, false),
                            attr("headlines", FieldType::RsaAssetList, false),
                            attr("descriptions", FieldType::RsaAssetList, false),
                        ],
                        blocks: vec![rsa_asset_block("headline"), rsa_asset_block("description")],
                    },
                },
                // A YouTube in-stream / bumper / non-skippable video ad. `video`
                // references an already-uploaded YouTube video by id — bidsmith does
                // not (and cannot) upload the video file itself; see the upload
                // notice `plan` surfaces.
                NestedBlockSchema {
                    name: "video_responsive_ad",
                    schema: BlockSchema {
                        attributes: vec![
                            attr(
                                "video",
                                FieldType::Ref(&["google_ads_youtube_video_asset"]),
                                true,
                            ),
                            attr("headlines", FieldType::list_of(FieldType::String), false),
                            attr(
                                "long_headlines",
                                FieldType::list_of(FieldType::String),
                                false,
                            ),
                            attr("descriptions", FieldType::list_of(FieldType::String), false),
                            attr(
                                "call_to_actions",
                                FieldType::list_of(FieldType::String),
                                false,
                            ),
                            attr("breadcrumb1", FieldType::String, false),
                            attr("breadcrumb2", FieldType::String, false),
                        ],
                        blocks: vec![],
                    },
                },
                // The creative a UI-built VIDEO campaign actually carries:
                // `Ad.video_ad`, a single YouTube video in one of the in-stream /
                // bumper / in-feed formats. Adopt-only — the VIDEO channel refuses
                // every create and update — so the block models the one field that
                // identifies the creative and leaves the format alone.
                NestedBlockSchema {
                    name: "video_ad",
                    schema: BlockSchema {
                        attributes: vec![attr(
                            "video",
                            FieldType::Ref(&["google_ads_youtube_video_asset"]),
                            true,
                        )],
                        blocks: vec![],
                    },
                },
                // A Demand Gen video responsive ad — the ad type a DEMAND_GEN
                // campaign carries. A distinct API message from video_responsive_ad
                // (VIDEO campaigns). `business_name` is optional here but required
                // by the API on create; `call_to_actions` are CALL_TO_ACTION asset
                // refs on the wire, which bidsmith does not model yet.
                NestedBlockSchema {
                    name: "demand_gen_video_responsive_ad",
                    schema: BlockSchema {
                        attributes: vec![
                            attr(
                                "videos",
                                FieldType::list_of(FieldType::Ref(&[
                                    "google_ads_youtube_video_asset",
                                ])),
                                false,
                            ),
                            attr("headlines", FieldType::list_of(FieldType::String), false),
                            attr(
                                "long_headlines",
                                FieldType::list_of(FieldType::String),
                                false,
                            ),
                            attr("descriptions", FieldType::list_of(FieldType::String), false),
                            attr(
                                "call_to_actions",
                                FieldType::list_of(FieldType::String),
                                false,
                            ),
                            attr("breadcrumb1", FieldType::String, false),
                            attr("breadcrumb2", FieldType::String, false),
                            attr("business_name", FieldType::String, false),
                        ],
                        blocks: vec![],
                    },
                },
            ],
        },
    }
}

fn keyword_block() -> NestedBlockSchema {
    NestedBlockSchema {
        name: "keyword",
        schema: BlockSchema {
            attributes: vec![
                attr("text", FieldType::String, true),
                attr("match_type", FieldType::Enum(KEYWORD_MATCH_TYPE), true),
            ],
            blocks: vec![],
        },
    }
}

fn negative_keyword_block() -> NestedBlockSchema {
    NestedBlockSchema {
        name: "negative_keyword",
        schema: BlockSchema {
            attributes: vec![
                attr("text", FieldType::String, true),
                attr("match_type", FieldType::Enum(KEYWORD_MATCH_TYPE), true),
            ],
            blocks: vec![],
        },
    }
}

/// A criterion block whose whole body is one attribute naming what it targets.
fn one_attr_block(
    name: &'static str,
    attribute: &'static str,
    ty: FieldType,
) -> NestedBlockSchema {
    NestedBlockSchema {
        name,
        schema: BlockSchema {
            attributes: vec![attr(attribute, ty, true)],
            blocks: vec![],
        },
    }
}

fn location_block() -> NestedBlockSchema {
    one_attr_block("location", "geo_target_constant", FieldType::String)
}

fn language_block() -> NestedBlockSchema {
    one_attr_block("language", "language_constant", FieldType::String)
}

/// Three distinct API criterion messages, one block: they all answer "which
/// audience?", and only one may be set. Enforced by `validate_exactly_one_of`,
/// not expressible in the schema.
fn audience_block() -> NestedBlockSchema {
    NestedBlockSchema {
        name: "audience",
        schema: BlockSchema {
            attributes: vec![
                attr(
                    "custom_audience",
                    FieldType::RefOrResourceName(&["google_ads_custom_audience"]),
                    false,
                ),
                attr("user_list", FieldType::String, false),
                attr("combined_audience", FieldType::String, false),
            ],
            blocks: vec![],
        },
    }
}

/// The who-and-where axes both criterion resources accept: cohort narrowing,
/// YouTube inventory, and demographics.
fn audience_targeting_blocks() -> Vec<NestedBlockSchema> {
    vec![
        one_attr_block("youtube_channel", "channel_id", FieldType::String),
        one_attr_block("youtube_video", "video_id", FieldType::String),
        one_attr_block("topic", "topic_constant", FieldType::String),
        one_attr_block(
            "user_interest",
            "user_interest_category",
            FieldType::String,
        ),
        one_attr_block("age_range", "type", FieldType::Enum(AGE_RANGE_TYPE)),
        one_attr_block("gender", "type", FieldType::Enum(GENDER_TYPE)),
        audience_block(),
    ]
}

// "exactly one of match_type / match_types" is not expressible here; enforced by validate_compact_keywords.
fn compact_keywords_block(name: &'static str) -> NestedBlockSchema {
    NestedBlockSchema {
        name,
        schema: BlockSchema {
            attributes: vec![
                attr("texts", FieldType::list_of(FieldType::String), true),
                attr("match_type", FieldType::Enum(KEYWORD_MATCH_TYPE), false),
                attr(
                    "match_types",
                    FieldType::list_of(FieldType::Enum(KEYWORD_MATCH_TYPE)),
                    false,
                ),
            ],
            blocks: vec![],
        },
    }
}

/// Whether each targeting dimension restricts who is eligible to see the ad, or
/// merely informs bidding. Shared by campaigns and ad groups — the API carries
/// the same `TargetingSetting` message on both (issue #135).
fn targeting_setting_block() -> NestedBlockSchema {
    NestedBlockSchema {
        name: "targeting_setting",
        schema: BlockSchema {
            attributes: vec![],
            blocks: vec![NestedBlockSchema {
                name: "target_restriction",
                schema: BlockSchema {
                    attributes: vec![
                        attr(
                            "targeting_dimension",
                            FieldType::Enum(TARGETING_DIMENSION),
                            true,
                        ),
                        attr("bid_only", FieldType::Bool, true),
                    ],
                    blocks: vec![],
                },
            }],
        },
    }
}

/// A bidding strategy the Google Ads API models as an empty message: the
/// block's presence is the whole setting, the bid amount lives on the ad group.
fn bidding_selector_block(name: &'static str) -> NestedBlockSchema {
    NestedBlockSchema {
        name,
        schema: BlockSchema {
            attributes: vec![],
            blocks: vec![],
        },
    }
}

fn attr(name: &'static str, ty: FieldType, required: bool) -> AttributeSchema {
    AttributeSchema {
        name,
        ty,
        required,
        default: None,
        always_emit: false,
    }
}

fn resource_schemas() -> &'static HashMap<&'static str, BlockSchema> {
    static SCHEMAS: OnceLock<HashMap<&'static str, BlockSchema>> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        let mut m = HashMap::new();

        m.insert(
            "google_ads_campaign_budget",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    // Which of the two amounts is required depends on `period`;
                    // enforced by validate_budget_amount.
                    attr("amount_micros", FieldType::Integer, false),
                    attr("total_amount_micros", FieldType::Integer, false),
                    attr("period", FieldType::Enum(BUDGET_PERIOD), false)
                        .with_default(DefaultValue::Str(DEFAULT_BUDGET_PERIOD)),
                    // No default: the API documents one for `period` but not for
                    // `type`, so an omitted type is left for Google to pick.
                    attr("type", FieldType::Enum(BUDGET_TYPE), false),
                    attr(
                        "delivery_method",
                        FieldType::Enum(&["STANDARD", "ACCELERATED"]),
                        false,
                    )
                    .with_default(DefaultValue::Str(DEFAULT_DELIVERY_METHOD)),
                    attr("explicitly_shared", FieldType::Bool, false)
                        .with_default(DefaultValue::Bool(DEFAULT_EXPLICITLY_SHARED)),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_campaign",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr("status", FieldType::Enum(STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                    attr(
                        "advertising_channel_type",
                        FieldType::Enum(&[
                            "SEARCH",
                            "DISPLAY",
                            "SHOPPING",
                            "VIDEO",
                            "PERFORMANCE_MAX",
                            "MULTI_CHANNEL",
                            "LOCAL",
                            "SMART",
                            "DISCOVERY",
                            "DEMAND_GEN",
                        ]),
                        true,
                    ),
                    // Which variant of the channel this is. Immutable after
                    // create, so a file that records it is the only thing that
                    // can tell two video campaigns apart (issue #133).
                    attr(
                        "advertising_channel_sub_type",
                        FieldType::Enum(ADVERTISING_CHANNEL_SUB_TYPE),
                        false,
                    ),
                    attr(
                        "campaign_budget",
                        FieldType::Ref(&["google_ads_campaign_budget"]),
                        true,
                    ),
                    attr(
                        "contains_eu_political_advertising",
                        FieldType::Enum(&[
                            "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING",
                            "CONTAINS_EU_POLITICAL_ADVERTISING",
                        ]),
                        false,
                    )
                    .with_default(DefaultValue::Str(DEFAULT_EU_POLITICAL))
                    .always_emit(),
                    attr("start_date", FieldType::Date, false),
                    attr("end_date", FieldType::Date, false),
                    attr("languages", FieldType::LanguageList, false),
                    attr("locations", FieldType::LocationList, false),
                    attr(
                        "devices",
                        FieldType::list_of(FieldType::Enum(DEVICE_TYPE)),
                        false,
                    ),
                    attr(
                        "excluded_devices",
                        FieldType::list_of(FieldType::Enum(DEVICE_TYPE)),
                        false,
                    ),
                    attr("callouts", FieldType::list_of(FieldType::String), false),
                ]
                .into_iter()
                .chain(tracking_attrs())
                .collect(),
                blocks: vec![
                    NestedBlockSchema {
                        name: "manual_cpc",
                        schema: BlockSchema {
                            attributes: vec![attr(
                                "enhanced_cpc_enabled",
                                FieldType::Bool,
                                false,
                            )],
                            blocks: vec![],
                        },
                    },
                    bidding_selector_block("manual_cpm"),
                    bidding_selector_block("manual_cpv"),
                    bidding_selector_block("target_cpm"),
                    bidding_selector_block("target_cpv"),
                    NestedBlockSchema {
                        name: "target_impression_share",
                        schema: BlockSchema {
                            attributes: vec![
                                attr(
                                    "location",
                                    FieldType::Enum(TARGET_IMPRESSION_SHARE_LOCATION),
                                    true,
                                ),
                                attr("location_fraction_micros", FieldType::Integer, true),
                                // Required by the API, not merely by this schema:
                                // impression-share bidding has no uncapped form.
                                attr("cpc_bid_ceiling_micros", FieldType::Integer, true),
                            ],
                            blocks: vec![],
                        },
                    },
                    NestedBlockSchema {
                        name: "target_spend",
                        schema: BlockSchema {
                            attributes: vec![attr(
                                "cpc_bid_ceiling_micros",
                                FieldType::Integer,
                                false,
                            )],
                            blocks: vec![],
                        },
                    },
                    NestedBlockSchema {
                        name: "network_settings",
                        schema: BlockSchema {
                            attributes: NETWORK_SETTINGS_FIELDS
                                .iter()
                                .map(|(field, _)| attr(field, FieldType::Bool, false))
                                .collect(),
                            blocks: vec![],
                        },
                    },
                    inline_snippet_block(),
                    // How the campaign's geo targets are interpreted — whether
                    // a location means "people there" or "people there plus
                    // people interested in there" (issue #114).
                    NestedBlockSchema {
                        name: "geo_target_type_setting",
                        schema: BlockSchema {
                            attributes: vec![
                                attr(
                                    "positive_geo_target_type",
                                    FieldType::Enum(GEO_TARGET_TYPE),
                                    false,
                                ),
                                attr(
                                    "negative_geo_target_type",
                                    FieldType::Enum(GEO_TARGET_TYPE),
                                    false,
                                ),
                            ],
                            blocks: vec![],
                        },
                    },
                    // Which YouTube inventory the campaign's responsive video
                    // ads may serve on — the property a format experiment
                    // exists to hold still (issue #133).
                    NestedBlockSchema {
                        name: "video_campaign_settings",
                        schema: BlockSchema {
                            attributes: vec![],
                            blocks: vec![NestedBlockSchema {
                                name: "video_ad_inventory_control",
                                schema: BlockSchema {
                                    attributes: VIDEO_AD_INVENTORY_FIELDS
                                        .iter()
                                        .map(|(field, _)| attr(field, FieldType::Bool, false))
                                        .collect(),
                                    blocks: vec![],
                                },
                            }],
                        },
                    },
                    targeting_setting_block(),
                    // Repeatable: one block per cap, mapping to a single
                    // `FrequencyCapEntry` in `Campaign.frequency_caps`.
                    NestedBlockSchema {
                        name: "frequency_caps",
                        schema: BlockSchema {
                            attributes: vec![
                                attr(
                                    "event_type",
                                    FieldType::Enum(FREQUENCY_CAP_EVENT_TYPE),
                                    true,
                                ),
                                attr(
                                    "time_unit",
                                    FieldType::Enum(FREQUENCY_CAP_TIME_UNIT),
                                    true,
                                ),
                                attr("time_length", FieldType::Integer, true),
                                attr("cap", FieldType::Integer, true),
                                attr("level", FieldType::Enum(FREQUENCY_CAP_LEVEL), false)
                                    .with_default(DefaultValue::Str(DEFAULT_FREQUENCY_CAP_LEVEL)),
                            ],
                            blocks: vec![],
                        },
                    },
                ],
            },
        );

        m.insert(
            "google_ads_ad_group_criterion",
            BlockSchema {
                attributes: vec![
                    attr(
                        "ad_group",
                        FieldType::Ref(&["google_ads_ad_group"]),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                    attr("negative", FieldType::Bool, false)
                        .with_default(DefaultValue::Bool(DEFAULT_NEGATIVE)),
                    attr("cpc_bid_micros", FieldType::Integer, false),
                    attr("bid_modifier", FieldType::Number, false),
                ],
                blocks: {
                    let mut b = vec![
                        keyword_block(),
                        compact_keywords_block("keywords"),
                        negative_keyword_block(),
                        compact_keywords_block("negative_keywords"),
                    ];
                    b.extend(audience_targeting_blocks());
                    // Ad-group-only axes. `placement` names one site, app, or
                    // channel URL; the two demographics round out age / gender.
                    b.push(one_attr_block("placement", "url", FieldType::String));
                    b.push(one_attr_block(
                        "parental_status",
                        "type",
                        FieldType::Enum(PARENTAL_STATUS_TYPE),
                    ));
                    b.push(one_attr_block(
                        "income_range",
                        "type",
                        FieldType::Enum(INCOME_RANGE_TYPE),
                    ));
                    // Geo and language intersect with the campaign's own: a
                    // viewer has to match both to be targeted.
                    b.push(location_block());
                    b.push(language_block());
                    b
                },
            },
        );

        m.insert(
            "google_ads_campaign_criterion",
            BlockSchema {
                attributes: vec![
                    attr(
                        "campaign",
                        FieldType::Ref(&["google_ads_campaign"]),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                    attr("negative", FieldType::Bool, false)
                        .with_default(DefaultValue::Bool(DEFAULT_NEGATIVE)),
                    attr("bid_modifier", FieldType::Number, false),
                ],
                blocks: {
                    let mut b = vec![
                        keyword_block(),
                        negative_keyword_block(),
                        compact_keywords_block("negative_keywords"),
                        one_attr_block("device", "type", FieldType::Enum(DEVICE_TYPE)),
                        location_block(),
                        language_block(),
                        NestedBlockSchema {
                            name: "proximity",
                            schema: BlockSchema {
                                attributes: vec![
                                    attr("latitude", FieldType::Number, true),
                                    attr("longitude", FieldType::Number, true),
                                    attr("radius", FieldType::Number, true),
                                    attr(
                                        "radius_units",
                                        FieldType::Enum(PROXIMITY_RADIUS_UNITS),
                                        true,
                                    ),
                                ],
                                blocks: vec![],
                            },
                        },
                    ];
                    b.extend(audience_targeting_blocks());
                    b
                },
            },
        );

        m.insert(
            "google_ads_ad_group_ad",
            BlockSchema {
                attributes: vec![
                    attr(
                        "ad_group",
                        FieldType::Ref(&["google_ads_ad_group"]),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                    attr("template", FieldType::AdTemplateRef, false),
                    attr("final_urls", FieldType::list_of(FieldType::String), false),
                    attr("path1", FieldType::String, false),
                    attr("path2", FieldType::String, false),
                ]
                .into_iter()
                .chain(tracking_attrs())
                .collect(),
                blocks: vec![ad_block(true)],
            },
        );

        m.insert(
            "google_ads_ad_group",
            BlockSchema {
                attributes: {
                    let mut a = vec![
                        attr("name", FieldType::String, true),
                        attr(
                            "campaign",
                            FieldType::Ref(&["google_ads_campaign"]),
                            true,
                        ),
                        attr("status", FieldType::Enum(STATUS), false)
                            .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                        attr(
                            "type",
                            FieldType::Enum(&[
                                "SEARCH_STANDARD",
                                "DISPLAY_STANDARD",
                                "SHOPPING_PRODUCT_ADS",
                                "VIDEO_BUMPER",
                                "VIDEO_TRUE_VIEW_IN_STREAM",
                                "VIDEO_TRUE_VIEW_IN_DISPLAY",
                                "VIDEO_NON_SKIPPABLE_IN_STREAM",
                                "VIDEO_RESPONSIVE",
                            ]),
                            false,
                        ),
                    ];
                    a.extend(
                        AD_GROUP_BID_FIELDS
                            .iter()
                            .map(|(name, _)| attr(name, FieldType::Integer, false)),
                    );
                    a.extend(tracking_attrs());
                    a.push(attr("callouts", FieldType::list_of(FieldType::String), false));
                    a
                },
                blocks: vec![targeting_setting_block(), inline_snippet_block()],
            },
        );

        m.insert(
            "google_ads_conversion_action",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr(
                        "type",
                        FieldType::Enum(CONVERSION_ACTION_TYPE),
                        true,
                    ),
                    attr(
                        "category",
                        FieldType::Enum(CONVERSION_ACTION_CATEGORY),
                        true,
                    ),
                    attr(
                        "status",
                        FieldType::Enum(CONVERSION_ACTION_STATUS),
                        false,
                    )
                    .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                    attr(
                        "counting_type",
                        FieldType::Enum(CONVERSION_COUNTING_TYPE),
                        false,
                    ),
                    attr(
                        "click_through_lookback_window_days",
                        FieldType::Integer,
                        false,
                    ),
                    attr(
                        "view_through_lookback_window_days",
                        FieldType::Integer,
                        false,
                    ),
                ],
                blocks: vec![NestedBlockSchema {
                    name: "value_settings",
                    schema: BlockSchema {
                        attributes: vec![
                            attr("default_value", FieldType::Number, false),
                            attr("default_currency_code", FieldType::String, false),
                            attr("always_use_default_value", FieldType::Bool, false),
                        ],
                        blocks: vec![],
                    },
                }],
            },
        );

        m.insert(
            "google_ads_call_asset",
            BlockSchema {
                attributes: vec![
                    attr("country_code", FieldType::String, true),
                    attr("phone_number", FieldType::String, true),
                    attr(
                        "call_conversion_reporting_state",
                        FieldType::Enum(CALL_CONVERSION_REPORTING_STATE),
                        false,
                    ),
                    attr(
                        "call_conversion_action",
                        FieldType::RefOrResourceName(&["google_ads_conversion_action"]),
                        false,
                    ),
                ],
                blocks: vec![],
            },
        );

        // A sitelink Asset — a text extension that renders extra links below an
        // RSA. `final_urls` lives on the parent Asset; link text + descriptions
        // live in the `sitelink_asset` sub-object at the API level.
        m.insert(
            "google_ads_sitelink_asset",
            BlockSchema {
                attributes: vec![
                    attr("link_text", FieldType::String, true),
                    attr("description1", FieldType::String, false),
                    attr("description2", FieldType::String, false),
                    attr("final_urls", FieldType::list_of(FieldType::String), true),
                ],
                blocks: vec![],
            },
        );

        // A callout Asset — a short, non-clickable phrase that expands an RSA.
        m.insert(
            "google_ads_callout_asset",
            BlockSchema {
                attributes: vec![attr("text", FieldType::String, true)],
                blocks: vec![],
            },
        );

        // A structured-snippet Asset — a header plus a list of values.
        m.insert(
            "google_ads_structured_snippet_asset",
            BlockSchema {
                attributes: vec![
                    attr("header", FieldType::String, true),
                    attr("values", FieldType::list_of(FieldType::String), true),
                ],
                blocks: vec![],
            },
        );

        // A YouTube video Asset — a reference to a video already published on a
        // YouTube channel, addressed by its 11-char video id. `apply` creates the
        // asset from that id; it never uploads the video file (that is the YouTube
        // Data API's job, a separate system).
        m.insert(
            "google_ads_youtube_video_asset",
            BlockSchema {
                attributes: vec![
                    attr("youtube_video_id", FieldType::String, true),
                    attr("youtube_video_title", FieldType::String, false),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_shared_set",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr("type", FieldType::Enum(SHARED_SET_TYPE), false),
                    attr("status", FieldType::Enum(SHARED_SET_STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                ],
                blocks: vec![
                    negative_keyword_block(),
                    compact_keywords_block("negative_keywords"),
                ],
            },
        );

        m.insert(
            "google_ads_custom_audience",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr("description", FieldType::String, false),
                    attr("type", FieldType::Enum(CUSTOM_AUDIENCE_TYPE), false)
                        .with_default(DefaultValue::Str(DEFAULT_CUSTOM_AUDIENCE_TYPE)),
                    attr("status", FieldType::Enum(CUSTOM_AUDIENCE_STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                ],
                // Repeatable. Exactly one payload attribute per member;
                // enforced by validate_custom_audience_member.
                blocks: vec![NestedBlockSchema {
                    name: "member",
                    schema: BlockSchema {
                        attributes: vec![
                            attr("keyword", FieldType::String, false),
                            attr("url", FieldType::String, false),
                            attr("place_category", FieldType::String, false),
                            attr("app", FieldType::String, false),
                        ],
                        blocks: vec![],
                    },
                }],
            },
        );

        m.insert(
            "google_ads_shared_criterion",
            BlockSchema {
                attributes: vec![attr(
                    "shared_set",
                    FieldType::RefOrResourceName(&["google_ads_shared_set"]),
                    true,
                )],
                blocks: vec![keyword_block()],
            },
        );

        m.insert(
            "google_ads_campaign_shared_set",
            BlockSchema {
                attributes: vec![
                    attr(
                        "campaign",
                        FieldType::RefOrResourceName(&["google_ads_campaign"]),
                        true,
                    ),
                    attr(
                        "shared_set",
                        FieldType::RefOrResourceName(&["google_ads_shared_set"]),
                        true,
                    ),
                    attr(
                        "status",
                        FieldType::Enum(CAMPAIGN_SHARED_SET_STATUS),
                        false,
                    )
                    .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_customer_asset",
            BlockSchema {
                attributes: vec![
                    attr("asset", FieldType::Ref(ASSET_TYPES), false),
                    attr("assets", FieldType::list_of(FieldType::Ref(ASSET_TYPES)), false),
                    attr("field_type", FieldType::Enum(ASSET_FIELD_TYPE), false),
                    attr("status", FieldType::Enum(STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_campaign_asset",
            BlockSchema {
                attributes: vec![
                    attr(
                        "campaign",
                        FieldType::RefOrResourceName(&["google_ads_campaign"]),
                        true,
                    ),
                    attr("asset", FieldType::Ref(ASSET_TYPES), false),
                    attr("assets", FieldType::list_of(FieldType::Ref(ASSET_TYPES)), false),
                    attr("field_type", FieldType::Enum(ASSET_FIELD_TYPE), false),
                    attr("status", FieldType::Enum(STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_ad_group_asset",
            BlockSchema {
                attributes: vec![
                    attr(
                        "ad_group",
                        FieldType::RefOrResourceName(&["google_ads_ad_group"]),
                        true,
                    ),
                    attr("asset", FieldType::Ref(ASSET_TYPES), false),
                    attr("assets", FieldType::list_of(FieldType::Ref(ASSET_TYPES)), false),
                    attr("field_type", FieldType::Enum(ASSET_FIELD_TYPE), false),
                    attr("status", FieldType::Enum(STATUS), false)
                        .with_default(DefaultValue::Str(DEFAULT_STATUS)),
                ],
                blocks: vec![],
            },
        );

        m
    })
}

/// Public accessor for the render/`fmt` layer to look up a resource's schema
/// (and, through it, the per-attribute defaults to strip).
pub fn resource_schema(ty: &str) -> Option<&'static BlockSchema> {
    resource_schemas().get(ty)
}

/// Meta-block any `resource` may carry. It says how bidsmith is allowed to act
/// on the resource rather than what the resource *is*, so it belongs to no
/// resource type's schema and is validated separately (issue #115).
pub const LIFECYCLE_BLOCK: &str = "lifecycle";

/// Criterion resources are excluded: the Google Ads API creates criteria
/// freely, so there is no adopt-only workflow to declare, and one criterion
/// resource can fan out into several live members — leaving nothing single for
/// `create = false` to point at. Silently doing nothing would be worse than
/// saying so.
const LIFECYCLE_UNSUPPORTED_TYPES: &[&str] = &[
    "google_ads_ad_group_criterion",
    "google_ads_campaign_criterion",
    "google_ads_shared_criterion",
];

fn lifecycle_schema() -> &'static BlockSchema {
    static SCHEMA: OnceLock<BlockSchema> = OnceLock::new();
    SCHEMA.get_or_init(|| BlockSchema {
        attributes: vec![attr("create", FieldType::Bool, false)
            .with_default(DefaultValue::Bool(true))],
        blocks: vec![],
    })
}

/// True when `block` declares `lifecycle { create = false }` — the resource is
/// adopt-only and `plan` must never propose creating it.
pub fn declares_adopt_only(block: &Block) -> bool {
    block.body.iter().any(|s| match s {
        Structure::Block(b) if b.ident.as_str() == LIFECYCLE_BLOCK => {
            b.body.iter().any(|inner| match inner {
                Structure::Attribute(a) if a.key.as_str() == "create" => {
                    matches!(&a.value, Expression::Bool(v) if !*v.as_ref())
                }
                _ => false,
            })
        }
        _ => false,
    })
}

fn provider_schemas() -> &'static HashMap<&'static str, BlockSchema> {
    static SCHEMAS: OnceLock<HashMap<&'static str, BlockSchema>> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        let mut m = HashMap::new();
        m.insert(
            "google_ads",
            BlockSchema {
                attributes: vec![
                    attr("customer_id", FieldType::String, false),
                    attr("login_customer_id", FieldType::String, false),
                ],
                blocks: vec![],
            },
        );
        m
    })
}

struct ResourceDecl {
    file: String,
    module: String,
}

#[derive(Default)]
pub struct ResourceRegistry {
    by_qualified: HashMap<String, ResourceDecl>,
    by_short: HashMap<String, Vec<String>>,
}

impl ResourceRegistry {
    pub fn qualified(module: &str, ty: &str, name: &str) -> String {
        format!("{module}.{ty}.{name}")
    }

    fn short(ty: &str, name: &str) -> String {
        format!("{ty}.{name}")
    }

    pub fn resolve(&self, module: &str, ty: &str, name: &str) -> Resolution<'_> {
        let qualified = Self::qualified(module, ty, name);
        if self.by_qualified.contains_key(&qualified) {
            return Resolution::Found(qualified);
        }
        let short = Self::short(ty, name);
        match self.by_short.get(&short).map(Vec::as_slice).unwrap_or(&[]) {
            [] => Resolution::Missing,
            [only] => Resolution::Found(Self::qualified(only, ty, name)),
            many => Resolution::Ambiguous(many),
        }
    }

    /// True if exactly this `<module>.<type>.<name>` resource is declared,
    /// without the same-module-first / global-fallback resolution `resolve`
    /// applies. Used by `mv` to detect an occupied rename target.
    pub fn declared(&self, module: &str, ty: &str, name: &str) -> bool {
        self.by_qualified
            .contains_key(&Self::qualified(module, ty, name))
    }

    pub fn build(files: &[ParsedFile]) -> (Self, Vec<Diag>) {
        let mut registry = ResourceRegistry::default();
        let mut diags = Vec::new();
        for f in files {
            for s in f.body.iter() {
                let Structure::Block(b) = s else { continue };
                if b.ident.as_str() != "resource" || b.labels.len() != 2 {
                    continue;
                }
                let ty = b.labels[0].as_str();
                let name = b.labels[1].as_str();
                let qualified = Self::qualified(&f.module, ty, name);
                if let Some(prev) = registry.by_qualified.get(&qualified) {
                    let short = Self::short(ty, name);
                    let extra = if prev.module == f.module {
                        String::new()
                    } else {
                        format!(" (module '{}')", f.module)
                    };
                    diags.push(Diag::new(
                        f.src.clone(),
                        block_span(b),
                        format!(
                            "duplicate resource '{short}'{extra} (also declared at {})",
                            prev.file
                        ),
                    ));
                    continue;
                }
                registry.by_qualified.insert(
                    qualified,
                    ResourceDecl {
                        file: f.path.display().to_string(),
                        module: f.module.clone(),
                    },
                );
                registry
                    .by_short
                    .entry(Self::short(ty, name))
                    .or_default()
                    .push(f.module.clone());
            }
        }
        (registry, diags)
    }
}

pub enum Resolution<'a> {
    Found(String),
    Missing,
    Ambiguous(&'a [String]),
}

#[derive(Default, Clone)]
pub struct InputBindings {
    pub vars: HashMap<String, String>,
}

impl InputBindings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn merge_env(&mut self) {
        for (k, v) in std::env::vars() {
            if let Some(name) = k.strip_prefix("BIDSMITH_VAR_") {
                if !name.is_empty() {
                    self.vars.entry(name.to_string()).or_insert(v);
                }
            }
        }
    }
}

pub struct LocalDecl {
    pub module: String,
    pub value: Expression,
}

#[derive(Default)]
pub struct LocalsRegistry {
    by_qualified: HashMap<String, LocalDecl>,
    by_short: HashMap<String, Vec<String>>,
}

impl LocalsRegistry {
    pub fn qualified(module: &str, name: &str) -> String {
        format!("{module}.{name}")
    }

    pub fn resolve(&self, module: &str, name: &str) -> Resolution<'_> {
        let qualified = Self::qualified(module, name);
        if self.by_qualified.contains_key(&qualified) {
            return Resolution::Found(qualified);
        }
        match self.by_short.get(name).map(Vec::as_slice).unwrap_or(&[]) {
            [] => Resolution::Missing,
            [only] => Resolution::Found(Self::qualified(only, name)),
            many => Resolution::Ambiguous(many),
        }
    }

    pub fn get(&self, qualified: &str) -> Option<&LocalDecl> {
        self.by_qualified.get(qualified)
    }

    pub fn build(files: &[ParsedFile]) -> (Self, Vec<Diag>) {
        let mut registry = LocalsRegistry::default();
        let mut diags = Vec::new();
        for f in files {
            for s in f.body.iter() {
                let Structure::Block(b) = s else { continue };
                if b.ident.as_str() != "locals" {
                    continue;
                }
                if !b.labels.is_empty() {
                    diags.push(Diag::new(
                        f.src.clone(),
                        span_of(b.labels[0].span()),
                        "'locals' block does not take labels".to_string(),
                    ));
                }
                let mut seen: HashSet<String> = HashSet::new();
                for inner in b.body.iter() {
                    match inner {
                        Structure::Attribute(a) => {
                            let name = a.key.as_str().to_string();
                            if !seen.insert(name.clone()) {
                                diags.push(Diag::new(
                                    f.src.clone(),
                                    span_of(a.key.span()),
                                    format!(
                                        "duplicate local '{name}' in module '{}'",
                                        f.module
                                    ),
                                ));
                                continue;
                            }
                            let qualified = Self::qualified(&f.module, &name);
                            if registry.by_qualified.contains_key(&qualified) {
                                diags.push(Diag::new(
                                    f.src.clone(),
                                    span_of(a.key.span()),
                                    format!(
                                        "duplicate local '{name}' in module '{}'",
                                        f.module
                                    ),
                                ));
                                continue;
                            }
                            registry.by_qualified.insert(
                                qualified,
                                LocalDecl {
                                    module: f.module.clone(),
                                    value: a.value.clone(),
                                },
                            );
                            registry
                                .by_short
                                .entry(name)
                                .or_default()
                                .push(f.module.clone());
                        }
                        Structure::Block(inner_block) => {
                            diags.push(Diag::new(
                                f.src.clone(),
                                span_of(inner_block.ident.span()),
                                format!(
                                    "nested block '{}' is not allowed inside 'locals' — locals only takes attributes",
                                    inner_block.ident.as_str()
                                ),
                            ));
                        }
                    }
                }
            }
        }
        (registry, diags)
    }
}

pub struct AdTemplateDecl {
    pub block: Block,
}

/// Reusable `ad {}` bodies attached via `template = ad_template.<name>`; resolves same-module then global, like resources.
#[derive(Default)]
pub struct AdTemplateRegistry {
    by_qualified: HashMap<String, AdTemplateDecl>,
    by_short: HashMap<String, Vec<String>>,
}

impl AdTemplateRegistry {
    pub fn qualified(module: &str, name: &str) -> String {
        format!("{module}.{name}")
    }

    pub fn resolve(&self, module: &str, name: &str) -> Resolution<'_> {
        let qualified = Self::qualified(module, name);
        if self.by_qualified.contains_key(&qualified) {
            return Resolution::Found(qualified);
        }
        match self.by_short.get(name).map(Vec::as_slice).unwrap_or(&[]) {
            [] => Resolution::Missing,
            [only] => Resolution::Found(Self::qualified(only, name)),
            many => Resolution::Ambiguous(many),
        }
    }

    pub fn get(&self, qualified: &str) -> Option<&AdTemplateDecl> {
        self.by_qualified.get(qualified)
    }

    pub fn build(files: &[ParsedFile]) -> (Self, Vec<Diag>) {
        let mut registry = AdTemplateRegistry::default();
        let mut diags = Vec::new();
        for f in files {
            for s in f.body.iter() {
                let Structure::Block(b) = s else { continue };
                if b.ident.as_str() != "ad_template" {
                    continue;
                }
                if b.labels.len() != 1 {
                    diags.push(Diag::new(
                        f.src.clone(),
                        span_of(b.ident.span()),
                        format!(
                            "'ad_template' block requires exactly one label (the template name), got {}",
                            b.labels.len()
                        ),
                    ));
                    continue;
                }
                let name = b.labels[0].as_str().to_string();
                let qualified = Self::qualified(&f.module, &name);
                if registry.by_qualified.contains_key(&qualified) {
                    diags.push(Diag::new(
                        f.src.clone(),
                        span_of(b.labels[0].span()),
                        format!("duplicate ad_template '{name}' in module '{}'", f.module),
                    ));
                    continue;
                }
                registry
                    .by_qualified
                    .insert(qualified, AdTemplateDecl { block: b.clone() });
                registry
                    .by_short
                    .entry(name)
                    .or_default()
                    .push(f.module.clone());
            }
        }
        (registry, diags)
    }
}

pub struct DefaultsDecl {
    pub file: String,
    pub block: Block,
}

/// The meta-attribute a resource uses to opt into a named `defaults` block.
pub const DEFAULTS_ATTR: &str = "defaults";

/// Type-scoped attribute/block defaults from top-level `defaults "<type>" {}`
/// blocks. A resource's own attribute or nested block always wins (blocks
/// override wholesale, no deep merge).
///
/// An unlabeled block applies to every resource of its type. A block with a
/// second label is opt-in: only resources naming it with
/// `defaults = defaults.<name>` pick it up, and for those it *replaces* the
/// unlabeled block rather than layering on top — one visible source per
/// resource, so a reviewer reading the opt-in knows the whole story
/// (issue #145).
#[derive(Default)]
pub struct DefaultsRegistry {
    by_type: HashMap<String, DefaultsDecl>,
    by_name: HashMap<(String, String), DefaultsDecl>,
}

/// The `<name>` in a `defaults = defaults.<name>` opt-in, else `None`.
pub fn defaults_ref_name(expr: &Expression) -> Option<String> {
    let Expression::Traversal(t) = expr else {
        return None;
    };
    let path = extract_traversal_path(t)?;
    if path.len() != 2 || path[0] != DEFAULTS_ATTR {
        return None;
    }
    Some(path[1].clone())
}

/// The name a resource block opts into, if any.
pub fn declared_defaults_name(block: &Block) -> Option<String> {
    block.body.iter().find_map(|s| match s {
        Structure::Attribute(a) if a.key.as_str() == DEFAULTS_ATTR => {
            defaults_ref_name(&a.value)
        }
        _ => None,
    })
}

impl DefaultsRegistry {
    fn decl_for(&self, ty: &str, name: Option<&str>) -> Option<&DefaultsDecl> {
        match name {
            Some(n) => self.by_name.get(&(ty.to_string(), n.to_string())),
            None => self.by_type.get(ty),
        }
    }

    pub fn has_named(&self, ty: &str, name: &str) -> bool {
        self.by_name.contains_key(&(ty.to_string(), name.to_string()))
    }

    /// Names declared for a type, for the "did you mean" tail of an error.
    pub fn names_for(&self, ty: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .by_name
            .keys()
            .filter(|(t, _)| t == ty)
            .map(|(_, n)| n.clone())
            .collect();
        out.sort();
        out
    }

    pub fn provided_attrs_named(&self, ty: &str, name: Option<&str>) -> HashSet<String> {
        let Some(decl) = self.decl_for(ty, name) else {
            return HashSet::new();
        };
        decl.block
            .body
            .iter()
            .filter_map(|s| match s {
                Structure::Attribute(a) => Some(a.key.as_str().to_string()),
                _ => None,
            })
            .collect()
    }

    pub fn provided_attrs(&self, ty: &str) -> HashSet<String> {
        self.provided_attrs_named(ty, None)
    }

    pub fn provided_blocks(&self, ty: &str) -> HashSet<String> {
        let Some(decl) = self.by_type.get(ty) else {
            return HashSet::new();
        };
        decl.block
            .body
            .iter()
            .filter_map(|s| match s {
                Structure::Block(b) => Some(b.ident.as_str().to_string()),
                _ => None,
            })
            .collect()
    }

    /// The resource block with missing attributes / nested blocks filled in
    /// from the defaults it opts into (or its type's unlabeled block);
    /// `None` when nothing applies.
    pub fn merge(&self, ty: &str, block: &Block) -> Option<Block> {
        let opted = declared_defaults_name(block);
        let decl = self.decl_for(ty, opted.as_deref())?;
        let have_attrs: HashSet<&str> = block
            .body
            .iter()
            .filter_map(|s| match s {
                Structure::Attribute(a) => Some(a.key.as_str()),
                _ => None,
            })
            .collect();
        let have_blocks: HashSet<&str> = block
            .body
            .iter()
            .filter_map(|s| match s {
                Structure::Block(b) => Some(b.ident.as_str()),
                _ => None,
            })
            .collect();
        // A bidding block is one slot, not five: a campaign picking `target_cpv`
        // must not also inherit the defaults' `manual_cpc`, or the merged
        // resource declares two alternatives of the same `oneof`.
        let have_bidding = ty == "google_ads_campaign"
            && have_blocks
                .iter()
                .any(|b| CAMPAIGN_BIDDING_BLOCKS.contains(b));
        let additions: Vec<&Structure> = decl
            .block
            .body
            .iter()
            .filter(|s| match s {
                Structure::Attribute(a) => !have_attrs.contains(a.key.as_str()),
                Structure::Block(b) => {
                    !have_blocks.contains(b.ident.as_str())
                        && !(have_bidding && CAMPAIGN_BIDDING_BLOCKS.contains(&b.ident.as_str()))
                }
            })
            .collect();
        if additions.is_empty() {
            return None;
        }
        let mut merged = block.clone();
        for s in additions {
            merged.body.push(s.clone());
        }
        Some(merged)
    }

    pub fn build(files: &[ParsedFile]) -> (Self, Vec<Diag>) {
        let mut registry = DefaultsRegistry::default();
        let mut diags = Vec::new();
        for f in files {
            for s in f.body.iter() {
                let Structure::Block(b) = s else { continue };
                if b.ident.as_str() != "defaults" {
                    continue;
                }
                if b.labels.is_empty() || b.labels.len() > 2 {
                    diags.push(Diag::new(
                        f.src.clone(),
                        span_of(b.ident.span()),
                        format!(
                            "'defaults' block takes the resource type, plus an optional name \
                             resources opt into with 'defaults = defaults.<name>' — got {} label(s)",
                            b.labels.len()
                        ),
                    ));
                    continue;
                }
                let ty = b.labels[0].as_str().to_string();
                let decl_name = b.labels.get(1).map(|l| l.as_str().to_string());
                if !resource_schemas().contains_key(ty.as_str()) {
                    diags.push(Diag::new(
                        f.src.clone(),
                        span_of(b.labels[0].span()),
                        format!("unknown resource type '{ty}' in defaults block"),
                    ));
                    continue;
                }
                if ty == "google_ads_ad_group_ad" {
                    let offending = b.body.iter().find_map(|s| match s {
                        Structure::Attribute(a) if a.key.as_str() == "template" => {
                            Some(span_of(a.key.span()))
                        }
                        Structure::Block(ib) if ib.ident.as_str() == "ad" => {
                            Some(span_of(ib.ident.span()))
                        }
                        _ => None,
                    });
                    if let Some(span) = offending {
                        diags.push(Diag::new(
                            f.src.clone(),
                            span,
                            "defaults cannot provide an ad body: declare 'ad' or 'template' on each google_ads_ad_group_ad (use ad_template for reusable bodies)".to_string(),
                        ));
                        continue;
                    }
                }
                let decl = DefaultsDecl {
                    file: f.path.display().to_string(),
                    block: b.clone(),
                };
                match decl_name {
                    Some(name) => {
                        if let Some(prev) = registry.by_name.get(&(ty.clone(), name.clone())) {
                            diags.push(Diag::new(
                                f.src.clone(),
                                span_of(b.labels[1].span()),
                                format!(
                                    "duplicate defaults '{name}' for '{ty}' (also declared at {}); one block per name per resource type",
                                    prev.file
                                ),
                            ));
                            continue;
                        }
                        registry.by_name.insert((ty, name), decl);
                    }
                    None => {
                        if let Some(prev) = registry.by_type.get(&ty) {
                            diags.push(Diag::new(
                                f.src.clone(),
                                span_of(b.labels[0].span()),
                                format!(
                                    "duplicate defaults for '{ty}' (also declared at {}); one unnamed defaults block per resource type — give one a name and opt in with 'defaults = defaults.<name>'",
                                    prev.file
                                ),
                            ));
                            continue;
                        }
                        registry.by_type.insert(ty, decl);
                    }
                }
            }
        }
        (registry, diags)
    }
}

/// The `<name>` in an `ad_template.<name>` traversal, else `None`.
pub fn ad_template_ref_name(expr: &Expression) -> Option<String> {
    let Expression::Traversal(t) = expr else {
        return None;
    };
    let path = extract_traversal_path(t)?;
    if path.len() != 2 || path[0] != "ad_template" {
        return None;
    }
    Some(path[1].clone())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VarType {
    String,
    Number,
    Bool,
}

impl VarType {
    fn from_ident(ident: &str) -> Option<VarType> {
        match ident {
            "string" => Some(VarType::String),
            "number" => Some(VarType::Number),
            "bool" => Some(VarType::Bool),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            VarType::String => "string",
            VarType::Number => "number",
            VarType::Bool => "bool",
        }
    }
}

pub struct VariableDecl {
    pub module: String,
    pub value: Expression,
}

#[derive(Default)]
pub struct VariablesRegistry {
    by_qualified: HashMap<String, VariableDecl>,
    by_short: HashMap<String, Vec<String>>,
}

pub struct Bindings {
    pub locals: LocalsRegistry,
    pub variables: VariablesRegistry,
}

impl Bindings {
    pub fn build(files: &[ParsedFile], inputs: &InputBindings) -> (Self, Vec<Diag>) {
        let (locals, mut diags) = LocalsRegistry::build(files);
        let (variables, var_diags) = VariablesRegistry::build(files, inputs);
        diags.extend(var_diags);
        (Self { locals, variables }, diags)
    }

    /// Best-effort evaluation: follows `local.`/`var.` chains and renders
    /// string templates; on any failure the original expression is returned
    /// unchanged (the validator reports evaluation errors with spans).
    pub fn resolve_value<'a>(
        &'a self,
        from_module: &str,
        expr: &'a Expression,
    ) -> Cow<'a, Expression> {
        let ctx = EvalCtx {
            locals: &self.locals,
            variables: &self.variables,
        };
        ctx.eval(from_module, expr)
            .unwrap_or(Cow::Borrowed(expr))
    }
}

impl VariablesRegistry {
    pub fn qualified(module: &str, name: &str) -> String {
        format!("{module}.{name}")
    }

    pub fn resolve(&self, module: &str, name: &str) -> Resolution<'_> {
        let qualified = Self::qualified(module, name);
        if self.by_qualified.contains_key(&qualified) {
            return Resolution::Found(qualified);
        }
        match self.by_short.get(name).map(Vec::as_slice).unwrap_or(&[]) {
            [] => Resolution::Missing,
            [only] => Resolution::Found(Self::qualified(only, name)),
            many => Resolution::Ambiguous(many),
        }
    }

    pub fn get(&self, qualified: &str) -> Option<&VariableDecl> {
        self.by_qualified.get(qualified)
    }

    pub fn build(files: &[ParsedFile], inputs: &InputBindings) -> (Self, Vec<Diag>) {
        let mut registry = VariablesRegistry::default();
        let mut diags = Vec::new();
        for f in files {
            for s in f.body.iter() {
                let Structure::Block(b) = s else { continue };
                if b.ident.as_str() != "variable" {
                    continue;
                }
                if b.labels.len() != 1 {
                    diags.push(Diag::new(
                        f.src.clone(),
                        span_of(b.ident.span()),
                        format!(
                            "'variable' block requires exactly one label (the variable name), got {}",
                            b.labels.len()
                        ),
                    ));
                    continue;
                }
                let name = b.labels[0].as_str().to_string();
                let mut declared_type: Option<VarType> = None;
                let mut default_expr: Option<&Expression> = None;
                let mut default_span: Option<std::ops::Range<usize>> = None;
                let mut seen: HashSet<&str> = HashSet::new();
                let mut had_body_err = false;
                for inner in b.body.iter() {
                    match inner {
                        Structure::Attribute(a) => {
                            let key = a.key.as_str();
                            if !seen.insert(key) {
                                diags.push(Diag::new(
                                    f.src.clone(),
                                    span_of(a.key.span()),
                                    format!("duplicate attribute '{key}' in variable '{name}'"),
                                ));
                                had_body_err = true;
                                continue;
                            }
                            match key {
                                "type" => match &a.value {
                                    Expression::Variable(v) => {
                                        match VarType::from_ident(v.as_str()) {
                                            Some(ty) => declared_type = Some(ty),
                                            None => {
                                                diags.push(Diag::new(
                                                    f.src.clone(),
                                                    span_of(a.value.span()),
                                                    format!(
                                                        "invalid variable type '{}'; expected one of [string, number, bool]",
                                                        v.as_str()
                                                    ),
                                                ));
                                                had_body_err = true;
                                            }
                                        }
                                    }
                                    other => {
                                        diags.push(Diag::new(
                                            f.src.clone(),
                                            span_of(a.value.span()),
                                            format!(
                                                "variable 'type' must be one of [string, number, bool] (as a bare identifier), got {}",
                                                describe_expr(other)
                                            ),
                                        ));
                                        had_body_err = true;
                                    }
                                },
                                "default" => {
                                    default_expr = Some(&a.value);
                                    default_span = Some(span_of(a.value.span()));
                                }
                                "description" => {
                                    if !matches!(a.value, Expression::String(_)) {
                                        diags.push(Diag::new(
                                            f.src.clone(),
                                            span_of(a.value.span()),
                                            format!(
                                                "variable 'description' must be a string, got {}",
                                                describe_expr(&a.value)
                                            ),
                                        ));
                                        had_body_err = true;
                                    }
                                }
                                other => {
                                    diags.push(Diag::new(
                                        f.src.clone(),
                                        span_of(a.key.span()),
                                        format!(
                                            "unknown attribute '{other}' in variable '{name}'; allowed: type, default, description"
                                        ),
                                    ));
                                    had_body_err = true;
                                }
                            }
                        }
                        Structure::Block(inner_block) => {
                            diags.push(Diag::new(
                                f.src.clone(),
                                span_of(inner_block.ident.span()),
                                format!(
                                    "nested block '{}' is not allowed inside 'variable' — variable only takes attributes",
                                    inner_block.ident.as_str()
                                ),
                            ));
                            had_body_err = true;
                        }
                    }
                }
                let Some(ty) = declared_type else {
                    if !seen.contains("type") {
                        diags.push(Diag::new(
                            f.src.clone(),
                            span_of(b.ident.span()),
                            format!(
                                "variable '{name}' is missing required attribute 'type' (one of [string, number, bool])"
                            ),
                        ));
                    }
                    continue;
                };
                if had_body_err {
                    continue;
                }
                if let Some(expr) = default_expr {
                    if let Err(msg) = check_literal_matches(expr, ty) {
                        diags.push(Diag::new(
                            f.src.clone(),
                            default_span.clone().unwrap_or_else(|| span_of(b.ident.span())),
                            msg,
                        ));
                        continue;
                    }
                }

                let resolved = match inputs.vars.get(&name) {
                    Some(raw) => match parse_input_value(raw, ty) {
                        Ok(expr) => expr,
                        Err(msg) => {
                            diags.push(Diag::new(
                                f.src.clone(),
                                span_of(b.labels[0].span()),
                                format!(
                                    "invalid input for variable '{name}': {msg} (got \"{raw}\", expected {})",
                                    ty.name()
                                ),
                            ));
                            continue;
                        }
                    },
                    None => match default_expr {
                        Some(expr) => expr.clone(),
                        None => {
                            diags.push(Diag::new(
                                f.src.clone(),
                                span_of(b.labels[0].span()),
                                format!(
                                    "variable '{name}' has no value: set --var {name}=… or $BIDSMITH_VAR_{name}, or add a default"
                                ),
                            ));
                            continue;
                        }
                    },
                };

                let qualified = Self::qualified(&f.module, &name);
                if registry.by_qualified.contains_key(&qualified) {
                    diags.push(Diag::new(
                        f.src.clone(),
                        span_of(b.labels[0].span()),
                        format!("duplicate variable '{name}' in module '{}'", f.module),
                    ));
                    continue;
                }
                registry.by_qualified.insert(
                    qualified,
                    VariableDecl {
                        module: f.module.clone(),
                        value: resolved,
                    },
                );
                registry
                    .by_short
                    .entry(name)
                    .or_default()
                    .push(f.module.clone());
            }
        }
        (registry, diags)
    }
}

fn check_literal_matches(expr: &Expression, ty: VarType) -> Result<(), String> {
    let ok = matches!(
        (ty, expr),
        (VarType::String, Expression::String(_))
            | (VarType::Number, Expression::Number(_))
            | (VarType::Bool, Expression::Bool(_))
    );
    if ok {
        Ok(())
    } else {
        Err(format!(
            "variable default expects {}, got {}",
            ty.name(),
            describe_expr(expr)
        ))
    }
}

fn parse_input_value(raw: &str, ty: VarType) -> Result<Expression, String> {
    match ty {
        VarType::String => Ok(format!("\"{}\"", raw.replace('\\', "\\\\").replace('"', "\\\""))
            .parse::<Expression>()
            .map_err(|e| format!("could not parse as string literal: {e}"))?),
        VarType::Number => raw
            .parse::<f64>()
            .map_err(|_| "could not parse as number".to_string())
            .and_then(|_| {
                raw.parse::<Expression>()
                    .map_err(|e| format!("could not parse number literal: {e}"))
            }),
        VarType::Bool => match raw {
            "true" => "true"
                .parse::<Expression>()
                .map_err(|e| format!("internal: {e}")),
            "false" => "false"
                .parse::<Expression>()
                .map_err(|e| format!("internal: {e}")),
            _ => Err("expected 'true' or 'false'".to_string()),
        },
    }
}

pub fn validate_files(files: &[ParsedFile], inputs: &InputBindings) -> Vec<Diag> {
    let (expanded, expand_diags) = crate::expand::expand_resource_for_each(files, inputs);
    let files = &expanded[..];
    let (registry, mut diags) = ResourceRegistry::build(files);
    diags.extend(expand_diags);
    let (locals, locals_diags) = LocalsRegistry::build(files);
    diags.extend(locals_diags);
    let (variables, variables_diags) = VariablesRegistry::build(files, inputs);
    diags.extend(variables_diags);
    let (templates, template_diags) = AdTemplateRegistry::build(files);
    diags.extend(template_diags);
    let (defaults, defaults_diags) = DefaultsRegistry::build(files);
    diags.extend(defaults_diags);

    for f in files {
        validate_top_level(f, &registry, &locals, &variables, &defaults, &mut diags);
    }

    validate_ad_templates(files, &templates, &registry, &locals, &variables, &mut diags);
    validate_targeting_conflicts(files, &registry, &defaults, &mut diags);
    validate_targeting_setting_conflicts(files, &registry, &defaults, &mut diags);

    diags.sort_by(|a, b| {
        (a.src.name(), a.span.offset()).cmp(&(b.src.name(), b.span.offset()))
    });
    diags
}

/// Validate `ad_template` bodies against the `ad {}` schema and enforce the per-ad ad/template XOR + reference resolution.
fn validate_ad_templates(
    files: &[ParsedFile],
    templates: &AdTemplateRegistry,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    let ad_schema = ad_block(false);
    for f in files {
        for s in f.body.iter() {
            let Structure::Block(b) = s else { continue };
            match b.ident.as_str() {
                "ad_template" if b.labels.len() == 1 => {
                    let address = format!("ad_template.{}", b.labels[0].as_str());
                    validate_body(
                        f,
                        b,
                        &b.body,
                        &ad_schema.schema,
                        &address,
                        registry,
                        locals,
                        variables,
                        RequiredCheck::Enforce,
                        diags,
                    );
                    validate_ad_creative_exclusivity(f, &b.body, &address, diags);
                }
                "resource"
                    if b.labels.len() == 2
                        && b.labels[0].as_str() == "google_ads_ad_group_ad" =>
                {
                    validate_ad_group_ad_template(
                        f, b, templates, registry, locals, variables, diags,
                    );
                    if let Some(ad) = find_child_block(&b.body, "ad") {
                        let address = format!("google_ads_ad_group_ad.{}", b.labels[1].as_str());
                        validate_ad_creative_exclusivity(f, &ad.body, &address, diags);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Check an ad's `inputs = { … }` against the parameters its template actually
/// references. A template's parameters are the `input.<name>` names in its body
/// — there is no second list to drift — so both directions are checkable: a
/// missing binding leaves a dangling `input.x` in the mutate, and a surplus one
/// is a typo that would otherwise do nothing at all.
fn validate_template_inputs(
    file: &ParsedFile,
    address: &str,
    template_name: &str,
    decl: &AdTemplateDecl,
    inputs: Option<&hcl_edit::structure::Attribute>,
    fallback_span: std::ops::Range<usize>,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    let params = crate::expand::template_params(&decl.block);
    if params.is_empty() && inputs.is_none() {
        return;
    }
    let (span, supplied) = match inputs {
        Some(a) => {
            let Expression::Object(obj) = &a.value else {
                diags.push(Diag::new(
                    file.src.clone(),
                    span_of(a.value.span()),
                    format!(
                        "{address} 'inputs' must be a map of name = value, got {}",
                        describe_expr(&a.value)
                    ),
                ));
                return;
            };
            let mut names: Vec<String> = Vec::new();
            for (key, _) in obj.iter() {
                match crate::expand::object_key_str(key) {
                    Some(k) => names.push(k),
                    None => diags.push(Diag::new(
                        file.src.clone(),
                        span_of(a.value.span()),
                        format!("{address} 'inputs' keys must be identifiers or strings"),
                    )),
                }
            }
            (span_of(a.value.span()), names)
        }
        None => (fallback_span.clone(), Vec::new()),
    };

    let missing: Vec<&String> = params.iter().filter(|p| !supplied.contains(p)).collect();
    if !missing.is_empty() {
        diags.push(Diag::new(
            file.src.clone(),
            span.clone(),
            format!(
                "{address} uses 'ad_template.{template_name}', which needs {} — add {} to 'inputs'",
                params
                    .iter()
                    .map(|p| format!("input.{p}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                missing
                    .iter()
                    .map(|p| p.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ));
    }
    for name in &supplied {
        if !params.contains(name) {
            let known = if params.is_empty() {
                format!("ad_template.{template_name} takes no inputs")
            } else {
                format!("it takes {}", params.join(", "))
            };
            diags.push(Diag::new(
                file.src.clone(),
                span.clone(),
                format!("{address} passes input '{name}' that its template never uses — {known}"),
            ));
        }
    }

    // The declaration could not type-check its own placeholders; the body with
    // real values spliced in is the first point where `pin = 3` is visibly
    // wrong, so check it here and anchor the complaint at the use site.
    if let Some(attr) = inputs {
        let mut bindings: std::collections::HashMap<String, Expression> =
            std::collections::HashMap::new();
        if let Expression::Object(obj) = &attr.value {
            for (key, value) in obj.iter() {
                if let Some(k) = crate::expand::object_key_str(key) {
                    bindings.insert(k, value.expr().clone());
                }
            }
        }
        let bound = crate::expand::bind_template_inputs(&decl.block, &bindings);
        let mut bound_diags = Vec::new();
        validate_body(
            file,
            &bound,
            &bound.body,
            &ad_block(false).schema,
            &format!("{address} (via ad_template.{template_name})"),
            registry,
            locals,
            variables,
            RequiredCheck::Enforce,
            &mut bound_diags,
        );
        for d in bound_diags {
            diags.push(Diag::new(file.src.clone(), span.clone(), d.message));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_ad_group_ad_template(
    file: &ParsedFile,
    block: &Block,
    templates: &AdTemplateRegistry,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    let address = format!("google_ads_ad_group_ad.{}", block.labels[1].as_str());
    let mut has_ad_block = false;
    let mut template: Option<(std::ops::Range<usize>, &Expression)> = None;
    let mut overrides: Vec<(&str, std::ops::Range<usize>)> = Vec::new();
    let mut has_final_urls_override = false;
    let mut inputs: Option<&hcl_edit::structure::Attribute> = None;
    for s in block.body.iter() {
        match s {
            Structure::Block(b) if b.ident.as_str() == "ad" => has_ad_block = true,
            Structure::Attribute(a) => match a.key.as_str() {
                "template" => template = Some((span_of(a.key.span()), &a.value)),
                crate::expand::TEMPLATE_INPUTS_ATTR => inputs = Some(a),
                "final_urls" => {
                    overrides.push(("final_urls", span_of(a.key.span())));
                    if !is_empty_array_literal(&a.value) {
                        has_final_urls_override = true;
                    }
                }
                "path1" | "path2" | "final_url_suffix" | "custom_parameters" => {
                    overrides.push((a.key.as_str(), span_of(a.key.span())))
                }
                _ => {}
            },
            _ => {}
        }
    }

    // `final_urls` / `path1` / `path2` on the resource override a `template` body. With
    // an inline `ad {}` they belong inside it; with neither they have nothing to override.
    if has_ad_block {
        for (name, key_span) in &overrides {
            diags.push(Diag::new(
                file.src.clone(),
                key_span.clone(),
                format!(
                    "{address} sets '{name}' alongside an inline 'ad' block; move it inside the ad block ('{name}' on the resource overrides a 'template' body)"
                ),
            ));
        }
    } else if template.is_none() {
        for (name, key_span) in &overrides {
            diags.push(Diag::new(
                file.src.clone(),
                key_span.clone(),
                format!(
                    "{address} sets '{name}' without a 'template'; it only overrides a 'template = ad_template.<name>' body"
                ),
            ));
        }
    }

    match (has_ad_block, template) {
        (true, Some((key_span, _))) => diags.push(Diag::new(
            file.src.clone(),
            key_span,
            format!("{address} sets both an 'ad' block and 'template'; use one or the other"),
        )),
        (false, None) => diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "{address} must declare an ad: add an 'ad {{ … }}' block or 'template = ad_template.<name>'"
            ),
        )),
        (false, Some((_, value))) => {
            let Some(name) = ad_template_ref_name(value) else {
                return;
            };
            match templates.resolve(&file.module, &name) {
                Resolution::Found(qualified) => {
                    if let Some(decl) = templates.get(&qualified) {
                        validate_template_inputs(
                            file,
                            &address,
                            &name,
                            decl,
                            inputs,
                            span_of(value.span()),
                            registry,
                            locals,
                            variables,
                            diags,
                        );
                    }
                    let template_has_final_urls = templates
                        .get(&qualified)
                        .map(|d| body_has_attr(&d.block.body, "final_urls"))
                        .unwrap_or(false);
                    if !template_has_final_urls && !has_final_urls_override {
                        diags.push(Diag::new(
                            file.src.clone(),
                            span_of(value.span()),
                            format!(
                                "{address} uses 'ad_template.{name}', which declares no final_urls; add 'final_urls = [...]' to {address}"
                            ),
                        ));
                    }
                }
                Resolution::Missing => diags.push(Diag::new(
                    file.src.clone(),
                    span_of(value.span()),
                    format!("reference to undeclared ad_template 'ad_template.{name}'"),
                )),
                Resolution::Ambiguous(modules) => {
                    let mut sorted: Vec<&str> = modules.iter().map(String::as_str).collect();
                    sorted.sort();
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(value.span()),
                        format!(
                            "ambiguous reference to 'ad_template.{name}'; declared in modules [{}] — rename one so each is unique within its module",
                            sorted.join(", ")
                        ),
                    ));
                }
            }
        }
        (true, None) => {}
    }
}

fn find_child_block<'a>(body: &'a Body, name: &str) -> Option<&'a Block> {
    body.iter().find_map(|s| match s {
        Structure::Block(b) if b.ident.as_str() == name => Some(b),
        _ => None,
    })
}

/// An `ad {}` body (inline or in an `ad_template`) may carry at most one creative:
/// a `responsive_search_ad`, a `video_responsive_ad`, a `video_ad`, or a
/// `demand_gen_video_responsive_ad` — never more than one.
fn validate_ad_creative_exclusivity(
    file: &ParsedFile,
    ad_body: &Body,
    address: &str,
    diags: &mut Vec<Diag>,
) {
    let creatives = [
        "responsive_search_ad",
        "video_responsive_ad",
        "video_ad",
        "demand_gen_video_responsive_ad",
    ];
    let present: Vec<&Block> = creatives
        .iter()
        .filter_map(|name| find_child_block(ad_body, name))
        .collect();
    if present.len() > 1 {
        let offender = present[1];
        diags.push(Diag::new(
            file.src.clone(),
            span_of(offender.ident.span()),
            format!(
                "{address} declares more than one creative ({}); an ad has one creative — use only one",
                creatives.join(" / ")
            ),
        ));
    }
}

#[derive(Default, Clone, Copy)]
struct InlineAxes {
    languages: bool,
    locations: bool,
    /// Which spelling declared the device axis, so the conflict diagnostic
    /// names the attribute the author actually wrote.
    devices: Option<&'static str>,
}

/// A campaign can declare targeting *either* inline (`languages` / `locations`
/// / `devices`) *or* via explicit positive `google_ads_campaign_criterion`
/// resources — not both for the same axis. (Negative locations, proximity,
/// keywords, and non-positive criteria are unaffected — they only live in
/// explicit form.)
fn validate_targeting_conflicts(
    files: &[ParsedFile],
    registry: &ResourceRegistry,
    defaults: &DefaultsRegistry,
    diags: &mut Vec<Diag>,
) {
    let campaign_defaults = defaults.provided_attrs("google_ads_campaign");
    let default_axes = InlineAxes {
        languages: campaign_defaults.contains("languages"),
        locations: campaign_defaults.contains("locations"),
        devices: if campaign_defaults.contains("devices") {
            Some("devices")
        } else if campaign_defaults.contains("excluded_devices") {
            Some("excluded_devices")
        } else {
            None
        },
    };
    let mut inline: HashMap<String, InlineAxes> = HashMap::new();
    for f in files {
        for s in f.body.iter() {
            let Structure::Block(b) = s else { continue };
            if b.ident.as_str() != "resource" || b.labels.len() != 2 {
                continue;
            }
            if b.labels[0].as_str() != "google_ads_campaign" {
                continue;
            }
            let name = b.labels[1].as_str();
            let mut axes = default_axes;
            let mut devices_span = None;
            let mut excluded_span = None;
            for inner in b.body.iter() {
                if let Structure::Attribute(a) = inner {
                    match a.key.as_str() {
                        "languages" => axes.languages = true,
                        "locations" => axes.locations = true,
                        "devices" => {
                            axes.devices = Some("devices");
                            devices_span = Some(span_of(a.key.span()));
                        }
                        "excluded_devices" => {
                            if axes.devices.is_none() {
                                axes.devices = Some("excluded_devices");
                            }
                            excluded_span = Some(span_of(a.key.span()));
                        }
                        _ => {}
                    }
                }
            }
            // Two spellings of one axis: `devices` already excludes everything
            // it omits, so a second list saying so again can only contradict it.
            if let (Some(_), Some(span)) = (devices_span, excluded_span) {
                diags.push(Diag::new(
                    f.src.clone(),
                    span,
                    format!(
                        "google_ads_campaign.{name} declares both 'devices' and \
                         'excluded_devices'; 'devices' already excludes every device \
                         type it omits — keep one"
                    ),
                ));
            }
            if axes.languages || axes.locations || axes.devices.is_some() {
                inline.insert(
                    ResourceRegistry::qualified(&f.module, "google_ads_campaign", name),
                    axes,
                );
            }
        }
    }
    if inline.is_empty() {
        return;
    }

    for f in files {
        for s in f.body.iter() {
            let Structure::Block(b) = s else { continue };
            if b.ident.as_str() != "resource" || b.labels.len() != 2 {
                continue;
            }
            if b.labels[0].as_str() != "google_ads_campaign_criterion" {
                continue;
            }
            let mut negative = false;
            let mut campaign_ref: Option<(String, String)> = None;
            let mut loc_block: Option<&Block> = None;
            let mut lang_block: Option<&Block> = None;
            let mut device_block: Option<&Block> = None;
            for inner in b.body.iter() {
                match inner {
                    Structure::Attribute(a) => match a.key.as_str() {
                        "negative" => {
                            if let Expression::Bool(v) = &a.value {
                                negative = *v.as_ref();
                            }
                        }
                        "campaign" => campaign_ref = ref_type_name(&a.value),
                        _ => {}
                    },
                    Structure::Block(ib) => match ib.ident.as_str() {
                        "location" => loc_block = Some(ib),
                        "language" => lang_block = Some(ib),
                        "device" => device_block = Some(ib),
                        _ => {}
                    },
                }
            }
            if negative
                || (loc_block.is_none() && lang_block.is_none() && device_block.is_none())
            {
                continue;
            }
            let Some((ty, name)) = campaign_ref else { continue };
            if ty != "google_ads_campaign" {
                continue;
            }
            let target = match registry.resolve(&f.module, &ty, &name) {
                Resolution::Found(q) => q,
                _ => continue,
            };
            let Some(axes) = inline.get(&target) else { continue };
            if axes.locations {
                if let Some(ib) = loc_block {
                    diags.push(conflict_diag(f, ib, &name, "locations", "location"));
                }
            }
            if axes.languages {
                if let Some(ib) = lang_block {
                    diags.push(conflict_diag(f, ib, &name, "languages", "language"));
                }
            }
            if let Some(attr) = axes.devices {
                if let Some(ib) = device_block {
                    diags.push(conflict_diag(f, ib, &name, attr, "device"));
                }
            }
        }
    }
}

/// The Google Ads API refuses to *write* a `targeting_setting` on an ad group
/// whose campaign has one: "If the targeting_setting is set on the parent
/// Campaign, you must first remove the targeting_setting on the parent Campaign"
/// (developers.google.com/google-ads/api/docs/targeting/targeting-settings).
/// A warning, not an error: an account can carry both — Google fills them in —
/// so `export` has to be able to render what it read, and only an `apply` that
/// changes the ad group's side is refused. It takes the whole atomic batch with
/// it, which is why this is worth saying before the request goes out.
fn validate_targeting_setting_conflicts(
    files: &[ParsedFile],
    registry: &ResourceRegistry,
    defaults: &DefaultsRegistry,
    diags: &mut Vec<Diag>,
) {
    let campaign_default = defaults
        .provided_blocks("google_ads_campaign")
        .contains("targeting_setting");
    let mut campaigns: HashSet<String> = HashSet::new();
    for f in files {
        for b in resource_blocks(f, "google_ads_campaign") {
            if campaign_default || nested_block(&b.body, "targeting_setting").is_some() {
                campaigns.insert(ResourceRegistry::qualified(
                    &f.module,
                    "google_ads_campaign",
                    b.labels[1].as_str(),
                ));
            }
        }
    }
    if campaigns.is_empty() {
        return;
    }

    let ad_group_default = defaults
        .provided_blocks("google_ads_ad_group")
        .contains("targeting_setting");
    for f in files {
        for b in resource_blocks(f, "google_ads_ad_group") {
            // A defaults-provided block has no span in this file to point at,
            // so the ad group's own header carries the diagnostic.
            let at = match nested_block(&b.body, "targeting_setting") {
                Some(setting) => span_of(setting.ident.span()),
                None if ad_group_default => span_of(b.ident.span()),
                None => continue,
            };
            report_targeting_setting_conflict(f, b, at, registry, &campaigns, diags);
        }
    }
}

fn report_targeting_setting_conflict(
    file: &ParsedFile,
    ad_group: &Block,
    at: std::ops::Range<usize>,
    registry: &ResourceRegistry,
    campaigns: &HashSet<String>,
    diags: &mut Vec<Diag>,
) {
    let Some(value) = ad_group.body.iter().find_map(|s| match s {
        Structure::Attribute(a) if a.key.as_str() == "campaign" => Some(&a.value),
        _ => None,
    }) else {
        return;
    };
    let Some((ty, name)) = ref_type_name(value) else {
        return;
    };
    if ty != "google_ads_campaign" {
        return;
    }
    let Resolution::Found(target) = registry.resolve(&file.module, &ty, &name) else {
        return;
    };
    if !campaigns.contains(&target) {
        return;
    }
    diags.push(Diag::warning(
        file.src.clone(),
        at,
        format!(
            "campaign '{name}' also declares 'targeting_setting': Google Ads refuses to write one \
             on an ad group whose campaign has it, and a refused operation takes the whole apply \
             with it — declare the restrictions at one level, not both"
        ),
    ));
}

/// The `resource "<ty>" "<name>"` blocks in one file.
fn resource_blocks<'a>(file: &'a ParsedFile, ty: &'a str) -> impl Iterator<Item = &'a Block> {
    file.body.iter().filter_map(move |s| {
        let Structure::Block(b) = s else { return None };
        (b.ident.as_str() == "resource" && b.labels.len() == 2 && b.labels[0].as_str() == ty)
            .then_some(b)
    })
}

fn nested_block<'a>(body: &'a Body, name: &str) -> Option<&'a Block> {
    body.iter().find_map(|s| match s {
        Structure::Block(b) if b.ident.as_str() == name => Some(b),
        _ => None,
    })
}

fn conflict_diag(
    file: &ParsedFile,
    block: &Block,
    campaign_name: &str,
    attr: &str,
    axis: &str,
) -> Diag {
    Diag::new(
        file.src.clone(),
        span_of(block.ident.span()),
        format!(
            "campaign '{campaign_name}' already declares inline '{attr}'; drop this explicit positive {axis} criterion or the campaign's '{attr}' attribute (one source of truth per targeting type)"
        ),
    )
}

fn ref_type_name(expr: &Expression) -> Option<(String, String)> {
    let Expression::Traversal(t) = expr else {
        return None;
    };
    let path = extract_traversal_path(t)?;
    if path.len() < 2 {
        return None;
    }
    Some((path[0].clone(), path[1].clone()))
}

fn validate_top_level(
    file: &ParsedFile,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    defaults: &DefaultsRegistry,
    diags: &mut Vec<Diag>,
) {
    for s in file.body.iter() {
        match s {
            Structure::Attribute(a) => {
                diags.push(Diag::new(
                    file.src.clone(),
                    span_of(a.key.span()),
                    format!(
                        "top-level attributes are not allowed; place '{}' inside a 'provider', 'resource', 'defaults', 'locals', 'variable', 'module', or 'ad_template' block",
                        a.key.as_str()
                    ),
                ));
            }
            Structure::Block(b) => match b.ident.as_str() {
                "provider" => validate_provider(file, b, registry, locals, variables, diags),
                "resource" => {
                    validate_resource(file, b, registry, locals, variables, defaults, diags)
                }
                "defaults" => validate_defaults(file, b, registry, locals, variables, diags),
                "locals" => {
                    let _ = b;
                }
                "variable" => {
                    let _ = b;
                }
                "module" => {
                    let _ = b;
                }
                "ad_template" => {
                    let _ = b;
                }
                other => {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(b.ident.span()),
                        format!(
                            "unknown top-level block '{other}'; expected 'provider', 'resource', 'defaults', 'locals', 'variable', 'module', or 'ad_template'"
                        ),
                    ));
                }
            },
        }
    }
}

/// Validate a `defaults "<type>"` body against the type's schema. Required
/// attributes are not enforced at the top level (a defaults block legitimately
/// provides a subset); nested blocks it declares are checked in full.
/// Label-shape / unknown-type / duplicate errors are reported by
/// `DefaultsRegistry::build`.
fn validate_defaults(
    file: &ParsedFile,
    block: &Block,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    if block.labels.is_empty() || block.labels.len() > 2 {
        return;
    }
    let ty = block.labels[0].as_str();
    let Some(schema) = resource_schemas().get(ty) else {
        return;
    };
    let address = match block.labels.get(1) {
        Some(name) => format!("defaults.{ty}.{}", name.as_str()),
        None => format!("defaults.{ty}"),
    };
    validate_body(
        file,
        block,
        &block.body,
        schema,
        &address,
        registry,
        locals,
        variables,
        RequiredCheck::Skip,
        diags,
    );
    if ty == "google_ads_campaign" {
        validate_single_bidding_strategy(file, &block.body, &address, diags);
    }
}

fn validate_provider(
    file: &ParsedFile,
    block: &Block,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    if block.labels.len() != 1 {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "'provider' block requires exactly one label (the provider name), got {}",
                block.labels.len()
            ),
        ));
        return;
    }
    let provider_name = block.labels[0].as_str();
    let schema = match provider_schemas().get(provider_name) {
        Some(s) => s,
        None => {
            diags.push(Diag::new(
                file.src.clone(),
                span_of(block.labels[0].span()),
                format!("unknown provider '{provider_name}'"),
            ));
            return;
        }
    };
    validate_body(
        file,
        block,
        &block.body,
        schema,
        &format!("provider.{provider_name}"),
        registry,
        locals,
        variables,
        RequiredCheck::Enforce,
        diags,
    );
}

fn validate_resource(
    file: &ParsedFile,
    block: &Block,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    defaults: &DefaultsRegistry,
    diags: &mut Vec<Diag>,
) {
    if block.labels.len() != 2 {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "'resource' block requires exactly two labels (type and name), got {}",
                block.labels.len()
            ),
        ));
        return;
    }
    let ty = block.labels[0].as_str();
    let name = block.labels[1].as_str();
    let schema = match resource_schemas().get(ty) {
        Some(s) => s,
        None => {
            diags.push(Diag::new(
                file.src.clone(),
                span_of(block.labels[0].span()),
                format!("unknown resource type '{ty}'"),
            ));
            return;
        }
    };
    let address = format!("{ty}.{name}");
    let opted = validate_defaults_optin(file, block, ty, &address, defaults, diags);
    let provided = defaults.provided_attrs_named(ty, opted.as_deref());
    let without_lifecycle =
        validate_lifecycle(file, block, ty, &address, registry, locals, variables, diags);
    let body = strip_meta_attrs(without_lifecycle.unwrap_or_else(|| block.body.clone()));
    validate_body(
        file,
        block,
        &body,
        schema,
        &address,
        registry,
        locals,
        variables,
        RequiredCheck::SatisfiedBy(&provided),
        diags,
    );
    if ty == "google_ads_campaign" {
        validate_single_bidding_strategy(file, &block.body, &address, diags);
    }
    if ty == "google_ads_campaign_budget" {
        let merged = defaults.merge(ty, block);
        let body = merged.as_ref().map_or(&block.body, |b| &b.body);
        validate_budget_amount(file, block, body, &address, locals, variables, diags);
    }
    if ASSET_LINK_TYPES.contains(&ty) {
        validate_asset_field_type(file, block, &address, diags);
    }
}

/// The three resources that attach an asset to something.
const ASSET_LINK_TYPES: &[&str] = &[
    "google_ads_customer_asset",
    "google_ads_campaign_asset",
    "google_ads_ad_group_asset",
];

/// A declared `field_type` that contradicts the asset it points at. The pairing
/// is 1:1, so this is always a mistake — and one the API only reports after the
/// whole atomic batch has been rejected.
fn validate_asset_field_type(
    file: &ParsedFile,
    block: &Block,
    address: &str,
    diags: &mut Vec<Diag>,
) {
    let mut asset_type: Option<String> = None;
    let mut declared: Option<(String, std::ops::Range<usize>)> = None;
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "asset" => {
                asset_type = ref_type_name(&a.value).map(|(ty, _)| ty);
            }
            "field_type" => {
                if let Expression::String(v) = &a.value {
                    declared = Some((v.value().to_string(), span_of(a.value.span())));
                }
            }
            _ => {}
        }
    }
    let (Some(asset_type), Some((declared, span))) = (asset_type, declared) else {
        return;
    };
    let Some(expected) = field_type_for_asset(&asset_type) else {
        return;
    };
    if declared != expected {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!(
                "{address} attaches a {asset_type} as '{declared}', but that asset type is                  always '{expected}' — drop the attribute and bidsmith infers it"
            ),
        ));
    }
}

/// Meta-attributes the type schema never sees: they configure how the resource
/// is assembled rather than what is sent to Google Ads.
const META_ATTRS: &[&str] = &[DEFAULTS_ATTR, crate::expand::TEMPLATE_INPUTS_ATTR];

fn strip_meta_attrs(body: Body) -> Body {
    let mut out = Body::new();
    for s in body.iter() {
        if matches!(s, Structure::Attribute(a) if META_ATTRS.contains(&a.key.as_str())) {
            continue;
        }
        out.push(s.clone());
    }
    out
}

/// Check a resource's `defaults = defaults.<name>` opt-in and return the name
/// it resolves to. A malformed or unknown reference is an error here rather
/// than a silent fall-through to the unlabeled block — inheriting the wrong
/// shell is exactly the failure this attribute exists to prevent.
fn validate_defaults_optin(
    file: &ParsedFile,
    block: &Block,
    ty: &str,
    address: &str,
    defaults: &DefaultsRegistry,
    diags: &mut Vec<Diag>,
) -> Option<String> {
    let found: Vec<&hcl_edit::structure::Attribute> = block
        .body
        .iter()
        .filter_map(|s| match s {
            Structure::Attribute(a) if a.key.as_str() == DEFAULTS_ATTR => Some(a),
            _ => None,
        })
        .collect();
    let (first, extras) = found.split_first()?;
    for extra in extras {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(extra.key.span()),
            format!("duplicate 'defaults' attribute in {address}"),
        ));
    }

    let Some(name) = defaults_ref_name(&first.value) else {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(first.value.span()),
            format!(
                "expected a defaults reference like 'defaults.<name>', got {}",
                describe_expr(&first.value)
            ),
        ));
        return None;
    };
    if !defaults.has_named(ty, &name) {
        let known = defaults.names_for(ty);
        let tail = if known.is_empty() {
            format!("no named defaults are declared for {ty}")
        } else {
            format!("known for {ty}: {}", known.join(", "))
        };
        diags.push(Diag::new(
            file.src.clone(),
            span_of(first.value.span()),
            format!("unknown defaults '{name}' — {tail}"),
        ));
        return None;
    }
    Some(name)
}

/// Check the resource's `lifecycle` meta-block and return the rest of its body
/// for the type schema to validate. `None` means there was no `lifecycle` block
/// and the caller should use the body as-is.
#[allow(clippy::too_many_arguments)]
fn validate_lifecycle(
    file: &ParsedFile,
    block: &Block,
    ty: &str,
    address: &str,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) -> Option<Body> {
    let mut found: Vec<&Block> = Vec::new();
    let mut rest = Body::new();
    for s in block.body.iter() {
        match s {
            Structure::Block(b) if b.ident.as_str() == LIFECYCLE_BLOCK => found.push(b),
            other => rest.push(other.clone()),
        }
    }
    let first = found.first()?;

    if LIFECYCLE_UNSUPPORTED_TYPES.contains(&ty) {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(first.ident.span()),
            format!(
                "'lifecycle' is not supported on {ty} — the Google Ads API creates criteria \
                 freely, so there is nothing an adopt-only declaration would protect"
            ),
        ));
        return Some(rest);
    }
    for extra in found.iter().skip(1) {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(extra.ident.span()),
            format!("duplicate 'lifecycle' block in {address}"),
        ));
    }
    if !first.labels.is_empty() {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(first.labels[0].span()),
            "nested block 'lifecycle' does not take labels".to_string(),
        ));
    }
    validate_body(
        file,
        first,
        &first.body,
        lifecycle_schema(),
        &format!("{address}.{LIFECYCLE_BLOCK}"),
        registry,
        locals,
        variables,
        RequiredCheck::Enforce,
        diags,
    );
    Some(rest)
}

/// `amount_micros` and `total_amount_micros` are mutually exclusive, and which
/// one a budget needs is decided by `period`: a daily budget spends a rate, a
/// custom-period budget spends a lifetime cap and Google Ads ignores the rate.
/// The schema can mark both optional but not tie them to `period`.
fn validate_budget_amount(
    file: &ParsedFile,
    block: &Block,
    body: &Body,
    address: &str,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    let mut period: Option<String> = None;
    let mut has_amount = false;
    let mut has_total = false;
    for s in body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "amount_micros" => has_amount = true,
            "total_amount_micros" => has_total = true,
            "period" => {
                match resolve_for_lint(file, &a.value, locals, variables).as_ref() {
                    Expression::String(s) => period = Some(s.to_string()),
                    // An unresolvable expression could be either period, so the
                    // pairing below is unknowable — leave it to the API.
                    _ => return,
                }
            }
            _ => {}
        }
    }

    let custom = period.as_deref() == Some(CUSTOM_PERIOD);
    let message = match (custom, has_amount, has_total) {
        (true, _, false) => format!(
            "{address} sets period = \"{CUSTOM_PERIOD}\" but no 'total_amount_micros' — a \
             custom-period budget spends a lifetime total, not a daily amount"
        ),
        (true, true, true) => format!(
            "{address} sets both 'amount_micros' and 'total_amount_micros'; Google Ads ignores \
             the daily amount on a period = \"{CUSTOM_PERIOD}\" budget — keep \
             'total_amount_micros'"
        ),
        (false, _, true) => format!(
            "{address} sets 'total_amount_micros' on a daily budget — a lifetime total needs \
             period = \"{CUSTOM_PERIOD}\", otherwise say 'amount_micros'"
        ),
        (false, false, false) => {
            format!("missing required attribute 'amount_micros' in {address}")
        }
        _ => return,
    };
    diags.push(Diag::new(
        file.src.clone(),
        span_of(block.ident.span()),
        message,
    ));
}

/// The campaign's bidding blocks map to one protobuf `oneof`, which the block
/// schema can list but not constrain — declaring two is an API error at apply
/// time, so it is a validate error here.
fn validate_single_bidding_strategy(
    file: &ParsedFile,
    body: &Body,
    address: &str,
    diags: &mut Vec<Diag>,
) {
    let declared: Vec<&Block> = body
        .iter()
        .filter_map(|s| match s {
            Structure::Block(b) if CAMPAIGN_BIDDING_BLOCKS.contains(&b.ident.as_str()) => Some(b),
            _ => None,
        })
        .collect();
    let Some(extra) = declared.get(1) else { return };
    let names: Vec<&str> = declared.iter().map(|b| b.ident.as_str()).collect();
    diags.push(Diag::new(
        file.src.clone(),
        span_of(extra.ident.span()),
        format!(
            "{address} sets {}; a campaign has exactly one bidding strategy — keep one",
            quoted_list(&names)
        ),
    ));
}

enum RequiredCheck<'a> {
    Enforce,
    /// Enforce, except attributes a `defaults` block for this type provides.
    SatisfiedBy(&'a HashSet<String>),
    /// Do not enforce at this level (a defaults body provides a subset).
    Skip,
}

#[allow(clippy::too_many_arguments)]
fn validate_body(
    file: &ParsedFile,
    containing: &Block,
    body: &Body,
    schema: &BlockSchema,
    address: &str,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    required: RequiredCheck<'_>,
    diags: &mut Vec<Diag>,
) {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for s in body.iter() {
        match s {
            Structure::Attribute(a) => {
                let key = a.key.as_str();
                if !seen.insert(key) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(a.key.span()),
                        format!("duplicate attribute '{key}' in {address}"),
                    ));
                    continue;
                }
                let Some(attr_schema) = schema.attributes.iter().find(|x| x.name == key) else {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(a.key.span()),
                        format!("unknown attribute '{key}' in {address}"),
                    ));
                    continue;
                };
                validate_value(file, &a.value, &attr_schema.ty, registry, locals, variables, diags);
            }
            Structure::Block(b) => {
                let bname = b.ident.as_str();
                let Some(sub_schema) = schema.blocks.iter().find(|x| x.name == bname) else {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(b.ident.span()),
                        format!("unknown nested block '{bname}' in {address}"),
                    ));
                    continue;
                };
                if !b.labels.is_empty() {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(b.labels[0].span()),
                        format!("nested block '{bname}' does not take labels"),
                    ));
                }
                validate_body(
                    file,
                    b,
                    &b.body,
                    &sub_schema.schema,
                    &format!("{address}.{bname}"),
                    registry,
                    locals,
                    variables,
                    RequiredCheck::Enforce,
                    diags,
                );
                if matches!(bname, "keywords" | "negative_keywords") {
                    validate_compact_keywords(
                        file,
                        b,
                        &format!("{address}.{bname}"),
                        locals,
                        variables,
                        diags,
                    );
                }
                if matches!(bname, "audience" | "member") {
                    validate_exactly_one_of(
                        file,
                        b,
                        &format!("{address}.{bname}"),
                        &sub_schema.schema,
                        diags,
                    );
                }
            }
        }
    }

    for a in &schema.attributes {
        if !a.required || seen.contains(a.name) {
            continue;
        }
        match &required {
            RequiredCheck::Skip => continue,
            RequiredCheck::SatisfiedBy(provided) if provided.contains(a.name) => continue,
            _ => {}
        }
        diags.push(Diag::new(
            file.src.clone(),
            span_of(containing.ident.span()),
            format!("missing required attribute '{}' in {}", a.name, address),
        ));
    }
}

/// Follow a `local`/`var` chain to its value, discarding diags (`validate_value` reports reference errors); returns the input if not a resolvable binding.
fn resolve_for_lint<'a>(
    file: &ParsedFile,
    expr: &'a Expression,
    locals: &'a LocalsRegistry,
    variables: &'a VariablesRegistry,
) -> Cow<'a, Expression> {
    let mut sink = Vec::new();
    match resolve_binding_chain(file, expr, locals, variables, &mut sink) {
        BindingResolution::Resolved(value) => value,
        BindingResolution::Failed => Cow::Borrowed(expr),
    }
}

/// For a block whose attributes are mutually exclusive alternatives — the
/// schema can express "all optional" but not "pick exactly one".
fn validate_exactly_one_of(
    file: &ParsedFile,
    block: &Block,
    address: &str,
    schema: &BlockSchema,
    diags: &mut Vec<Diag>,
) {
    let set: Vec<&str> = block
        .body
        .iter()
        .filter_map(|s| match s {
            Structure::Attribute(a) => Some(a.key.as_str()),
            _ => None,
        })
        .filter(|k| schema.attributes.iter().any(|x| x.name == *k))
        .collect();
    if set.len() == 1 {
        return;
    }
    let choices: Vec<&str> = schema.attributes.iter().map(|a| a.name).collect();
    let message = if set.is_empty() {
        format!("{address} must set one of {}", quoted_list(&choices))
    } else {
        format!(
            "{address} sets {}; these are alternatives — set exactly one",
            quoted_list(&set)
        )
    };
    diags.push(Diag::new(
        file.src.clone(),
        span_of(block.ident.span()),
        message,
    ));
}

fn quoted_list(items: &[&str]) -> String {
    items
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(" / ")
}

fn validate_compact_keywords(
    file: &ParsedFile,
    block: &Block,
    address: &str,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    let mut has_match_type = false;
    let mut has_match_types = false;
    let mut empty_lists: Vec<(&str, std::ops::Range<usize>)> = Vec::new();
    for s in block.body.iter() {
        let Structure::Attribute(a) = s else { continue };
        match a.key.as_str() {
            "match_type" => has_match_type = true,
            "match_types" => {
                has_match_types = true;
                let resolved = resolve_for_lint(file, &a.value, locals, variables);
                if let Expression::Array(arr) = resolved.as_ref() {
                    if arr.is_empty() {
                        empty_lists.push(("match_types", span_of(a.value.span())));
                    }
                }
            }
            "texts" => {
                let resolved = resolve_for_lint(file, &a.value, locals, variables);
                if let Expression::Array(arr) = resolved.as_ref() {
                    if arr.is_empty() {
                        empty_lists.push(("texts", span_of(a.value.span())));
                    }
                }
            }
            _ => {}
        }
    }

    match (has_match_type, has_match_types) {
        (false, false) => diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "{address} must set either 'match_type' (a single match type) or 'match_types' (a list of match types)"
            ),
        )),
        (true, true) => diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "{address} sets both 'match_type' and 'match_types'; use one or the other"
            ),
        )),
        _ => {}
    }

    for (field, span) in empty_lists {
        let what = if field == "texts" {
            "at least one keyword"
        } else {
            "at least one match type"
        };
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!("{address} '{field}' must list {what}"),
        ));
    }
}

fn validate_value(
    file: &ParsedFile,
    expr: &Expression,
    ty: &FieldType,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    let span = span_of(expr.span());
    // A template parameter has no value here by construction: it is bound where
    // the template is used, and the bound body is type-checked there.
    if crate::expand::uses_template_input(expr) {
        return;
    }
    let evaluated = match resolve_binding_chain(file, expr, locals, variables, diags) {
        BindingResolution::Resolved(value) => value,
        BindingResolution::Failed => return,
    };
    let expr = evaluated.as_ref();
    match ty {
        FieldType::String => {
            if !matches!(expr, Expression::String(_)) {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    format!("expected string, got {}", describe_expr(expr)),
                ));
            }
        }
        FieldType::Date => match expr {
            Expression::String(s) => {
                if let Some(problem) = date_problem(s.value()) {
                    diags.push(Diag::new(file.src.clone(), span, problem));
                }
            }
            other => {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    format!(
                        "expected date string (YYYY-MM-DD), got {}",
                        describe_expr(other)
                    ),
                ));
            }
        },
        FieldType::Number => {
            if !matches!(expr, Expression::Number(_)) {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    format!("expected number, got {}", describe_expr(expr)),
                ));
            }
        }
        FieldType::Integer => match expr {
            Expression::Number(n) => {
                let formatted = &**n;
                if formatted.as_f64().map(|f| f.fract() != 0.0).unwrap_or(false) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span,
                        format!("expected integer, got fractional number {formatted}"),
                    ));
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!("expected integer, got {}", describe_expr(other)),
            )),
        },
        FieldType::Bool => {
            if !matches!(expr, Expression::Bool(_)) {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    format!("expected boolean, got {}", describe_expr(expr)),
                ));
            }
        }
        FieldType::Enum(values) => match expr {
            Expression::String(s) => {
                let v = s.as_str();
                if !values.contains(&v) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span,
                        format!(
                            "invalid value \"{v}\"; expected one of [{}]",
                            values.join(", ")
                        ),
                    ));
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!(
                    "expected one of [{}], got {}",
                    values.join(", "),
                    describe_expr(other)
                ),
            )),
        },
        FieldType::List(inner) => match expr {
            Expression::Array(arr) => {
                for item in arr.iter() {
                    validate_value(file, item, inner, registry, locals, variables, diags);
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!(
                    "expected list of {}, got {}",
                    describe_field_type(inner),
                    describe_expr(other)
                ),
            )),
        },
        FieldType::RsaAssetList => match expr {
            Expression::Array(arr) => {
                for item in arr.iter() {
                    let use_span = span_of(item.span());
                    let item = match resolve_binding_chain(file, item, locals, variables, diags) {
                        BindingResolution::Resolved(value) => value,
                        BindingResolution::Failed => continue,
                    };
                    validate_rsa_asset_item(file, item.as_ref(), use_span, locals, variables, diags);
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!(
                    "expected list of strings or {{ text, pin? }} objects, got {}",
                    describe_expr(other)
                ),
            )),
        },
        FieldType::LanguageList => {
            validate_code_list(file, expr, span, CodeKind::Language, locals, variables, diags)
        }
        FieldType::LocationList => {
            validate_code_list(file, expr, span, CodeKind::Location, locals, variables, diags)
        }
        FieldType::StringMap => match expr {
            Expression::Object(obj) => {
                for (key, value) in obj.iter() {
                    let value_span = span_of(value.expr().span());
                    if crate::expand::object_key_str(key).is_none() {
                        diags.push(Diag::new(
                            file.src.clone(),
                            value_span.clone(),
                            "map keys must be identifiers or strings".to_string(),
                        ));
                    }
                    validate_value(
                        file,
                        value.expr(),
                        &FieldType::String,
                        registry,
                        locals,
                        variables,
                        diags,
                    );
                }
            }
            other => diags.push(Diag::new(
                file.src.clone(),
                span,
                format!("expected map of name = string, got {}", describe_expr(other)),
            )),
        },
        FieldType::Ref(targets) => {
            validate_ref(file, expr, span, targets, registry, diags, false);
        }
        FieldType::RefOrResourceName(targets) => {
            if matches!(expr, Expression::String(_)) {
                return;
            }
            validate_ref(file, expr, span, targets, registry, diags, true);
        }
        // Structural only; existence + the ad/template XOR are checked in validate_ad_templates.
        FieldType::AdTemplateRef => {
            if ad_template_ref_name(expr).is_none() {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    format!(
                        "expected reference to ad_template (ad_template.<name>), got {}",
                        describe_expr(expr)
                    ),
                ));
            }
        }
    }
}

#[derive(Clone, Copy)]
enum CodeKind {
    Language,
    Location,
}

fn validate_code_list(
    file: &ParsedFile,
    expr: &Expression,
    span: std::ops::Range<usize>,
    kind: CodeKind,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    let (noun, example, constant) = match kind {
        CodeKind::Language => ("language code", "en", "languageConstants/NNNN"),
        CodeKind::Location => ("country code", "US", "geoTargetConstants/NNNN"),
    };
    let Expression::Array(arr) = expr else {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!("expected list of {noun}s, got {}", describe_expr(expr)),
        ));
        return;
    };
    for item in arr.iter() {
        let item_span = span_of(item.span());
        let item = match resolve_binding_chain(file, item, locals, variables, diags) {
            BindingResolution::Resolved(value) => value,
            BindingResolution::Failed => continue,
        };
        let Expression::String(s) = item.as_ref() else {
            diags.push(Diag::new(
                file.src.clone(),
                item_span,
                format!("expected {noun} string, got {}", describe_expr(item.as_ref())),
            ));
            continue;
        };
        let value = s.as_str();
        let resolved = match kind {
            CodeKind::Language => crate::targeting::resolve_language(value),
            CodeKind::Location => crate::targeting::resolve_location(value),
        };
        if resolved.is_none() {
            diags.push(Diag::new(
                file.src.clone(),
                item_span,
                format!(
                    "unknown {noun} \"{value}\"; use a known code (e.g. \"{example}\") or a raw {constant} string"
                ),
            ));
        }
    }
}

fn validate_ref(
    file: &ParsedFile,
    expr: &Expression,
    span: std::ops::Range<usize>,
    targets: &[&str],
    registry: &ResourceRegistry,
    diags: &mut Vec<Diag>,
    allow_resource_name: bool,
) {
    let expected = if allow_resource_name {
        format!("reference to {} or resource-name string", join_or(targets))
    } else {
        format!("reference to {}", join_or(targets))
    };
    let Expression::Traversal(t) = expr else {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!("expected {expected}, got {}", describe_expr(expr)),
        ));
        return;
    };
    let Some(path) = extract_traversal_path(t) else {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            "unsupported reference expression (only `<type>.<name>.<attribute>` is allowed)"
                .to_string(),
        ));
        return;
    };
    if path.len() < 2 {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!(
                "incomplete reference '{}'; expected '<type>.<name>.<attribute>'",
                path.join(".")
            ),
        ));
        return;
    }
    let ref_type = &path[0];
    let ref_name = &path[1];
    if !targets.iter().any(|&t| t == ref_type) {
        diags.push(Diag::new(
            file.src.clone(),
            span,
            format!(
                "expected {expected}, got reference to '{}'",
                ref_type
            ),
        ));
        return;
    }
    let address = format!("{ref_type}.{ref_name}");
    match registry.resolve(&file.module, ref_type, ref_name) {
        Resolution::Found(_) => {}
        Resolution::Missing => {
            diags.push(Diag::new(
                file.src.clone(),
                span,
                format!("reference to undeclared resource '{address}'"),
            ));
        }
        Resolution::Ambiguous(modules) => {
            let mut sorted: Vec<&str> = modules.iter().map(String::as_str).collect();
            sorted.sort();
            diags.push(Diag::new(
                file.src.clone(),
                span,
                format!(
                    "ambiguous reference to '{address}'; declared in modules [{}] — rename one of the resources so each is unique within its module",
                    sorted.join(", ")
                ),
            ));
        }
    }
}

enum BindingResolution<'a> {
    Resolved(Cow<'a, Expression>),
    Failed,
}

/// Evaluate an expression (binding chains + string templates); evaluation
/// errors become diags anchored at the use-site span.
fn resolve_binding_chain<'a>(
    file: &ParsedFile,
    expr: &'a Expression,
    locals: &'a LocalsRegistry,
    variables: &'a VariablesRegistry,
    diags: &mut Vec<Diag>,
) -> BindingResolution<'a> {
    let ctx = EvalCtx { locals, variables };
    match ctx.eval(&file.module, expr) {
        Ok(value) => BindingResolution::Resolved(value),
        Err(EvalError::Silent) => BindingResolution::Failed,
        Err(EvalError::Message(message)) => {
            diags.push(Diag::new(file.src.clone(), span_of(expr.span()), message));
            BindingResolution::Failed
        }
    }
}

pub(crate) fn extract_traversal_path(t: &Traversal) -> Option<Vec<String>> {
    let mut path = Vec::new();
    match &t.expr {
        Expression::Variable(v) => path.push(v.as_str().to_string()),
        _ => return None,
    }
    for op in t.operators.iter() {
        match &**op {
            TraversalOperator::GetAttr(name) => path.push(name.as_str().to_string()),
            // A `for_each` instance is addressed `co["howto"]`, one segment
            // whose name happens to contain brackets — fold the index back into
            // the segment it subscripts so it matches the generated label
            // verbatim (issue #145).
            TraversalOperator::Index(Expression::String(key)) => {
                let last = path.last_mut()?;
                last.push_str(&format!("[{:?}]", key.value().as_str()));
            }
            _ => return None,
        }
    }
    Some(path)
}

/// Google Ads dates are plain `YYYY-MM-DD` civil dates with no zone, so the
/// check is calendar arithmetic and nothing more. Returns the complaint to
/// show, or None when the string is a real date.
pub fn date_problem(s: &str) -> Option<String> {
    let bad = |why: &str| Some(format!("{s:?} is not a date (YYYY-MM-DD) — {why}"));
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3
        || parts[0].len() != 4
        || parts[1].len() != 2
        || parts[2].len() != 2
        || !s.bytes().all(|b| b.is_ascii_digit() || b == b'-')
    {
        return bad("expected four-digit year, two-digit month, two-digit day");
    }
    let (year, month, day) = (
        parts[0].parse::<u32>().ok()?,
        parts[1].parse::<u32>().ok()?,
        parts[2].parse::<u32>().ok()?,
    );
    if !(1..=12).contains(&month) {
        return bad("month must be 01-12");
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let last = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if leap => 29,
        _ => 28,
    };
    if day < 1 || day > last {
        return bad(&format!("{year:04}-{month:02} has {last} days"));
    }
    None
}

fn describe_field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::String => "string".to_string(),
        FieldType::Date => "date string (YYYY-MM-DD)".to_string(),
        FieldType::Integer => "integer".to_string(),
        FieldType::Number => "number".to_string(),
        FieldType::Bool => "boolean".to_string(),
        FieldType::Enum(values) => format!("one of [{}]", values.join(", ")),
        FieldType::Ref(targets) => format!("reference to {}", join_or(targets)),
        FieldType::RefOrResourceName(targets) => format!(
            "reference to {} or resource-name string",
            join_or(targets)
        ),
        FieldType::AdTemplateRef => "reference to ad_template".to_string(),
        FieldType::List(inner) => format!("list of {}", describe_field_type(inner)),
        FieldType::RsaAssetList => "list of strings or { text, pin? } objects".to_string(),
        FieldType::LanguageList => {
            "list of language codes (e.g. \"en\") or languageConstants/NNNN".to_string()
        }
        FieldType::LocationList => {
            "list of country codes (e.g. \"US\") or geoTargetConstants/NNNN".to_string()
        }
        FieldType::StringMap => "map of name = string".to_string(),
    }
}

fn validate_rsa_asset_item(
    file: &ParsedFile,
    expr: &Expression,
    use_span: std::ops::Range<usize>,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
    diags: &mut Vec<Diag>,
) {
    let span = expr.span().unwrap_or(use_span.clone());
    match expr {
        Expression::String(_) => {}
        Expression::Object(obj) => {
            let mut has_text = false;
            for (key, value) in obj.iter() {
                let Some(ident) = key.as_ident() else {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span.clone(),
                        "RSA asset object keys must be identifiers ('text' or 'pin')".to_string(),
                    ));
                    continue;
                };
                let value_span = value.expr().span().unwrap_or(use_span.clone());
                let resolved =
                    match resolve_binding_chain(file, value.expr(), locals, variables, diags) {
                        BindingResolution::Resolved(v) => v,
                        BindingResolution::Failed => continue,
                    };
                match ident.as_str() {
                    "text" => {
                        has_text = true;
                        if !matches!(resolved.as_ref(), Expression::String(_)) {
                            diags.push(Diag::new(
                                file.src.clone(),
                                value_span,
                                format!(
                                    "RSA asset 'text' must be a string, got {}",
                                    describe_expr(resolved.as_ref())
                                ),
                            ));
                        }
                    }
                    "pin" => match resolved.as_ref() {
                        Expression::String(s) => {
                            let v = s.as_str();
                            if !RSA_PIN.contains(&v) {
                                diags.push(Diag::new(
                                    file.src.clone(),
                                    value_span,
                                    format!(
                                        "invalid pin \"{v}\"; expected one of [{}]",
                                        RSA_PIN.join(", ")
                                    ),
                                ));
                            }
                        }
                        other => diags.push(Diag::new(
                            file.src.clone(),
                            value_span,
                            format!(
                                "RSA asset 'pin' must be a string, got {}",
                                describe_expr(other)
                            ),
                        )),
                    },
                    other => {
                        diags.push(Diag::new(
                            file.src.clone(),
                            span.clone(),
                            format!(
                                "unknown key '{other}' in RSA asset object; allowed: text, pin"
                            ),
                        ));
                    }
                }
            }
            if !has_text {
                diags.push(Diag::new(
                    file.src.clone(),
                    span,
                    "RSA asset object is missing required key 'text'".to_string(),
                ));
            }
        }
        other => diags.push(Diag::new(
            file.src.clone(),
            span,
            format!(
                "expected RSA asset (string or {{ text, pin? }} object), got {}",
                describe_expr(other)
            ),
        )),
    }
}

pub(crate) fn describe_expr(expr: &Expression) -> String {
    match expr {
        Expression::String(s) => format!("string \"{}\"", s.as_str()),
        Expression::Number(n) => format!("number {}", **n),
        Expression::Bool(b) => format!("boolean {}", **b),
        Expression::Traversal(t) => match extract_traversal_path(t) {
            Some(p) => format!("reference '{}'", p.join(".")),
            None => "expression".to_string(),
        },
        Expression::Variable(v) => format!("identifier '{}'", v.as_str()),
        Expression::Array(_) => "array".to_string(),
        Expression::Object(_) => "object".to_string(),
        Expression::Null(_) => "null".to_string(),
        Expression::StringTemplate(_) => "string template".to_string(),
        Expression::FuncCall(call) => {
            format!("function call '{}(…)'", call.name.name.as_str())
        }
        _ => "expression".to_string(),
    }
}

fn join_or(items: &[&str]) -> String {
    items.join(" or ")
}

fn is_empty_array_literal(expr: &Expression) -> bool {
    matches!(expr, Expression::Array(arr) if arr.is_empty())
}

fn body_has_attr(body: &Body, name: &str) -> bool {
    body.iter()
        .any(|s| matches!(s, Structure::Attribute(a) if a.key.as_str() == name))
}

fn block_span(b: &Block) -> std::ops::Range<usize> {
    let start = b.ident.span().map(|r| r.start).unwrap_or(0);
    let end = b
        .body
        .span()
        .map(|r| r.end)
        .or_else(|| b.labels.last().and_then(|l| l.span().map(|r| r.end)))
        .unwrap_or(start);
    start..end
}

fn span_of(s: Option<std::ops::Range<usize>>) -> std::ops::Range<usize> {
    s.unwrap_or(0..0)
}

#[derive(Serialize)]
pub struct SchemaDoc {
    pub version: &'static str,
    pub providers: BTreeMap<&'static str, BlockDoc>,
    pub resources: BTreeMap<&'static str, BlockDoc>,
}

#[derive(Serialize)]
pub struct BlockDoc {
    pub attributes: Vec<AttributeDoc>,
    pub blocks: Vec<NestedBlockDoc>,
}

#[derive(Serialize)]
pub struct AttributeDoc {
    pub name: &'static str,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "is_false")]
    pub always_emit: bool,
    #[serde(flatten)]
    pub ty: TypeDoc,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TypeDoc {
    String,
    Date,
    Integer,
    Number,
    Boolean,
    Enum { values: Vec<&'static str> },
    Reference { targets: Vec<&'static str> },
    ReferenceOrResourceName { targets: Vec<&'static str> },
    List { element: Box<TypeDoc> },
    RsaAssetList,
    LanguageList,
    LocationList,
    StringMap,
}

#[derive(Serialize)]
pub struct NestedBlockDoc {
    pub name: &'static str,
    pub attributes: Vec<AttributeDoc>,
    pub blocks: Vec<NestedBlockDoc>,
}

pub fn dump_schema() -> SchemaDoc {
    let providers = provider_schemas()
        .iter()
        .map(|(&k, v)| (k, block_to_doc(v)))
        .collect();
    let resources = resource_schemas()
        .iter()
        .map(|(&k, v)| (k, block_to_doc(v)))
        .collect();
    SchemaDoc {
        version: env!("CARGO_PKG_VERSION"),
        providers,
        resources,
    }
}

fn block_to_doc(b: &BlockSchema) -> BlockDoc {
    BlockDoc {
        attributes: b.attributes.iter().map(attr_to_doc).collect(),
        blocks: b.blocks.iter().map(nested_block_to_doc).collect(),
    }
}

fn nested_block_to_doc(n: &NestedBlockSchema) -> NestedBlockDoc {
    NestedBlockDoc {
        name: n.name,
        attributes: n.schema.attributes.iter().map(attr_to_doc).collect(),
        blocks: n.schema.blocks.iter().map(nested_block_to_doc).collect(),
    }
}

fn attr_to_doc(a: &AttributeSchema) -> AttributeDoc {
    AttributeDoc {
        name: a.name,
        required: a.required,
        default: a.default.as_ref().map(default_to_json),
        always_emit: a.always_emit,
        ty: ty_to_doc(&a.ty),
    }
}

fn default_to_json(d: &DefaultValue) -> serde_json::Value {
    match d {
        DefaultValue::Str(s) => serde_json::Value::String((*s).to_string()),
        DefaultValue::Bool(b) => serde_json::Value::Bool(*b),
    }
}

fn ty_to_doc(ty: &FieldType) -> TypeDoc {
    match ty {
        FieldType::String => TypeDoc::String,
        FieldType::Date => TypeDoc::Date,
        FieldType::Integer => TypeDoc::Integer,
        FieldType::Number => TypeDoc::Number,
        FieldType::Bool => TypeDoc::Boolean,
        FieldType::Enum(values) => TypeDoc::Enum {
            values: values.to_vec(),
        },
        FieldType::Ref(targets) => TypeDoc::Reference {
            targets: targets.to_vec(),
        },
        FieldType::RefOrResourceName(targets) => TypeDoc::ReferenceOrResourceName {
            targets: targets.to_vec(),
        },
        FieldType::AdTemplateRef => TypeDoc::Reference {
            targets: vec!["ad_template"],
        },
        FieldType::List(inner) => TypeDoc::List {
            element: Box::new(ty_to_doc(inner)),
        },
        FieldType::RsaAssetList => TypeDoc::RsaAssetList,
        FieldType::LanguageList => TypeDoc::LanguageList,
        FieldType::LocationList => TypeDoc::LocationList,
        FieldType::StringMap => TypeDoc::StringMap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_file;
    use std::io::Write;

    fn parse_str(name: &str, content: &str) -> ParsedFile {
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("bidsmith-locals-test-{name}.bid"));
        {
            let mut f = std::fs::File::create(&tmp).expect("create tmp");
            f.write_all(content.as_bytes()).expect("write tmp");
        }
        parse_file(&tmp).expect("parse")
    }

    fn bindings_from(pf: &ParsedFile) -> Bindings {
        let (b, diags) = Bindings::build(std::slice::from_ref(pf), &InputBindings::default());
        assert!(diags.is_empty(), "build diags: {:?}", diags.len());
        b
    }

    /// A block with no mask paths would go out as a bare `oneof` member and be
    /// rejected at apply-time, which only a live account would notice.
    #[test]
    fn every_bidding_block_knows_how_to_mask_itself() {
        for block in CAMPAIGN_BIDDING_BLOCKS {
            let paths = campaign_bidding_mask_paths(block)
                .unwrap_or_else(|| panic!("{block} has no update-mask paths"));
            assert!(!paths.is_empty(), "{block} has an empty mask");
            for p in paths {
                assert!(
                    *p == *block || p.starts_with(&format!("{block}.")),
                    "{block} masks an unrelated field: {p}"
                );
            }
        }
        assert_eq!(CAMPAIGN_BIDDING_BLOCKS.len(), CAMPAIGN_BIDDING_MASK_PATHS.len());
    }

    #[test]
    fn locals_resolve_literal_int() {
        let pf = parse_str(
            "literal_int",
            r#"
locals { x = 42 }
"#,
        );
        let bindings = bindings_from(&pf);
        let expr: Expression = "local.x".parse().expect("parse traversal");
        let resolved = bindings.resolve_value("module", &expr);
        match resolved.as_ref() {
            Expression::Number(n) => assert_eq!(n.as_f64(), Some(42.0)),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn locals_resolve_chain() {
        let pf = parse_str(
            "chain",
            r#"
locals {
  base = 5
  via  = local.base
  top  = local.via
}
"#,
        );
        let bindings = bindings_from(&pf);
        let expr: Expression = "local.top".parse().expect("parse");
        match bindings.resolve_value("chain", &expr).as_ref() {
            Expression::Number(n) => assert_eq!(n.as_f64(), Some(5.0)),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn locals_cycle_returns_traversal_silently() {
        let pf = parse_str(
            "cycle",
            r#"
locals {
  a = local.b
  b = local.a
}
"#,
        );
        let (bindings, _diags) =
            Bindings::build(std::slice::from_ref(&pf), &InputBindings::default());
        let expr: Expression = "local.a".parse().expect("parse");
        let resolved = bindings.resolve_value("cycle", &expr);
        assert!(matches!(resolved.as_ref(), Expression::Traversal(_)));
    }

    #[test]
    fn variables_resolve_default_number() {
        let pf = parse_str(
            "var_default",
            r#"
variable "city_radius_km" {
  type    = number
  default = 15
}
"#,
        );
        let bindings = bindings_from(&pf);
        let expr: Expression = "var.city_radius_km".parse().expect("parse");
        match bindings.resolve_value("var_default", &expr).as_ref() {
            Expression::Number(n) => assert_eq!(n.as_f64(), Some(15.0)),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    #[test]
    fn variables_input_overrides_default() {
        let pf = parse_str(
            "var_input",
            r#"
variable "wave" {
  type    = string
  default = "W1"
}
"#,
        );
        let mut inputs = InputBindings::default();
        inputs.vars.insert("wave".to_string(), "W2".to_string());
        let (bindings, diags) = Bindings::build(std::slice::from_ref(&pf), &inputs);
        assert!(diags.is_empty());
        let expr: Expression = "var.wave".parse().expect("parse");
        match bindings.resolve_value("var_input", &expr).as_ref() {
            Expression::String(s) => assert_eq!(s.as_str(), "W2"),
            other => panic!("expected String, got {other:?}"),
        }
    }

    #[test]
    fn variables_missing_value_errors() {
        let pf = parse_str(
            "var_missing",
            r#"
variable "wave" {
  type = string
}
"#,
        );
        let (_bindings, diags) =
            Bindings::build(std::slice::from_ref(&pf), &InputBindings::default());
        assert!(
            diags.iter().any(|d| d.message.contains("variable 'wave' has no value")),
            "expected missing-value diag, got {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn variables_type_mismatch_in_default_errors() {
        let pf = parse_str(
            "var_type_mismatch",
            r#"
variable "wave" {
  type    = number
  default = "W1"
}
"#,
        );
        let (_bindings, diags) =
            Bindings::build(std::slice::from_ref(&pf), &InputBindings::default());
        assert!(
            diags.iter().any(|d| d.message.contains("variable default expects number")),
            "expected default mismatch diag, got {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn locals_can_reference_variables() {
        let pf = parse_str(
            "loc_via_var",
            r#"
variable "budget_micros" {
  type    = number
  default = 10000000
}

locals {
  daily = var.budget_micros
}
"#,
        );
        let bindings = bindings_from(&pf);
        let expr: Expression = "local.daily".parse().expect("parse");
        match bindings.resolve_value("loc_via_var", &expr).as_ref() {
            Expression::Number(n) => assert_eq!(n.as_f64(), Some(10000000.0)),
            other => panic!("expected Number, got {other:?}"),
        }
    }

    fn validate_str(name: &str, content: &str) -> Vec<Diag> {
        let pf = parse_str(name, content);
        validate_files(std::slice::from_ref(&pf), &InputBindings::default())
    }

    #[test]
    fn compact_keywords_requires_a_match_type() {
        let diags = validate_str(
            "kw_neither",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  keywords {
    texts = ["a", "b"]
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("must set either 'match_type'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn compact_keywords_rejects_both_match_type_forms() {
        let diags = validate_str(
            "kw_both",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  keywords {
    match_type  = "EXACT"
    match_types = ["PHRASE"]
    texts       = ["a"]
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("use one or the other")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_custom_period_budget_declares_a_lifetime_total() {
        let diags = validate_str(
            "budget_custom_period",
            r#"
resource "google_ads_campaign_budget" "flight" {
  name                = "Q3 Flight"
  total_amount_micros = 91000000
  period              = "CUSTOM_PERIOD"
}
"#,
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_budget_with_no_amount_at_all_is_an_error() {
        let diags = validate_str(
            "budget_no_amount",
            r#"
resource "google_ads_campaign_budget" "b" {
  name = "B"
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("missing required attribute 'amount_micros'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_lifetime_total_without_a_custom_period_is_an_error() {
        let diags = validate_str(
            "budget_total_daily",
            r#"
resource "google_ads_campaign_budget" "b" {
  name                = "B"
  total_amount_micros = 91000000
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("needs period = \"CUSTOM_PERIOD\"")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_custom_period_budget_without_a_total_is_an_error() {
        let diags = validate_str(
            "budget_custom_no_total",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
  period        = "CUSTOM_PERIOD"
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("but no 'total_amount_micros'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_budget_setting_both_amounts_is_an_error() {
        let diags = validate_str(
            "budget_both_amounts",
            r#"
resource "google_ads_campaign_budget" "b" {
  name                = "B"
  amount_micros       = 1000000
  total_amount_micros = 91000000
  period              = "CUSTOM_PERIOD"
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("sets both 'amount_micros'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_budget_amount_a_defaults_block_provides_is_not_missing() {
        let diags = validate_str(
            "budget_from_defaults",
            r#"
defaults "google_ads_campaign_budget" {
  amount_micros = 1000000
}

resource "google_ads_campaign_budget" "b" {
  name = "B"
}
"#,
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn repeated_frequency_caps_validate() {
        let diags = validate_str(
            "freq_caps_ok",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "v" {
  name                     = "V"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id

  frequency_caps {
    event_type  = "IMPRESSION"
    time_unit   = "DAY"
    time_length = 1
    cap         = 3
  }

  frequency_caps {
    event_type  = "VIDEO_VIEW"
    time_unit   = "DAY"
    time_length = 1
    cap         = 1
  }
}
"#,
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn frequency_caps_reject_a_bad_event_type_and_a_missing_cap() {
        let diags = validate_str(
            "freq_caps_bad",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "v" {
  name                     = "V"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id

  frequency_caps {
    event_type  = "CLICK"
    time_unit   = "DAY"
    time_length = 1
  }
}
"#,
        );
        let msgs: Vec<&String> = diags.iter().map(|d| &d.message).collect();
        assert!(msgs.iter().any(|m| m.contains("CLICK")), "{msgs:?}");
        assert!(
            msgs.iter().any(|m| m.contains("missing required attribute 'cap'")),
            "{msgs:?}"
        );
    }

    fn targeting_setting_project(campaign_block: &str, ad_group_block: &str) -> String {
        format!(
            r#"
resource "google_ads_campaign_budget" "b" {{
  name          = "B"
  amount_micros = 1000000
}}

resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
{campaign_block}
}}

resource "google_ads_ad_group" "g" {{
  name     = "G"
  campaign = google_ads_campaign.c.id
{ad_group_block}
}}
"#
        )
    }

    const OBSERVE_AUDIENCE: &str = r#"
  targeting_setting {
    target_restriction {
      targeting_dimension = "AUDIENCE"
      bid_only            = true
    }
  }
"#;

    #[test]
    fn repeated_target_restrictions_validate_on_either_level() {
        let diags = validate_str(
            "targeting_setting_ok",
            &targeting_setting_project(
                "",
                r#"
  targeting_setting {
    target_restriction {
      targeting_dimension = "AGE_RANGE"
      bid_only            = true
    }

    target_restriction {
      targeting_dimension = "GENDER"
      bid_only            = false
    }
  }
"#,
            ),
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    /// Keywords always restrict, so the API has no reading of a KEYWORD
    /// restriction — and `bid_only` is the whole point of the block, never a
    /// thing to leave to a default.
    #[test]
    fn a_target_restriction_rejects_keyword_and_a_missing_bid_only() {
        let diags = validate_str(
            "targeting_setting_bad",
            &targeting_setting_project(
                r#"
  targeting_setting {
    target_restriction {
      targeting_dimension = "KEYWORD"
    }
  }
"#,
                "",
            ),
        );
        let msgs: Vec<&String> = diags.iter().map(|d| &d.message).collect();
        assert!(msgs.iter().any(|m| m.contains("KEYWORD")), "{msgs:?}");
        assert!(
            msgs.iter().any(|m| m.contains("missing required attribute 'bid_only'")),
            "{msgs:?}"
        );
    }

    /// Google Ads refuses to write an ad group's targeting setting while its
    /// campaign has one, and one refusal sinks the whole atomic batch. A
    /// warning, not an error: an account can carry both, so `export` has to be
    /// able to render what it read.
    #[test]
    fn declaring_a_targeting_setting_at_both_levels_warns() {
        let diags = validate_str(
            "targeting_setting_both",
            &targeting_setting_project(OBSERVE_AUDIENCE, OBSERVE_AUDIENCE),
        );
        assert_eq!(diags.len(), 1, "{:?}", diags.iter().map(|d| &d.message).collect::<Vec<_>>());
        assert!(!diags[0].is_error(), "{}", diags[0].message);
        assert!(
            diags[0].message.contains("campaign 'c' also declares 'targeting_setting'"),
            "{}",
            diags[0].message
        );
    }

    #[test]
    fn one_level_at_a_time_is_quiet() {
        for (campaign, ad_group) in [(OBSERVE_AUDIENCE, ""), ("", OBSERVE_AUDIENCE)] {
            let diags = validate_str(
                "targeting_setting_one_level",
                &targeting_setting_project(campaign, ad_group),
            );
            assert!(
                diags.is_empty(),
                "{:?}",
                diags.iter().map(|d| &d.message).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn audience_and_member_blocks_require_exactly_one_target() {
        let diags = validate_str(
            "one_of",
            r#"
resource "google_ads_custom_audience" "a" {
  name = "A"

  member {
    keyword = "x"
    url     = "https://example.com"
  }

  member {}
}

resource "google_ads_campaign_criterion" "c" {
  campaign = google_ads_campaign_criterion.c.id

  audience {}
}
"#,
        );
        let msgs: Vec<&String> = diags.iter().map(|d| &d.message).collect();
        assert!(
            msgs.iter()
                .any(|m| m.contains("sets 'keyword' / 'url'") && m.contains("set exactly one")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("must set one of 'keyword'")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("must set one of 'custom_audience'")),
            "{msgs:?}"
        );
    }

    #[test]
    fn video_targeting_criterion_blocks_validate() {
        let diags = validate_str(
            "video_targeting",
            r#"
resource "google_ads_custom_audience" "seg" {
  name = "Ad blocker searchers"
  type = "SEARCH"

  member { keyword = "ad blocker" }
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "v" {
  name                     = "V"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id
}

resource "google_ads_campaign_criterion" "intent" {
  campaign = google_ads_campaign.v.id

  audience {
    custom_audience = google_ads_custom_audience.seg.id
  }
}

resource "google_ads_campaign_criterion" "channel" {
  campaign = google_ads_campaign.v.id

  youtube_channel { channel_id = "UCabc" }
}

resource "google_ads_campaign_criterion" "no_kids" {
  campaign = google_ads_campaign.v.id
  negative = true

  age_range { type = "AGE_RANGE_18_24" }
}
"#,
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ad_group_targeting_criterion_blocks_validate() {
        let diags = validate_str(
            "ag_targeting",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "v" {
  name                     = "V"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id
}

resource "google_ads_ad_group" "cohort" {
  name     = "Cohort 35+"
  campaign = google_ads_campaign.v.id
  type     = "VIDEO_TRUE_VIEW_IN_STREAM"
}

resource "google_ads_ad_group_criterion" "cohort_list" {
  ad_group     = google_ads_ad_group.cohort.id
  bid_modifier = 1.2

  audience { user_list = "customers/1/userLists/987" }
}

resource "google_ads_ad_group_criterion" "cohort_age" {
  ad_group = google_ads_ad_group.cohort.id

  age_range { type = "AGE_RANGE_35_44" }
}

resource "google_ads_ad_group_criterion" "cohort_income" {
  ad_group = google_ads_ad_group.cohort.id

  income_range { type = "INCOME_RANGE_90_UP" }
}

resource "google_ads_ad_group_criterion" "cohort_parents" {
  ad_group = google_ads_ad_group.cohort.id
  negative = true

  parental_status { type = "PARENT" }
}

resource "google_ads_ad_group_criterion" "cohort_placement" {
  ad_group = google_ads_ad_group.cohort.id

  placement { url = "https://example.com/reviews" }
}

resource "google_ads_ad_group_criterion" "cohort_market" {
  ad_group = google_ads_ad_group.cohort.id

  location { geo_target_constant = "geoTargetConstants/2702" }
}
"#,
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn compact_keywords_rejects_empty_texts() {
        let diags = validate_str(
            "kw_empty",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  keywords {
    match_type = "EXACT"
    texts      = []
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("'texts' must list at least one keyword")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn compact_keywords_reports_both_empty_lists() {
        let diags = validate_str(
            "kw_both_empty",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  keywords {
    texts       = []
    match_types = []
  }
}
"#,
        );
        let msgs: Vec<&String> = diags.iter().map(|d| &d.message).collect();
        assert!(
            msgs.iter().any(|m| m.contains("'texts' must list at least one keyword")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("'match_types' must list at least one match type")),
            "{msgs:?}"
        );
    }

    #[test]
    fn compact_keywords_valid_form_has_no_errors() {
        let diags = validate_str(
            "kw_ok",
            r#"
resource "google_ads_ad_group" "ag" {
  name     = "AG"
  campaign = google_ads_campaign.c.id
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id
  status   = "ENABLED"

  keywords {
    match_types = ["EXACT", "PHRASE"]
    texts       = ["a", "b"]
  }
}
"#,
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    const TARGETING_PREAMBLE: &str = r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}
"#;

    #[test]
    fn video_bidding_selector_blocks_validate() {
        let diags = validate_str(
            "video_bidding_ok",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id

  target_cpm {{}}
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn video_campaign_settings_and_sub_type_validate() {
        let diags = validate_str(
            "video_inventory_ok",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                         = "C"
  advertising_channel_type     = "VIDEO"
  advertising_channel_sub_type = "VIDEO_NON_SKIPPABLE"
  campaign_budget              = google_ads_campaign_budget.b.id

  target_cpm {{}}

  video_campaign_settings {{
    video_ad_inventory_control {{
      allow_in_stream               = true
      allow_in_feed                 = false
      allow_shorts                  = false
      allow_non_skippable_in_stream = false
    }}
  }}
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn an_unknown_channel_sub_type_errors() {
        let diags = validate_str(
            "video_sub_type_bad",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                         = "C"
  advertising_channel_type     = "VIDEO"
  advertising_channel_sub_type = "VIDEO_OUTSTREAM"
  campaign_budget              = google_ads_campaign_budget.b.id

  target_cpm {{}}
}}
"#
            ),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.is_error() && d.message.contains("VIDEO_OUTSTREAM")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn search_bidding_blocks_validate_with_their_subfields() {
        let diags = validate_str(
            "search_bidding_ok",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "generic" {{
  name                     = "Search_Generic"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id

  target_impression_share {{
    location                 = "ANYWHERE_ON_PAGE"
    location_fraction_micros = 800000
    cpc_bid_ceiling_micros   = 500000
  }}
}}

resource "google_ads_campaign" "ublock" {{
  name                     = "Search_uBlock"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id

  target_spend {{}}
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn an_impression_share_block_missing_its_ceiling_errors() {
        let diags = validate_str(
            "tis_missing_ceiling",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id

  target_impression_share {{
    location                 = "TOP_OF_PAGE"
    location_fraction_micros = 650000
  }}
}}
"#
            ),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("cpc_bid_ceiling_micros")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_impression_share_location_outside_the_enum_errors() {
        let diags = validate_str(
            "tis_bad_location",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id

  target_impression_share {{
    location                 = "SIDEBAR"
    location_fraction_micros = 650000
    cpc_bid_ceiling_micros   = 500000
  }}
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("SIDEBAR")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_bidding_blocks_on_one_campaign_error() {
        let diags = validate_str(
            "video_bidding_conflict",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id

  manual_cpc {{}}

  target_cpv {{}}
}}
"#
            ),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("exactly one bidding strategy")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    fn merged_attr(name: &str, src: &str, resource_index: usize) -> Vec<(String, String)> {
        // A distinct name per call: parse_str writes a temp file keyed by it,
        // and the test binary runs these concurrently.
        let pf = parse_str(name, src);
        let (defaults, diags) = DefaultsRegistry::build(std::slice::from_ref(&pf));
        assert!(diags.is_empty(), "{:?}", diags.iter().map(|d| &d.message).collect::<Vec<_>>());
        let resource = pf
            .body
            .iter()
            .filter_map(|s| match s {
                Structure::Block(b) if b.ident.as_str() == "resource" => Some(b),
                _ => None,
            })
            .nth(resource_index)
            .expect("resource block");
        let merged = defaults
            .merge("google_ads_campaign", resource)
            .unwrap_or_else(|| resource.clone());
        merged
            .body
            .iter()
            .filter_map(|s| match s {
                Structure::Attribute(a) => {
                    Some((a.key.as_str().to_string(), a.value.to_string().trim().to_string()))
                }
                _ => None,
            })
            .collect()
    }

    const MIXED_DEFAULTS: &str = r#"
defaults "google_ads_campaign" {
  status = "PAUSED"
}

defaults "google_ads_campaign" "search_us" {
  advertising_channel_type = "SEARCH"
  locations                = ["US"]
}

resource "google_ads_campaign" "search" {
  defaults        = defaults.search_us
  name            = "S"
  campaign_budget = google_ads_campaign_budget.b.id
}

resource "google_ads_campaign" "other" {
  name                     = "O"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id
}
"#;

    #[test]
    fn an_opted_in_resource_takes_the_named_block_instead_of_the_unnamed_one() {
        // Named defaults replace rather than layer: one visible source per
        // resource, so the opt-in tells the whole story (issue #145).
        let attrs = merged_attr("named_merge_optin", MIXED_DEFAULTS, 0);
        let keys: Vec<&str> = attrs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"advertising_channel_type"), "{attrs:?}");
        assert!(keys.contains(&"locations"), "{attrs:?}");
        assert!(
            !keys.contains(&"status"),
            "the unnamed block must not layer under the named one: {attrs:?}",
        );
    }

    #[test]
    fn a_resource_that_does_not_opt_in_still_gets_the_unnamed_block() {
        let attrs = merged_attr("named_merge_no_optin", MIXED_DEFAULTS, 1);
        let keys: Vec<&str> = attrs.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"status"), "{attrs:?}");
        assert!(!keys.contains(&"locations"), "{attrs:?}");
    }

    #[test]
    fn a_resources_own_value_still_beats_the_named_block() {
        let attrs = merged_attr(
            "named_merge_own_value",
            r#"
defaults "google_ads_campaign" "search_us" {
  advertising_channel_type = "SEARCH"
  locations                = ["US"]
}

resource "google_ads_campaign" "search" {
  defaults                 = defaults.search_us
  name                     = "S"
  advertising_channel_type = "DISPLAY"
  campaign_budget          = google_ads_campaign_budget.b.id
}
"#,
            0,
        );
        let channel = attrs
            .iter()
            .find(|(k, _)| k == "advertising_channel_type")
            .map(|(_, v)| v.as_str());
        assert_eq!(channel, Some("\"DISPLAY\""), "{attrs:?}");
    }

    #[test]
    fn a_campaigns_own_bidding_block_replaces_the_defaults_one() {
        let pf = parse_str(
            "bidding_defaults",
            r#"
defaults "google_ads_campaign" {
  manual_cpc {
    enhanced_cpc_enabled = false
  }

  network_settings {
    target_google_search = true
  }
}

resource "google_ads_campaign" "video" {
  name                     = "V"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id

  target_cpv {}
}
"#,
        );
        let (defaults, _) = DefaultsRegistry::build(std::slice::from_ref(&pf));
        let campaign = pf
            .body
            .iter()
            .filter_map(|s| match s {
                Structure::Block(b) if b.ident.as_str() == "resource" => Some(b),
                _ => None,
            })
            .next()
            .expect("campaign block");
        let merged = defaults
            .merge("google_ads_campaign", campaign)
            .expect("network_settings still merges in");
        let blocks: Vec<&str> = merged
            .body
            .iter()
            .filter_map(|s| match s {
                Structure::Block(b) => Some(b.ident.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(blocks, vec!["target_cpv", "network_settings"], "{blocks:?}");
    }

    #[test]
    fn inline_targeting_validates_clean() {
        let diags = validate_str(
            "inline_ok",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  languages                = ["en", "pl"]
  locations                = ["US", "geoTargetConstants/2702"]
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn inline_targeting_rejects_unknown_code() {
        let diags = validate_str(
            "inline_bad_code",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  locations                = ["US", "Atlantis"]
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("unknown country code \"Atlantis\"")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_and_explicit_same_axis_conflict() {
        let diags = validate_str(
            "inline_conflict",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  locations                = ["US"]
}}

resource "google_ads_campaign_criterion" "extra_geo" {{
  campaign = google_ads_campaign.c.id
  location {{
    geo_target_constant = "geoTargetConstants/2826"
  }}
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("already declares inline 'locations'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_locations_allows_explicit_negative_location() {
        let diags = validate_str(
            "inline_neg_ok",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  locations                = ["US"]
}}

resource "google_ads_campaign_criterion" "exclude_ak" {{
  campaign = google_ads_campaign.c.id
  negative = true
  location {{
    geo_target_constant = "geoTargetConstants/21132"
  }}
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn inline_devices_conflict_with_an_explicit_device_criterion() {
        let diags = validate_str(
            "inline_dev_conflict",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  devices                  = ["DESKTOP"]
}}

resource "google_ads_campaign_criterion" "c_mobile" {{
  campaign     = google_ads_campaign.c.id
  bid_modifier = 0
  device {{
    type = "MOBILE"
  }}
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("already declares inline 'devices'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn excluded_devices_names_itself_in_the_conflict() {
        let diags = validate_str(
            "excl_dev_conflict",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  excluded_devices         = ["MOBILE"]
}}

resource "google_ads_campaign_criterion" "c_desktop" {{
  campaign = google_ads_campaign.c.id
  device {{
    type = "DESKTOP"
  }}
}}
"#
            ),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("already declares inline 'excluded_devices'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn devices_and_excluded_devices_on_one_campaign_is_an_error() {
        // `devices` is closed — it already excludes what it omits — so a second
        // list can only agree redundantly or contradict.
        let diags = validate_str(
            "both_device_attrs",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  devices                  = ["DESKTOP"]
  excluded_devices         = ["MOBILE"]
}}
"#
            ),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("declares both 'devices' and 'excluded_devices'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_devices_reject_a_value_outside_the_device_enum() {
        let diags = validate_str(
            "bad_device",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  devices                  = ["LAPTOP"]
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("invalid value \"LAPTOP\"")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn inline_devices_alone_validate_clean() {
        let diags = validate_str(
            "inline_dev_ok",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  devices                  = ["DESKTOP"]
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn locals_build_rejects_duplicate_across_blocks_and_nested_block() {
        let pf = parse_str(
            "dup",
            r#"
locals {
  a = 1
  inner { x = 1 }
}

locals {
  a = 2
}
"#,
        );
        let (_locals, diags) = LocalsRegistry::build(std::slice::from_ref(&pf));
        let msgs: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert!(msgs.iter().any(|m| m.contains("duplicate local 'a'")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("nested block 'inner'")), "{msgs:?}");
    }

    const LIST_LOCAL_PREAMBLE: &str = r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  manual_cpc { enhanced_cpc_enabled = false }
}

resource "google_ads_ad_group" "g" {
  name           = "G"
  campaign       = google_ads_campaign.c.id
  cpc_bid_micros = 1000000
}
"#;

    #[test]
    fn list_local_in_rsa_and_compact_keywords_validates() {
        let mut content = String::from(
            r#"
locals {
  headlines    = ["One Headline", "Two Headline", "Three Headline"]
  descriptions = ["A description here", "Another description here"]
  urls         = ["https://example.com"]
  themes       = ["alpha", "beta", "gamma"]
}

resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  status   = "ENABLED"

  ad {
    final_urls = local.urls

    responsive_search_ad {
      headlines    = local.headlines
      descriptions = local.descriptions
    }
  }
}

resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.g.id
  status   = "ENABLED"

  keywords {
    texts      = local.themes
    match_type = "PHRASE"
  }
}
"#,
        );
        content.push_str(LIST_LOCAL_PREAMBLE);
        let diags = validate_str("list_local_ok", &content);
        let errors: Vec<&String> = diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| &d.message)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn per_element_local_in_rsa_list_validates() {
        let mut content = String::from(
            r#"
locals {
  promo = "Promo Headline Here"
}

resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  status   = "ENABLED"

  ad {
    final_urls = ["https://example.com"]

    responsive_search_ad {
      headlines    = ["First Headline", local.promo, "Third Headline"]
      descriptions = ["A description here", "Another description here"]
    }
  }
}
"#,
        );
        content.push_str(LIST_LOCAL_PREAMBLE);
        let diags = validate_str("per_element_local", &content);
        let errors: Vec<&String> = diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| &d.message)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn scalar_local_into_list_attribute_errors() {
        let diags = validate_str(
            "scalar_into_list",
            r#"
locals {
  not_a_list = "just a string"
}

resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id

  ad {
    final_urls = ["https://example.com"]
    responsive_search_ad {
      headlines    = local.not_a_list
      descriptions = ["A description here", "Another description here"]
    }
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.is_error()
                && d.message.contains("expected list of strings or")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn undeclared_list_local_reference_errors() {
        let diags = validate_str(
            "undeclared_list",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.g.id

  keywords {
    texts      = local.missing
    match_type = "PHRASE"
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("undeclared local 'local.missing'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn undeclared_variable_reference_errors() {
        let diags = validate_str(
            "undeclared_var",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.g.id

  keywords {
    texts      = var.missing
    match_type = "PHRASE"
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("undeclared variable 'var.missing'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    const AD_TEMPLATE_PREAMBLE: &str = r#"
ad_template "shared" {
  final_urls = ["https://example.com"]
  responsive_search_ad {
    headlines    = ["One Headline", "Two Headline", "Three Headline"]
    descriptions = ["A description here", "Another description here"]
  }
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  manual_cpc { enhanced_cpc_enabled = false }
}

resource "google_ads_ad_group" "g" {
  name           = "G"
  campaign       = google_ads_campaign.c.id
  cpc_bid_micros = 1000000
}
"#;

    // A template that omits `final_urls` — every reference must supply it via an override.
    const URL_AGNOSTIC_PREAMBLE: &str = r#"
ad_template "agnostic" {
  responsive_search_ad {
    headlines    = ["One Headline", "Two Headline", "Three Headline"]
    descriptions = ["A description here", "Another description here"]
  }
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  manual_cpc { enhanced_cpc_enabled = false }
}

resource "google_ads_ad_group" "g" {
  name           = "G"
  campaign       = google_ads_campaign.c.id
  cpc_bid_micros = 1000000
}
"#;

    #[test]
    fn ad_template_reference_validates() {
        let mut content = String::from(
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  status   = "ENABLED"
  template = ad_template.shared
}
"#,
        );
        content.push_str(AD_TEMPLATE_PREAMBLE);
        let diags = validate_str("ad_template_ok", &content);
        let errors: Vec<&String> = diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| &d.message)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    const PARAMETERIZED_PREAMBLE: &str = r#"
ad_template "param" {
  final_urls = ["https://example.com/?utm=${input.slug}"]
  responsive_search_ad {
    headline {
      text = input.headline_1
      pin  = input.slot
    }
    headline { text = "Two Headline" }
    headline { text = "Three Headline" }
    description { text = "A description here" }
    description { text = "Another description here" }
  }
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  manual_cpc { enhanced_cpc_enabled = false }
}

resource "google_ads_ad_group" "g" {
  name           = "G"
  campaign       = google_ads_campaign.c.id
  cpc_bid_micros = 1000000
}
"#;

    fn validate_parameterized(name: &str, ad: &str) -> Vec<String> {
        let mut content = String::from(ad);
        content.push_str(PARAMETERIZED_PREAMBLE);
        validate_str(name, &content)
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn a_fully_bound_template_validates() {
        let errors = validate_parameterized(
            "tmpl_inputs_ok",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.param
  inputs = {
    headline_1 = "Block Facebook Ads Now"
    slug       = "rsa_a"
    slot       = "HEADLINE_1"
  }
}
"#,
        );
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn a_missing_input_names_what_the_template_needs() {
        let errors = validate_parameterized(
            "tmpl_inputs_missing",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.param
  inputs = {
    headline_1 = "Block Facebook Ads Now"
  }
}
"#,
        );
        assert!(
            errors.iter().any(|m| m.contains("which needs")
                && m.contains("input.slug")
                && m.contains("add slug, slot")),
            "{errors:?}"
        );
    }

    #[test]
    fn an_input_the_template_never_uses_is_a_typo_worth_flagging() {
        let errors = validate_parameterized(
            "tmpl_inputs_surplus",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.param
  inputs = {
    headline_1 = "H"
    slug       = "s"
    slot       = "HEADLINE_1"
    headline2  = "typo"
  }
}
"#,
        );
        assert!(
            errors
                .iter()
                .any(|m| m.contains("passes input 'headline2' that its template never uses")),
            "{errors:?}"
        );
    }

    #[test]
    fn a_bound_value_is_type_checked_against_the_field_it_lands_in() {
        // The declaration cannot check `pin = input.slot`; the use site can.
        let errors = validate_parameterized(
            "tmpl_inputs_badtype",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.param
  inputs = {
    headline_1 = "H"
    slug       = "s"
    slot       = "TOPLEFT"
  }
}
"#,
        );
        assert!(
            errors.iter().any(|m| m.contains("invalid value \"TOPLEFT\"")),
            "{errors:?}"
        );
    }

    #[test]
    fn inputs_must_be_a_map() {
        let errors = validate_parameterized(
            "tmpl_inputs_shape",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.param
  inputs   = ["headline_1"]
}
"#,
        );
        assert!(
            errors.iter().any(|m| m.contains("'inputs' must be a map")),
            "{errors:?}"
        );
    }

    #[test]
    fn ad_template_and_ad_block_together_errors() {
        let diags = validate_str(
            "ad_template_xor",
            r#"
ad_template "shared" {
  final_urls = ["https://example.com"]
  responsive_search_ad {
    headlines    = ["One Headline", "Two Headline", "Three Headline"]
    descriptions = ["A description here", "Another description here"]
  }
}

resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.shared
  ad {
    final_urls = ["https://example.com"]
    responsive_search_ad {
      headlines    = ["One Headline", "Two Headline", "Three Headline"]
      descriptions = ["A description here", "Another description here"]
    }
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("sets both an 'ad' block and 'template'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_video_ad_is_a_creative_of_its_own() {
        let diags = validate_str(
            "video_ad_alone",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "YouTube US"
  amount_micros = 50000000
}

resource "google_ads_campaign" "c" {
  name                     = "GH_YouTubeUS_v1"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id
}

resource "google_ads_ad_group" "g" {
  name     = "US in-stream"
  campaign = google_ads_campaign.c.id
}

resource "google_ads_youtube_video_asset" "brand" {
  youtube_video_id = "dQw4w9WgXcQ"
}

resource "google_ads_ad_group_ad" "preroll" {
  ad_group = google_ads_ad_group.g.id

  ad {
    final_urls        = ["https://ghostery.com/?utm_campaign=GH_YouTubeUS_v1"]
    final_mobile_urls = ["https://m.ghostery.com/?utm_campaign=GH_YouTubeUS_v1"]
    display_url       = "www.ghostery.com"

    video_ad {
      video = google_ads_youtube_video_asset.brand.id
    }
  }
}
"#,
        );
        assert!(
            diags.iter().all(|d| !d.is_error()),
            "{:?}",
            diags.iter().filter(|d| d.is_error()).map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_video_ad_beside_another_creative_errors() {
        let diags = validate_str(
            "video_ad_plus_rsa",
            r#"
resource "google_ads_youtube_video_asset" "brand" {
  youtube_video_id = "dQw4w9WgXcQ"
}

resource "google_ads_ad_group_ad" "preroll" {
  ad_group = google_ads_ad_group.g.id

  ad {
    final_urls = ["https://ghostery.com/get"]

    video_ad {
      video = google_ads_youtube_video_asset.brand.id
    }

    video_responsive_ad {
      video = google_ads_youtube_video_asset.brand.id
    }
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("more than one creative")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ad_group_ad_without_ad_or_template_errors() {
        let diags = validate_str(
            "ad_template_neither",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  status   = "ENABLED"
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("must declare an ad")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn dangling_ad_template_reference_errors() {
        let diags = validate_str(
            "ad_template_dangling",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.missing
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("undeclared ad_template 'ad_template.missing'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ad_template_body_type_error_reported() {
        let diags = validate_str(
            "ad_template_body",
            r#"
ad_template "shared" {
  final_urls = ["https://example.com"]
  responsive_search_ad {
    headlines    = "not a list"
    descriptions = ["A description here", "Another description here"]
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.is_error()
                && d.message.contains("expected list of strings or")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_ad_template_errors() {
        let pf = parse_str(
            "dup_tpl",
            r#"
ad_template "shared" {
  final_urls = ["https://example.com"]
}

ad_template "shared" {
  final_urls = ["https://example.org"]
}
"#,
        );
        let (_reg, diags) = AdTemplateRegistry::build(std::slice::from_ref(&pf));
        assert!(
            diags.iter().any(|d| d.message.contains("duplicate ad_template 'shared'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ad_template_final_urls_override_validates() {
        let mut content = String::from(
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group   = google_ads_ad_group.g.id
  template   = ad_template.shared
  final_urls = ["https://example.com/override"]
  path1      = "shop"
}
"#,
        );
        content.push_str(AD_TEMPLATE_PREAMBLE);
        let diags = validate_str("ad_template_override_ok", &content);
        let errors: Vec<&String> = diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| &d.message)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn url_agnostic_template_with_override_validates() {
        let mut content = String::from(
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group   = google_ads_ad_group.g.id
  template   = ad_template.agnostic
  final_urls = ["https://example.com/landing"]
}
"#,
        );
        content.push_str(URL_AGNOSTIC_PREAMBLE);
        let diags = validate_str("agnostic_ok", &content);
        let errors: Vec<&String> = diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| &d.message)
            .collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn url_agnostic_template_without_override_errors() {
        let mut content = String::from(
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.agnostic
}
"#,
        );
        content.push_str(URL_AGNOSTIC_PREAMBLE);
        let diags = validate_str("agnostic_missing_url", &content);
        assert!(
            diags.iter().any(|d| d.message.contains("which declares no final_urls")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn override_with_inline_ad_block_errors() {
        let diags = validate_str(
            "override_inline_ad",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group   = google_ads_ad_group.g.id
  final_urls = ["https://example.com/override"]
  ad {
    final_urls = ["https://example.com"]
    responsive_search_ad {
      headlines    = ["One Headline", "Two Headline", "Three Headline"]
      descriptions = ["A description here", "Another description here"]
    }
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("alongside an inline 'ad' block")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn override_without_template_errors() {
        let diags = validate_str(
            "override_no_template",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group   = google_ads_ad_group.g.id
  final_urls = ["https://example.com/override"]
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("without a 'template'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn template_renders_through_locals() {
        let pf = parse_str(
            "tmpl_render",
            r#"
locals {
  base = "https://example.com/page"
  utm  = "GH_Test_0101"
  url  = "${local.base}?utm_campaign=${local.utm}-rsa_a"
}
"#,
        );
        let bindings = bindings_from(&pf);
        let expr: Expression = "local.url".parse().expect("parse");
        match bindings.resolve_value("tmpl_render", &expr).as_ref() {
            Expression::String(s) => assert_eq!(
                s.as_str(),
                "https://example.com/page?utm_campaign=GH_Test_0101-rsa_a"
            ),
            other => panic!("expected rendered String, got {other:?}"),
        }
    }

    #[test]
    fn template_stringifies_numbers_and_bools() {
        let pf = parse_str(
            "tmpl_scalar",
            r#"
locals {
  radius  = 25
  enabled = true
  label   = "r${local.radius}-${local.enabled}"
}
"#,
        );
        let bindings = bindings_from(&pf);
        let expr: Expression = "local.label".parse().expect("parse");
        match bindings.resolve_value("tmpl_scalar", &expr).as_ref() {
            Expression::String(s) => assert_eq!(s.as_str(), "r25-true"),
            other => panic!("expected rendered String, got {other:?}"),
        }
    }

    #[test]
    fn template_in_string_attribute_validates() {
        let diags = validate_str(
            "tmpl_attr",
            r#"
locals {
  utm = "GH_Test_0101"
}

resource "google_ads_campaign_budget" "t" {
  name          = "t ${local.utm}"
  amount_micros = 1000000
}
"#,
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn template_local_in_list_attribute_validates() {
        let content = format!(
            r#"{LIST_LOCAL_PREAMBLE}
locals {{
  base  = "https://example.com/page"
  url_a = "${{local.base}}?utm_campaign=x-rsa_a"
}}

resource "google_ads_ad_group_ad" "rsa" {{
  ad_group = google_ads_ad_group.g.id
  ad {{
    final_urls = [local.url_a, "${{local.base}}-direct"]
    responsive_search_ad {{
      headlines    = ["One Headline", "Two Headline", "Three Headline"]
      descriptions = ["A description here", "Another description here"]
    }}
  }}
}}
"#
        );
        let diags = validate_str("tmpl_list", &content);
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn template_with_undeclared_local_errors() {
        let diags = validate_str(
            "tmpl_undeclared",
            r#"
resource "google_ads_campaign_budget" "t" {
  name          = "t ${local.nope}"
  amount_micros = 1000000
}
"#,
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("reference to undeclared local 'local.nope'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn template_interpolating_a_list_errors() {
        let diags = validate_str(
            "tmpl_list_err",
            r#"
locals {
  tail = ["a", "b"]
}

resource "google_ads_campaign_budget" "t" {
  name          = "t ${local.tail}"
  amount_micros = 1000000
}
"#,
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("string interpolation must resolve to a string, number, or bool")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn template_cycle_errors() {
        let diags = validate_str(
            "tmpl_cycle",
            r#"
locals {
  a = "x${local.b}"
  b = "y${local.a}"
}

resource "google_ads_campaign_budget" "t" {
  name          = local.a
  amount_micros = 1000000
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("cyclic reference")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn template_same_local_twice_is_not_a_cycle() {
        let diags = validate_str(
            "tmpl_twice",
            r#"
locals {
  utm = "GH"
}

resource "google_ads_campaign_budget" "t" {
  name          = "${local.utm}-${local.utm}"
  amount_micros = 1000000
}
"#,
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn concat_merges_list_locals() {
        let pf = parse_str(
            "concat_merge",
            r#"
locals {
  specific = ["Stop Cookie Pop-Ups for Good"]
  common_tail = ["Add to Chrome, Free", "Open Source & Private"]
  merged = concat(local.specific, local.common_tail)
}
"#,
        );
        let bindings = bindings_from(&pf);
        let expr: Expression = "local.merged".parse().expect("parse");
        match bindings.resolve_value("concat_merge", &expr).as_ref() {
            Expression::Array(arr) => {
                let texts: Vec<&str> = arr
                    .iter()
                    .map(|e| match e {
                        Expression::String(s) => s.as_str(),
                        other => panic!("expected string item, got {other:?}"),
                    })
                    .collect();
                assert_eq!(
                    texts,
                    vec![
                        "Stop Cookie Pop-Ups for Good",
                        "Add to Chrome, Free",
                        "Open Source & Private"
                    ]
                );
            }
            other => panic!("expected merged Array, got {other:?}"),
        }
    }

    #[test]
    fn concat_nests_and_takes_inline_lists() {
        let pf = parse_str(
            "concat_nest",
            r#"
locals {
  a = ["one"]
  b = concat(local.a, ["two"])
  c = concat(local.b, ["three"])
}
"#,
        );
        let bindings = bindings_from(&pf);
        let expr: Expression = "local.c".parse().expect("parse");
        match bindings.resolve_value("concat_nest", &expr).as_ref() {
            Expression::Array(arr) => assert_eq!(arr.iter().count(), 3),
            other => panic!("expected Array, got {other:?}"),
        }
    }

    #[test]
    fn concat_in_rsa_list_attribute_validates() {
        let content = format!(
            r#"{LIST_LOCAL_PREAMBLE}
locals {{
  hl_specific = ["Stop Cookie Pop-Ups for Good", {{ text = "Block Cookie Banners", pin = "HEADLINE_1" }}]
  hl_brand_tail = ["Add to Chrome, Free", "Open Source & Private"]
}}

resource "google_ads_ad_group_ad" "rsa" {{
  ad_group = google_ads_ad_group.g.id
  ad {{
    final_urls = ["https://example.com"]
    responsive_search_ad {{
      headlines    = concat(local.hl_specific, local.hl_brand_tail)
      descriptions = ["A description here", "Another description here"]
    }}
  }}
}}
"#
        );
        let diags = validate_str("concat_rsa", &content);
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn concat_of_non_list_errors() {
        let diags = validate_str(
            "concat_scalar",
            r#"
locals {
  tail = ["a"]
  oops = "not a list"
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  locations                = concat(local.tail, local.oops)
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}
"#,
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("concat() arguments must be lists, got string \"not a list\"")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_function_errors() {
        let diags = validate_str(
            "unknown_fn",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = max(1000000, 2000000)
}
"#,
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("unknown function 'max'; supported functions: concat")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn concat_validates_element_types() {
        let content = format!(
            r#"{LIST_LOCAL_PREAMBLE}
locals {{
  urls = ["https://example.com"]
}}

resource "google_ads_ad_group_ad" "rsa" {{
  ad_group = google_ads_ad_group.g.id
  ad {{
    final_urls = concat(local.urls, [42])
    responsive_search_ad {{
      headlines    = ["One Headline", "Two Headline", "Three Headline"]
      descriptions = ["A description here", "Another description here"]
    }}
  }}
}}
"#
        );
        let diags = validate_str("concat_elem", &content);
        assert!(
            diags.iter().any(|d| d.message.contains("expected string, got number 42")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn for_each_device_criteria_validate_clean() {
        let diags = validate_str(
            "fe_validate",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "t" {{
  name                     = "T"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}}

resource "google_ads_campaign_criterion" "t_devices" {{
  for_each     = ["MOBILE", "TABLET"]
  campaign     = google_ads_campaign.t.id
  bid_modifier = 0

  device {{
    type = each.value
  }}
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn for_each_invalid_each_value_is_type_checked_per_instance() {
        let diags = validate_str(
            "fe_enum",
            &format!(
                r#"{TARGETING_PREAMBLE}
resource "google_ads_campaign" "t" {{
  name                     = "T"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
}}

resource "google_ads_campaign_criterion" "t_devices" {{
  for_each     = ["MOBILE", "FRIDGE"]
  campaign     = google_ads_campaign.t.id
  bid_modifier = 0

  device {{
    type = each.value
  }}
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("invalid value \"FRIDGE\"")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_for_each_instances_across_declarations_error() {
        let diags = validate_str(
            "fe_dup_decl",
            r#"
resource "google_ads_campaign_budget" "b" {
  for_each      = ["a"]
  name          = "B ${each.key}"
  amount_micros = 1000000
}

resource "google_ads_campaign_budget" "b[\"a\"]" {
  name          = "Collides"
  amount_micros = 1000000
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("duplicate resource")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    const CAMPAIGN_DEFAULTS: &str = r#"
defaults "google_ads_campaign" {
  advertising_channel_type = "SEARCH"
  languages = ["en"]
  locations = ["US"]
  contains_eu_political_advertising = "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"

  manual_cpc {
    enhanced_cpc_enabled = false
  }

  network_settings {
    target_google_search = true
    target_search_network = false
    target_content_network = false
    target_partner_search_network = false
  }
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}
"#;

    #[test]
    fn defaults_satisfy_required_attributes() {
        let diags = validate_str(
            "defaults_required",
            &format!(
                r#"{CAMPAIGN_DEFAULTS}
resource "google_ads_campaign" "shell" {{
  name            = "GH_Cookies 08.07.2026"
  campaign_budget = google_ads_campaign_budget.b.id
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    const NAMED_DEFAULTS: &str = r#"
defaults "google_ads_campaign" "search_us" {
  advertising_channel_type = "SEARCH"
  languages                = ["en"]
  locations                = ["US"]
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}
"#;

    #[test]
    fn a_named_defaults_block_satisfies_required_attributes_for_resources_that_opt_in() {
        let diags = validate_str(
            "named_defaults_optin",
            &format!(
                r#"{NAMED_DEFAULTS}
resource "google_ads_campaign" "search" {{
  defaults        = defaults.search_us
  name            = "GH_Search"
  campaign_budget = google_ads_campaign_budget.b.id
}}
"#
            ),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}", errors.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn a_named_defaults_block_does_not_leak_into_resources_that_do_not_opt_in() {
        // The whole point (issue #145): a VIDEO campaign in the same tree must
        // not silently inherit the search shell.
        let diags = validate_str(
            "named_defaults_no_leak",
            &format!(
                r#"{NAMED_DEFAULTS}
resource "google_ads_campaign" "video" {{
  name            = "GH_Video"
  campaign_budget = google_ads_campaign_budget.b.id
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("advertising_channel_type")),
            "the un-opted campaign must still be missing its own channel type: {:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn opting_into_a_name_that_does_not_exist_is_an_error() {
        let diags = validate_str(
            "named_defaults_unknown",
            &format!(
                r#"{NAMED_DEFAULTS}
resource "google_ads_campaign" "search" {{
  defaults        = defaults.search_uk
  name            = "GH_Search"
  campaign_budget = google_ads_campaign_budget.b.id
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("unknown defaults 'search_uk'")
                && d.message.contains("search_us")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_defaults_attribute_is_meta_not_a_campaign_field() {
        // It must not reach the type schema as an unknown attribute.
        let diags = validate_str(
            "named_defaults_meta",
            &format!(
                r#"{NAMED_DEFAULTS}
resource "google_ads_campaign" "search" {{
  defaults        = defaults.search_us
  name            = "GH_Search"
  campaign_budget = google_ads_campaign_budget.b.id
}}
"#
            ),
        );
        assert!(
            !diags.iter().any(|d| d.message.contains("unknown attribute 'defaults'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_named_defaults_body_is_schema_validated_at_declaration() {
        let diags = validate_str(
            "named_defaults_body",
            r#"
defaults "google_ads_campaign" "search_us" {
  advertising_channel_type = "TELEPATHY"
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("invalid value \"TELEPATHY\"")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_named_defaults_for_one_type_coexist_but_a_repeated_name_does_not() {
        let ok = validate_str(
            "named_defaults_two",
            r#"
defaults "google_ads_campaign" "search_us" {
  advertising_channel_type = "SEARCH"
}

defaults "google_ads_campaign" "video_us" {
  advertising_channel_type = "VIDEO"
}
"#,
        );
        assert!(
            !ok.iter().any(|d| d.message.contains("duplicate defaults")),
            "distinct names are the point: {:?}",
            ok.iter().map(|d| &d.message).collect::<Vec<_>>()
        );

        let dup = validate_str(
            "named_defaults_dup",
            r#"
defaults "google_ads_campaign" "search_us" {
  advertising_channel_type = "SEARCH"
}

defaults "google_ads_campaign" "search_us" {
  advertising_channel_type = "VIDEO"
}
"#,
        );
        assert!(
            dup.iter().any(|d| d.message.contains("duplicate defaults 'search_us'")),
            "{:?}",
            dup.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn defaults_body_is_schema_validated_at_declaration() {
        let diags = validate_str(
            "defaults_body",
            r#"
defaults "google_ads_campaign" {
  advertising_channel_type = "TELEPATHY"
  frequency = 7
}
"#,
        );
        let msgs: Vec<&String> = diags.iter().map(|d| &d.message).collect();
        assert!(
            msgs.iter().any(|m| m.contains("invalid value \"TELEPATHY\"")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("unknown attribute 'frequency' in defaults.google_ads_campaign")),
            "{msgs:?}"
        );
    }

    #[test]
    fn defaults_unknown_type_errors() {
        let diags = validate_str(
            "defaults_unknown_type",
            r#"
defaults "google_ads_flying_carpet" {
  status = "ENABLED"
}
"#,
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("unknown resource type 'google_ads_flying_carpet' in defaults block")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_defaults_for_a_type_error() {
        let diags = validate_str(
            "defaults_dup",
            r#"
defaults "google_ads_campaign" {
  advertising_channel_type = "SEARCH"
}

defaults "google_ads_campaign" {
  advertising_channel_type = "DISPLAY"
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("duplicate defaults for 'google_ads_campaign'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn defaults_cannot_provide_an_ad_body() {
        let diags = validate_str(
            "defaults_ad_body",
            r#"
defaults "google_ads_ad_group_ad" {
  template = ad_template.shared
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("defaults cannot provide an ad body")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn defaults_locations_conflict_with_explicit_positive_criterion() {
        let diags = validate_str(
            "defaults_conflict",
            &format!(
                r#"{CAMPAIGN_DEFAULTS}
resource "google_ads_campaign" "shell" {{
  name            = "Shell"
  campaign_budget = google_ads_campaign_budget.b.id
}}

resource "google_ads_campaign_criterion" "extra_geo" {{
  campaign = google_ads_campaign.shell.id
  location {{
    geo_target_constant = "geoTargetConstants/2826"
  }}
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("already declares inline 'locations'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_flight_date_that_is_not_a_date_errors() {
        for (label, value, expected) in [
            ("date_month", "2026-13-01", "month must be 01-12"),
            ("date_day", "2026-02-30", "2026-02 has 28 days"),
            ("date_shape", "11.08.2026", "expected four-digit year"),
            ("date_short", "2026-8-1", "expected four-digit year"),
        ] {
            let diags = validate_str(
                label,
                &format!(
                    r#"
resource "google_ads_campaign_budget" "b" {{
  name          = "B"
  amount_micros = 1000000
}}

resource "google_ads_campaign" "c" {{
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  end_date                 = "{value}"
}}
"#
                ),
            );
            assert!(
                diags.iter().any(|d| d.message.contains(expected)),
                "{value}: {:?}",
                diags.iter().map(|d| &d.message).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn a_leap_day_is_a_date() {
        let diags = validate_str(
            "date_leap",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  start_date               = "2028-02-29"
  end_date                 = "2028-12-31"
}
"#,
        );
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_geo_target_type_outside_the_two_google_offers_errors() {
        let diags = validate_str(
            "geo_target_type",
            r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  locations                = ["US"]

  geo_target_type_setting {
    positive_geo_target_type = "PRESENCE"
    negative_geo_target_type = "SEARCH_INTEREST"
  }
}
"#,
        );
        let messages: Vec<&String> = diags.iter().map(|d| &d.message).collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].contains("SEARCH_INTEREST")
                && messages[0].contains("PRESENCE_OR_INTEREST"),
            "{messages:?}"
        );
    }

    #[test]
    fn resource_missing_attr_without_defaults_still_errors() {
        let diags = validate_str(
            "defaults_other_type",
            &format!(
                r#"{CAMPAIGN_DEFAULTS}
resource "google_ads_ad_group" "g" {{
  campaign = google_ads_campaign.shell.id
}}

resource "google_ads_campaign" "shell" {{
  name            = "Shell"
  campaign_budget = google_ads_campaign_budget.b.id
}}
"#
            ),
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("missing required attribute 'name' in google_ads_ad_group.g")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn template_rendered_enum_is_validated() {
        let diags = validate_str(
            "tmpl_enum",
            r#"
locals {
  st = "ENAB"
}

resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "c" {
  name                     = "C"
  advertising_channel_type = "SEARCH"
  campaign_budget          = google_ads_campaign_budget.b.id
  status                   = "${local.st}LED_X"
}
"#,
        );
        assert!(
            diags.iter().any(|d| d.message.contains("invalid value \"ENABLED_X\"")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    const ADOPT_ONLY_VIDEO: &str = r#"
resource "google_ads_campaign_budget" "b" {
  name          = "B"
  amount_micros = 1000000
}

resource "google_ads_campaign" "v" {
  name                     = "GH_YouTube_FR Instream"
  advertising_channel_type = "VIDEO"
  campaign_budget          = google_ads_campaign_budget.b.id

  lifecycle {
    create = false
  }
}
"#;

    #[test]
    fn lifecycle_block_validates_clean() {
        let diags = validate_str("lifecycle_ok", ADOPT_ONLY_VIDEO);
        assert!(
            diags.is_empty(),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn lifecycle_rejects_an_unknown_attribute() {
        let diags = validate_str(
            "lifecycle_unknown",
            &ADOPT_ONLY_VIDEO.replace("create = false", "adopt_match = \"name\""),
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("unknown attribute 'adopt_match' in google_ads_campaign.v.lifecycle")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn lifecycle_rejects_a_non_bool_create() {
        let diags = validate_str(
            "lifecycle_type",
            &ADOPT_ONLY_VIDEO.replace("create = false", "create = \"no\""),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("expected bool")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn duplicate_lifecycle_blocks_error() {
        let diags = validate_str(
            "lifecycle_dup",
            &ADOPT_ONLY_VIDEO.replace(
                "  lifecycle {\n    create = false\n  }",
                "  lifecycle {\n    create = false\n  }\n\n  lifecycle {\n    create = true\n  }",
            ),
        );
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("duplicate 'lifecycle' block in google_ads_campaign.v")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn lifecycle_on_a_criterion_is_rejected() {
        let diags = validate_str(
            "lifecycle_criterion",
            r#"
resource "google_ads_ad_group_criterion" "kw" {
  ad_group = google_ads_ad_group.ag.id

  keyword {
    text       = "shoes"
    match_type = "EXACT"
  }

  lifecycle {
    create = false
  }
}
"#,
        );
        assert!(
            diags.iter().any(|d| d
                .message
                .contains("'lifecycle' is not supported on google_ads_ad_group_criterion")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn lifecycle_does_not_hide_a_sibling_error() {
        let diags = validate_str(
            "lifecycle_sibling",
            &ADOPT_ONLY_VIDEO.replace("  lifecycle {", "  budget_typo = 1\n\n  lifecycle {"),
        );
        assert!(
            diags.iter().any(|d| d.message.contains("unknown attribute 'budget_typo'")),
            "{:?}",
            diags.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }
}
