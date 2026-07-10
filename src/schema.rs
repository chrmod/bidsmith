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
pub const DEFAULT_NEGATIVE: bool = false;

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
const RSA_PIN: &[&str] = &[
    "HEADLINE_1",
    "HEADLINE_2",
    "HEADLINE_3",
    "DESCRIPTION_1",
    "DESCRIPTION_2",
];
const PROXIMITY_RADIUS_UNITS: &[&str] = &["MILES", "KILOMETERS"];
const DEVICE_TYPE: &[&str] = &["MOBILE", "DESKTOP", "TABLET", "CONNECTED_TV", "OTHER"];
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
            ],
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
                // not (and cannot) upload the video file itself; see the CLI limitation
                // notice surfaced by `plan` and the `google_ads_youtube_video_asset` lint.
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
                        ],
                        blocks: vec![],
                    },
                },
                // A Demand Gen video responsive ad — the ad type a DEMAND_GEN
                // campaign carries. A distinct API message from video_responsive_ad
                // (VIDEO campaigns); the video assets are UI-managed like the
                // youtube video ad, so bidsmith round-trips the creative but does
                // not create/update it on the live account.
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
                    attr("amount_micros", FieldType::Integer, true),
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
                    attr("languages", FieldType::LanguageList, false),
                    attr("locations", FieldType::LocationList, false),
                ],
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
                    NestedBlockSchema {
                        name: "network_settings",
                        schema: BlockSchema {
                            attributes: vec![
                                attr("target_google_search", FieldType::Bool, false),
                                attr("target_search_network", FieldType::Bool, false),
                                attr("target_content_network", FieldType::Bool, false),
                                attr(
                                    "target_partner_search_network",
                                    FieldType::Bool,
                                    false,
                                ),
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
                ],
                blocks: vec![
                    keyword_block(),
                    compact_keywords_block("keywords"),
                    negative_keyword_block(),
                    compact_keywords_block("negative_keywords"),
                ],
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
                blocks: vec![
                    keyword_block(),
                    negative_keyword_block(),
                    compact_keywords_block("negative_keywords"),
                    NestedBlockSchema {
                        name: "device",
                        schema: BlockSchema {
                            attributes: vec![attr(
                                "type",
                                FieldType::Enum(DEVICE_TYPE),
                                true,
                            )],
                            blocks: vec![],
                        },
                    },
                    NestedBlockSchema {
                        name: "location",
                        schema: BlockSchema {
                            attributes: vec![attr(
                                "geo_target_constant",
                                FieldType::String,
                                true,
                            )],
                            blocks: vec![],
                        },
                    },
                    NestedBlockSchema {
                        name: "language",
                        schema: BlockSchema {
                            attributes: vec![attr(
                                "language_constant",
                                FieldType::String,
                                true,
                            )],
                            blocks: vec![],
                        },
                    },
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
                ],
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
                ],
                blocks: vec![ad_block(true)],
            },
        );

        m.insert(
            "google_ads_ad_group",
            BlockSchema {
                attributes: vec![
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
                    attr("cpc_bid_micros", FieldType::Integer, false),
                ],
                blocks: vec![],
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
        // YouTube channel, addressed by its 11-char video id. bidsmith records the
        // reference so a video ad can point at it; it never uploads the video file
        // (that is the YouTube Data API's job, a separate system). See the lint in
        // `lint.rs` and the `plan` video notice for the full workflow.
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
                    attr("asset", FieldType::Ref(ASSET_TYPES), true),
                    attr(
                        "field_type",
                        FieldType::Enum(ASSET_FIELD_TYPE),
                        true,
                    ),
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
                    attr("asset", FieldType::Ref(ASSET_TYPES), true),
                    attr("field_type", FieldType::Enum(ASSET_FIELD_TYPE), true),
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
                    attr("asset", FieldType::Ref(ASSET_TYPES), true),
                    attr("field_type", FieldType::Enum(ASSET_FIELD_TYPE), true),
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

    for f in files {
        validate_top_level(f, &registry, &locals, &variables, &mut diags);
    }

    validate_ad_templates(files, &templates, &registry, &locals, &variables, &mut diags);
    validate_targeting_conflicts(files, &registry, &mut diags);

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
                        diags,
                    );
                    validate_ad_creative_exclusivity(f, &b.body, &address, diags);
                }
                "resource"
                    if b.labels.len() == 2
                        && b.labels[0].as_str() == "google_ads_ad_group_ad" =>
                {
                    validate_ad_group_ad_template(f, b, templates, diags);
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

fn validate_ad_group_ad_template(
    file: &ParsedFile,
    block: &Block,
    templates: &AdTemplateRegistry,
    diags: &mut Vec<Diag>,
) {
    let address = format!("google_ads_ad_group_ad.{}", block.labels[1].as_str());
    let mut has_ad_block = false;
    let mut template: Option<(std::ops::Range<usize>, &Expression)> = None;
    let mut overrides: Vec<(&str, std::ops::Range<usize>)> = Vec::new();
    let mut has_final_urls_override = false;
    for s in block.body.iter() {
        match s {
            Structure::Block(b) if b.ident.as_str() == "ad" => has_ad_block = true,
            Structure::Attribute(a) => match a.key.as_str() {
                "template" => template = Some((span_of(a.key.span()), &a.value)),
                "final_urls" => {
                    overrides.push(("final_urls", span_of(a.key.span())));
                    if !is_empty_array_literal(&a.value) {
                        has_final_urls_override = true;
                    }
                }
                "path1" | "path2" => overrides.push((a.key.as_str(), span_of(a.key.span()))),
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
/// a `responsive_search_ad`, a `video_responsive_ad`, or a
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
}

/// A campaign can declare targeting *either* inline (`languages` / `locations`)
/// *or* via explicit positive `google_ads_campaign_criterion` resources — not
/// both for the same axis. (Negative locations, proximity, keywords, and
/// non-positive criteria are unaffected — they only live in explicit form.)
fn validate_targeting_conflicts(
    files: &[ParsedFile],
    registry: &ResourceRegistry,
    diags: &mut Vec<Diag>,
) {
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
            let mut axes = InlineAxes::default();
            for inner in b.body.iter() {
                if let Structure::Attribute(a) = inner {
                    match a.key.as_str() {
                        "languages" => axes.languages = true,
                        "locations" => axes.locations = true,
                        _ => {}
                    }
                }
            }
            if axes.languages || axes.locations {
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
                        _ => {}
                    },
                }
            }
            if negative || (loc_block.is_none() && lang_block.is_none()) {
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
        }
    }
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
    diags: &mut Vec<Diag>,
) {
    for s in file.body.iter() {
        match s {
            Structure::Attribute(a) => {
                diags.push(Diag::new(
                    file.src.clone(),
                    span_of(a.key.span()),
                    format!(
                        "top-level attributes are not allowed; place '{}' inside a 'provider', 'resource', 'locals', 'variable', 'module', or 'ad_template' block",
                        a.key.as_str()
                    ),
                ));
            }
            Structure::Block(b) => match b.ident.as_str() {
                "provider" => validate_provider(file, b, registry, locals, variables, diags),
                "resource" => validate_resource(file, b, registry, locals, variables, diags),
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
                            "unknown top-level block '{other}'; expected 'provider', 'resource', 'locals', 'variable', 'module', or 'ad_template'"
                        ),
                    ));
                }
            },
        }
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
        diags,
    );
}

fn validate_resource(
    file: &ParsedFile,
    block: &Block,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
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
    validate_body(
        file,
        block,
        &block.body,
        schema,
        &format!("{ty}.{name}"),
        registry,
        locals,
        variables,
        diags,
    );
}

fn validate_body(
    file: &ParsedFile,
    containing: &Block,
    body: &Body,
    schema: &BlockSchema,
    address: &str,
    registry: &ResourceRegistry,
    locals: &LocalsRegistry,
    variables: &VariablesRegistry,
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
            }
        }
    }

    for a in &schema.attributes {
        if a.required && !seen.contains(a.name) {
            diags.push(Diag::new(
                file.src.clone(),
                span_of(containing.ident.span()),
                format!("missing required attribute '{}' in {}", a.name, address),
            ));
        }
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
            _ => return None,
        }
    }
    Some(path)
}

fn describe_field_type(ty: &FieldType) -> String {
    match ty {
        FieldType::String => "string".to_string(),
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
}
