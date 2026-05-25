use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

use hcl_edit::Span;
use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Block, Body, Structure};
use serde::Serialize;

use crate::diagnostics::Diag;
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
    List(Box<FieldType>),
    RsaAssetList,
}

impl FieldType {
    pub fn list_of(inner: FieldType) -> Self {
        FieldType::List(Box::new(inner))
    }
}

pub struct AttributeSchema {
    pub name: &'static str,
    pub ty: FieldType,
    pub required: bool,
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

fn attr(name: &'static str, ty: FieldType, required: bool) -> AttributeSchema {
    AttributeSchema { name, ty, required }
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
                    ),
                    attr("explicitly_shared", FieldType::Bool, false),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_campaign",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr("status", FieldType::Enum(STATUS), false),
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
                    ),
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
                    attr("status", FieldType::Enum(STATUS), false),
                    attr("negative", FieldType::Bool, false),
                    attr("cpc_bid_micros", FieldType::Integer, false),
                ],
                blocks: vec![
                    keyword_block(),
                    NestedBlockSchema {
                        name: "negative_keyword",
                        schema: BlockSchema {
                            attributes: vec![
                                attr("text", FieldType::String, true),
                                attr(
                                    "match_type",
                                    FieldType::Enum(KEYWORD_MATCH_TYPE),
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
            "google_ads_campaign_criterion",
            BlockSchema {
                attributes: vec![
                    attr(
                        "campaign",
                        FieldType::Ref(&["google_ads_campaign"]),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false),
                    attr("negative", FieldType::Bool, false),
                ],
                blocks: vec![
                    keyword_block(),
                    NestedBlockSchema {
                        name: "negative_keyword",
                        schema: BlockSchema {
                            attributes: vec![
                                attr("text", FieldType::String, true),
                                attr(
                                    "match_type",
                                    FieldType::Enum(KEYWORD_MATCH_TYPE),
                                    true,
                                ),
                            ],
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
                    attr("status", FieldType::Enum(STATUS), false),
                ],
                blocks: vec![NestedBlockSchema {
                    name: "ad",
                    schema: BlockSchema {
                        attributes: vec![
                            attr("name", FieldType::String, false),
                            attr(
                                "final_urls",
                                FieldType::list_of(FieldType::String),
                                true,
                            ),
                        ],
                        blocks: vec![NestedBlockSchema {
                            name: "responsive_search_ad",
                            schema: BlockSchema {
                                attributes: vec![
                                    attr("path1", FieldType::String, false),
                                    attr("path2", FieldType::String, false),
                                    attr("headlines", FieldType::RsaAssetList, false),
                                    attr("descriptions", FieldType::RsaAssetList, false),
                                ],
                                blocks: vec![
                                    rsa_asset_block("headline"),
                                    rsa_asset_block("description"),
                                ],
                            },
                        }],
                    },
                }],
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
                    attr("status", FieldType::Enum(STATUS), false),
                    attr(
                        "type",
                        FieldType::Enum(&[
                            "SEARCH_STANDARD",
                            "DISPLAY_STANDARD",
                            "SHOPPING_PRODUCT_ADS",
                            "VIDEO_BUMPER",
                            "VIDEO_TRUE_VIEW_IN_STREAM",
                            "VIDEO_TRUE_VIEW_IN_DISPLAY",
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
                    ),
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

        m.insert(
            "google_ads_shared_set",
            BlockSchema {
                attributes: vec![
                    attr("name", FieldType::String, true),
                    attr("type", FieldType::Enum(SHARED_SET_TYPE), false),
                    attr("status", FieldType::Enum(SHARED_SET_STATUS), false),
                ],
                blocks: vec![NestedBlockSchema {
                    name: "negative_keyword",
                    schema: BlockSchema {
                        attributes: vec![
                            attr("text", FieldType::String, true),
                            attr(
                                "match_type",
                                FieldType::Enum(KEYWORD_MATCH_TYPE),
                                true,
                            ),
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
                    ),
                ],
                blocks: vec![],
            },
        );

        m.insert(
            "google_ads_customer_asset",
            BlockSchema {
                attributes: vec![
                    attr(
                        "asset",
                        FieldType::Ref(&["google_ads_call_asset"]),
                        true,
                    ),
                    attr(
                        "field_type",
                        FieldType::Enum(ASSET_FIELD_TYPE),
                        true,
                    ),
                    attr("status", FieldType::Enum(STATUS), false),
                ],
                blocks: vec![],
            },
        );

        m
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
                    attr("customer_id", FieldType::String, true),
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

    pub fn resolve_value<'a>(&'a self, from_module: &str, expr: &'a Expression) -> &'a Expression {
        let mut visited: HashSet<(BindingKind, String)> = HashSet::new();
        let mut current_module = from_module.to_string();
        let mut current_expr: &Expression = expr;
        while let Some((kind, name)) = binding_ref(current_expr) {
            let (qualified, next_module, next_value) = match kind {
                BindingKind::Local => match self.locals.resolve(&current_module, &name) {
                    Resolution::Found(q) => match self.locals.get(&q) {
                        Some(decl) => (q, decl.module.clone(), &decl.value),
                        None => return current_expr,
                    },
                    _ => return current_expr,
                },
                BindingKind::Var => match self.variables.resolve(&current_module, &name) {
                    Resolution::Found(q) => match self.variables.get(&q) {
                        Some(decl) => (q, decl.module.clone(), &decl.value),
                        None => return current_expr,
                    },
                    _ => return current_expr,
                },
            };
            if !visited.insert((kind, qualified)) {
                return current_expr;
            }
            current_module = next_module;
            current_expr = next_value;
        }
        current_expr
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
    let ok = match (ty, expr) {
        (VarType::String, Expression::String(_)) => true,
        (VarType::Number, Expression::Number(_)) => true,
        (VarType::Bool, Expression::Bool(_)) => true,
        _ => false,
    };
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
    let (registry, mut diags) = ResourceRegistry::build(files);
    let (locals, locals_diags) = LocalsRegistry::build(files);
    diags.extend(locals_diags);
    let (variables, variables_diags) = VariablesRegistry::build(files, inputs);
    diags.extend(variables_diags);

    for f in files {
        validate_top_level(f, &registry, &locals, &variables, &mut diags);
    }

    diags.sort_by(|a, b| {
        (a.src.name(), a.span.offset()).cmp(&(b.src.name(), b.span.offset()))
    });
    diags
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
                        "top-level attributes are not allowed; place '{}' inside a 'provider', 'resource', 'locals', 'variable', or 'module' block",
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
                other => {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(b.ident.span()),
                        format!(
                            "unknown top-level block '{other}'; expected 'provider', 'resource', 'locals', 'variable', or 'module'"
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
    let expr = match resolve_binding_chain(file, expr, locals, variables, diags) {
        BindingResolution::NotABinding => expr,
        BindingResolution::Resolved(value) => value,
        BindingResolution::Failed => return,
    };
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
                if !values.iter().any(|&x| x == v) {
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
                    validate_rsa_asset_item(file, item, diags);
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
        FieldType::Ref(targets) => {
            validate_ref(file, expr, span, targets, registry, diags, false);
        }
        FieldType::RefOrResourceName(targets) => {
            if matches!(expr, Expression::String(_)) {
                return;
            }
            validate_ref(file, expr, span, targets, registry, diags, true);
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
    NotABinding,
    Resolved(&'a Expression),
    Failed,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum BindingKind {
    Local,
    Var,
}

impl BindingKind {
    fn prefix(self) -> &'static str {
        match self {
            BindingKind::Local => "local",
            BindingKind::Var => "var",
        }
    }
}

fn resolve_binding_chain<'a>(
    file: &ParsedFile,
    expr: &'a Expression,
    locals: &'a LocalsRegistry,
    variables: &'a VariablesRegistry,
    diags: &mut Vec<Diag>,
) -> BindingResolution<'a> {
    let Some((first_kind, first_name)) = binding_ref(expr) else {
        return BindingResolution::NotABinding;
    };
    let use_span = span_of(expr.span());
    let mut visited: HashSet<(BindingKind, String)> = HashSet::new();
    let mut current_module = file.module.as_str();
    let mut current_kind = first_kind;
    let mut current_name = first_name;
    loop {
        let (qualified, decl_module, value): (String, &str, &'a Expression) = match current_kind {
            BindingKind::Local => {
                match locals.resolve(current_module, &current_name) {
                    Resolution::Found(q) => match locals.get(&q) {
                        Some(decl) => (q, decl.module.as_str(), &decl.value),
                        None => return BindingResolution::Failed,
                    },
                    Resolution::Missing => {
                        diags.push(Diag::new(
                            file.src.clone(),
                            use_span.clone(),
                            format!("reference to undeclared local 'local.{current_name}'"),
                        ));
                        return BindingResolution::Failed;
                    }
                    Resolution::Ambiguous(modules) => {
                        let mut sorted: Vec<&str> = modules.iter().map(String::as_str).collect();
                        sorted.sort();
                        diags.push(Diag::new(
                            file.src.clone(),
                            use_span,
                            format!(
                                "ambiguous reference to 'local.{current_name}'; declared in modules [{}] — rename one of the locals so each is unique within its module",
                                sorted.join(", ")
                            ),
                        ));
                        return BindingResolution::Failed;
                    }
                }
            }
            BindingKind::Var => {
                match variables.resolve(current_module, &current_name) {
                    Resolution::Found(q) => match variables.get(&q) {
                        Some(decl) => (q, decl.module.as_str(), &decl.value),
                        None => return BindingResolution::Failed,
                    },
                    Resolution::Missing => {
                        diags.push(Diag::new(
                            file.src.clone(),
                            use_span.clone(),
                            format!("reference to undeclared variable 'var.{current_name}'"),
                        ));
                        return BindingResolution::Failed;
                    }
                    Resolution::Ambiguous(modules) => {
                        let mut sorted: Vec<&str> = modules.iter().map(String::as_str).collect();
                        sorted.sort();
                        diags.push(Diag::new(
                            file.src.clone(),
                            use_span,
                            format!(
                                "ambiguous reference to 'var.{current_name}'; declared in modules [{}] — rename one of the variables so each is unique within its module",
                                sorted.join(", ")
                            ),
                        ));
                        return BindingResolution::Failed;
                    }
                }
            }
        };
        if !visited.insert((current_kind, qualified.clone())) {
            diags.push(Diag::new(
                file.src.clone(),
                use_span,
                format!(
                    "cyclic reference involving '{}.{current_name}'",
                    current_kind.prefix()
                ),
            ));
            return BindingResolution::Failed;
        }
        match binding_ref(value) {
            Some((next_kind, next_name)) => {
                current_module = decl_module;
                current_kind = next_kind;
                current_name = next_name;
            }
            None => {
                return BindingResolution::Resolved(value);
            }
        }
    }
}

fn binding_ref(expr: &Expression) -> Option<(BindingKind, String)> {
    let Expression::Traversal(t) = expr else {
        return None;
    };
    let path = extract_traversal_path(t)?;
    if path.len() < 2 {
        return None;
    }
    let kind = match path[0].as_str() {
        "local" => BindingKind::Local,
        "var" => BindingKind::Var,
        _ => return None,
    };
    Some((kind, path[1].clone()))
}

fn extract_traversal_path(t: &Traversal) -> Option<Vec<String>> {
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
        FieldType::List(inner) => format!("list of {}", describe_field_type(inner)),
        FieldType::RsaAssetList => "list of strings or { text, pin? } objects".to_string(),
    }
}

fn validate_rsa_asset_item(file: &ParsedFile, expr: &Expression, diags: &mut Vec<Diag>) {
    let span = span_of(expr.span());
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
                match ident.as_str() {
                    "text" => {
                        has_text = true;
                        if !matches!(value.expr(), Expression::String(_)) {
                            diags.push(Diag::new(
                                file.src.clone(),
                                span_of(value.expr().span()),
                                format!(
                                    "RSA asset 'text' must be a string, got {}",
                                    describe_expr(value.expr())
                                ),
                            ));
                        }
                    }
                    "pin" => match value.expr() {
                        Expression::String(s) => {
                            let v = s.as_str();
                            if !RSA_PIN.iter().any(|&x| x == v) {
                                diags.push(Diag::new(
                                    file.src.clone(),
                                    span_of(value.expr().span()),
                                    format!(
                                        "invalid pin \"{v}\"; expected one of [{}]",
                                        RSA_PIN.join(", ")
                                    ),
                                ));
                            }
                        }
                        other => diags.push(Diag::new(
                            file.src.clone(),
                            span_of(value.expr().span()),
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

fn describe_expr(expr: &Expression) -> String {
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
        _ => "expression".to_string(),
    }
}

fn join_or(items: &[&str]) -> String {
    items.join(" or ")
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
    #[serde(flatten)]
    pub ty: TypeDoc,
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
        ty: ty_to_doc(&a.ty),
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
        FieldType::List(inner) => TypeDoc::List {
            element: Box::new(ty_to_doc(inner)),
        },
        FieldType::RsaAssetList => TypeDoc::RsaAssetList,
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
        match resolved {
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
        match bindings.resolve_value("chain", &expr) {
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
        assert!(matches!(resolved, Expression::Traversal(_)));
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
        match bindings.resolve_value("var_default", &expr) {
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
        match bindings.resolve_value("var_input", &expr) {
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
        match bindings.resolve_value("loc_via_var", &expr) {
            Expression::Number(n) => assert_eq!(n.as_f64(), Some(10000000.0)),
            other => panic!("expected Number, got {other:?}"),
        }
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
}
