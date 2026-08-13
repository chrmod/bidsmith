use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::process::ExitCode;

use serde::Deserialize;

/// Prefix of the Google Ads label name bidsmith writes on every managed
/// resource to record its address: `bidsmith:address=<module>.<type>.<name>`.
/// The label name is bidsmith's identity key for the four labelable resource
/// types (campaign, ad_group, ad_group_ad, ad_group_criterion).
pub const ADDRESS_LABEL_PREFIX: &str = "bidsmith:address=";

/// Google Ads caps `label.name` at 80 characters and rejects any longer name
/// with `Too long.` — sinking the whole atomic adoption batch.
pub const MAX_LABEL_NAME_LEN: usize = 80;

/// Prefix of the label bidsmith associates with a campaign / ad group to
/// record that it manages a criterion *category* there (e.g.
/// `bidsmith:owns=keyword_negative`). Criteria members carry no identity of
/// their own (the API forbids labels on negative criteria), so this claim is
/// what lets `plan` keep destroying orphaned members after the last declared
/// member of a category is removed.
pub const OWNS_LABEL_PREFIX: &str = "bidsmith:owns=";

/// The label-name payload (the part after `ADDRESS_LABEL_PREFIX`) for an
/// address. Short addresses are kept verbatim — backward compatible with labels
/// already in the account. An address that would push the label past the 80-char
/// cap is encoded as a legible head plus a stable SHA-256 suffix that always
/// fits and stays unique. Matching, label reuse, and relabeling all run in this
/// payload space, so both sides agree regardless of which form an address took.
pub fn address_label_payload(address: &str) -> String {
    let budget = MAX_LABEL_NAME_LEN - ADDRESS_LABEL_PREFIX.len();
    if address.len() <= budget {
        return address.to_string();
    }
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    const HASH_BYTES: usize = 12;
    const SEP: char = '~';
    let mut hasher = Sha256::new();
    hasher.update(address.as_bytes());
    let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(&hasher.finalize()[..HASH_BYTES]);
    let head = truncate_on_char_boundary(address, budget - SEP.len_utf8() - hash.len());
    format!("{head}{SEP}{hash}")
}

fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[derive(Deserialize)]
pub struct ExportInput {
    pub customer_id: String,
    #[serde(default)]
    pub login_customer_id: Option<String>,
    /// ISO-4217 code the account bills in, read off the `customer` query.
    /// Live-only; None for declared state and for pre-currency cache entries.
    #[serde(default)]
    pub currency_code: Option<String>,
    #[serde(default)]
    pub campaign_budgets: Vec<JsonBudget>,
    #[serde(default)]
    pub campaigns: Vec<JsonCampaign>,
    #[serde(default)]
    pub ad_groups: Vec<JsonAdGroup>,
    #[serde(default)]
    pub ad_group_ads: Vec<JsonAdGroupAd>,
    #[serde(default)]
    pub ad_group_criteria: Vec<JsonAdGroupCriterion>,
    #[serde(default)]
    pub campaign_criteria: Vec<JsonCampaignCriterion>,
    #[serde(default)]
    pub conversion_actions: Vec<JsonConversionAction>,
    #[serde(default)]
    pub call_assets: Vec<JsonCallAsset>,
    #[serde(default)]
    pub sitelink_assets: Vec<JsonSitelinkAsset>,
    #[serde(default)]
    pub callout_assets: Vec<JsonCalloutAsset>,
    #[serde(default)]
    pub structured_snippet_assets: Vec<JsonStructuredSnippetAsset>,
    #[serde(default)]
    pub customer_assets: Vec<JsonCustomerAsset>,
    #[serde(default)]
    pub campaign_assets: Vec<JsonCampaignAsset>,
    #[serde(default)]
    pub ad_group_assets: Vec<JsonAdGroupAsset>,
    #[serde(default)]
    pub shared_sets: Vec<JsonSharedSet>,
    #[serde(default)]
    pub shared_criteria: Vec<JsonSharedCriterion>,
    #[serde(default)]
    pub campaign_shared_sets: Vec<JsonCampaignSharedSet>,
    #[serde(default)]
    pub youtube_video_assets: Vec<JsonYoutubeVideoAsset>,
    #[serde(default)]
    pub custom_audiences: Vec<JsonCustomAudience>,
    /// Live `bidsmith:address=<addr>` labels keyed by address -> label
    /// resource_name. Lets the mutate builder reuse an existing label instead
    /// of re-creating one (a duplicate name is an API error). Live-only; empty
    /// for declared state.
    #[serde(default)]
    pub labels: HashMap<String, String>,
    /// Live `bidsmith:owns=<category>` labels keyed by category -> label
    /// resource_name, for the same reuse-by-name purpose. Live-only.
    #[serde(default)]
    pub claim_labels: HashMap<String, String>,
    /// Addresses the file marked `lifecycle { create = false }`. Declared-side
    /// only: live state has nothing left to adopt.
    #[serde(default)]
    pub adopt_only: HashSet<String>,
    /// The modules a **partial** run read, or `None` when the input covered
    /// every `.bid` file the project has. Removing a labeled live resource is
    /// authorized by its absence from the declaration, and absence only means
    /// something against a complete input — so a partial run keeps its
    /// removals inside the modules it read rather than reading "not declared
    /// here" as "not declared anywhere" (issue #160). Declared-side only.
    #[serde(default)]
    pub partial_modules: Option<BTreeSet<String>>,
    /// Asset field types (`SITELINK`, `CALLOUT`, …) the `provider` block's
    /// `owns` list claims at the account level: a live `customer_asset` of one
    /// of them that no block declares is destroyed. Declared-side only —
    /// nothing live can carry an account-wide claim, so it has to be said in
    /// the file.
    #[serde(default)]
    pub owned_account_assets: HashSet<String>,
    /// Whether the `provider` block's `owns` list claims what Google's
    /// automation attached to the account itself. Separate from the field-type
    /// set above because it names a source, not a kind of asset: no `.bid` can
    /// declare one, so nothing about it is provable from a declaration.
    #[serde(default)]
    pub owns_account_automatic_assets: bool,
    /// Live campaign id -> criterion categories a `bidsmith:owns=` label claims
    /// on it. Live-only; empty for declared state.
    #[serde(default)]
    pub campaign_claims: HashMap<String, Vec<String>>,
    /// Live ad group id -> criterion categories claimed on it. Live-only.
    #[serde(default)]
    pub ad_group_claims: HashMap<String, Vec<String>>,
}

#[derive(Deserialize)]
pub struct JsonBudget {
    pub id: String,
    pub name: String,
    /// The daily rate. `None` on a `CUSTOM_PERIOD` budget, which spends
    /// `total_amount_micros` over its lifetime instead — the API treats the two
    /// as mutually exclusive.
    #[serde(default)]
    pub amount_micros: Option<i64>,
    #[serde(default)]
    pub total_amount_micros: Option<i64>,
    #[serde(default)]
    pub period: Option<String>,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    #[serde(default)]
    pub delivery_method: Option<String>,
    #[serde(default)]
    pub explicitly_shared: Option<bool>,
    /// Live-only, and never rendered: a budget has no declarable `status`, but
    /// Google auto-removes one along with the last campaign that used it, and a
    /// dead budget matching by name would shadow the declared one (issue #161).
    #[serde(default)]
    pub status: Option<String>,
}

impl JsonBudget {
    /// Whether this budget's amount is a lifetime cap rather than a daily rate.
    /// An absent period is the API's documented `DAILY` default.
    pub fn is_custom_period(&self) -> bool {
        self.period.as_deref() == Some(crate::schema::CUSTOM_PERIOD)
    }

    /// The amount the budget actually commits, in the unit its period implies.
    pub fn committed_micros(&self) -> i64 {
        if self.is_custom_period() {
            self.total_amount_micros.unwrap_or(0)
        } else {
            self.amount_micros.unwrap_or(0)
        }
    }
}

#[derive(Deserialize)]
pub struct JsonCampaign {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    pub advertising_channel_type: String,
    #[serde(default)]
    pub advertising_channel_sub_type: Option<String>,
    pub campaign_budget: String,
    #[serde(default)]
    pub contains_eu_political_advertising: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub end_date: Option<String>,
    #[serde(default)]
    pub final_url_suffix: Option<String>,
    #[serde(default)]
    pub custom_parameters: Option<Vec<JsonCustomParameter>>,
    #[serde(default)]
    pub manual_cpc: Option<JsonManualCpc>,
    #[serde(default)]
    pub manual_cpm: Option<JsonBidSelector>,
    #[serde(default)]
    pub manual_cpv: Option<JsonBidSelector>,
    #[serde(default)]
    pub target_cpm: Option<JsonBidSelector>,
    #[serde(default)]
    pub target_cpv: Option<JsonBidSelector>,
    #[serde(default)]
    pub target_impression_share: Option<JsonTargetImpressionShare>,
    #[serde(default)]
    pub target_spend: Option<JsonTargetSpend>,
    #[serde(default)]
    pub network_settings: Option<JsonNetworkSettings>,
    #[serde(default)]
    pub geo_target_type_setting: Option<JsonGeoTargetTypeSetting>,
    #[serde(default)]
    pub video_campaign_settings: Option<JsonVideoCampaignSettings>,
    #[serde(default)]
    pub asset_automation_settings: Option<JsonAssetAutomationSettings>,
    #[serde(default)]
    pub ai_max_setting: Option<JsonAiMaxSetting>,
    #[serde(default)]
    pub dynamic_search_ads_setting: Option<JsonDynamicSearchAdsSetting>,
    #[serde(default)]
    pub targeting_setting: Option<JsonTargetingSetting>,
    /// Repeated, and managed as a whole list: an empty list means "this
    /// campaign has no frequency caps", so caps set in the UI on a declared
    /// campaign read as drift rather than staying invisible.
    #[serde(default)]
    pub frequency_caps: Vec<JsonFrequencyCap>,
    /// Whether `owns` claims the assets Google's automation attached to this
    /// campaign. Declared-side only: nothing live records it, because the
    /// claim is over resources no file can declare.
    #[serde(default)]
    pub owns_automatic_assets: bool,
    /// `bidsmith:address=<addr>` label read off a live resource. None for
    /// declared resources (their address is `id`) and for unmanaged live ones.
    #[serde(default)]
    pub managed_address: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonManualCpc {
    #[serde(default)]
    pub enhanced_cpc_enabled: Option<bool>,
}

/// A bidding strategy the API models as an empty message (`ManualCpv`,
/// `TargetCpm`, …) — the block's presence is the whole setting.
#[derive(Deserialize)]
pub struct JsonBidSelector {}

#[derive(Deserialize, Default)]
pub struct JsonTargetImpressionShare {
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub location_fraction_micros: Option<i64>,
    #[serde(default)]
    pub cpc_bid_ceiling_micros: Option<i64>,
}

#[derive(Deserialize, Default)]
pub struct JsonTargetSpend {
    #[serde(default)]
    pub cpc_bid_ceiling_micros: Option<i64>,
}

impl JsonCampaign {
    /// Which `campaign_bidding_strategy` the campaign picks, as the block name
    /// the file uses and the API field the update mask names. `None` means the
    /// file left bidding unmanaged.
    pub fn bidding_strategy(&self) -> Option<&'static str> {
        if self.manual_cpc.is_some() {
            Some("manual_cpc")
        } else if self.manual_cpm.is_some() {
            Some("manual_cpm")
        } else if self.manual_cpv.is_some() {
            Some("manual_cpv")
        } else if self.target_cpm.is_some() {
            Some("target_cpm")
        } else if self.target_cpv.is_some() {
            Some("target_cpv")
        } else if self.target_impression_share.is_some() {
            Some("target_impression_share")
        } else if self.target_spend.is_some() {
            Some("target_spend")
        } else {
            None
        }
    }

    /// One `video_ad_inventory_control` field, or `None` when the file left it
    /// — or the whole block — unmanaged.
    pub fn video_ad_inventory(&self, field: &str) -> Option<bool> {
        self.video_campaign_settings
            .as_ref()?
            .video_ad_inventory_control
            .as_ref()?
            .get(field)
    }

    /// The campaign's asset automation as the API holds it: one
    /// `(automation type, status)` pair per opt-in the campaign carries, in the
    /// schema's order so two sides compare as sets rather than as orderings.
    pub fn asset_automation_list(&self) -> Vec<(&str, &str)> {
        let Some(s) = self.asset_automation_settings.as_ref() else {
            return Vec::new();
        };
        crate::schema::ASSET_AUTOMATION_FIELDS
            .iter()
            .filter_map(|(field, api)| s.get(field).map(|status| (*api, status)))
            .chain(
                s.unmodelled
                    .iter()
                    .map(|(api, status)| (api.as_str(), status.as_str())),
            )
            .collect()
    }
}

#[derive(Deserialize, Default)]
pub struct JsonNetworkSettings {
    #[serde(default)]
    pub target_google_search: Option<bool>,
    #[serde(default)]
    pub target_search_network: Option<bool>,
    #[serde(default)]
    pub target_content_network: Option<bool>,
    #[serde(default)]
    pub target_partner_search_network: Option<bool>,
    #[serde(default)]
    pub target_youtube: Option<bool>,
    #[serde(default)]
    pub target_google_tv_network: Option<bool>,
}

impl JsonNetworkSettings {
    pub fn get(&self, field: &str) -> Option<bool> {
        match field {
            "target_google_search" => self.target_google_search,
            "target_search_network" => self.target_search_network,
            "target_content_network" => self.target_content_network,
            "target_partner_search_network" => self.target_partner_search_network,
            "target_youtube" => self.target_youtube,
            "target_google_tv_network" => self.target_google_tv_network,
            _ => None,
        }
    }

    pub fn set(&mut self, field: &str, value: Option<bool>) {
        let slot = match field {
            "target_google_search" => &mut self.target_google_search,
            "target_search_network" => &mut self.target_search_network,
            "target_content_network" => &mut self.target_content_network,
            "target_partner_search_network" => &mut self.target_partner_search_network,
            "target_youtube" => &mut self.target_youtube,
            "target_google_tv_network" => &mut self.target_google_tv_network,
            _ => return,
        };
        *slot = value;
    }
}

/// `Campaign.video_campaign_settings`. Only the inventory control is modelled;
/// `video_ad_sequence` and `video_ad_format_control` are still unread, so the
/// block is deliberately a container rather than a settings bag — a later field
/// lands beside this one instead of changing what the existing lines mean.
#[derive(Deserialize, Default)]
pub struct JsonVideoCampaignSettings {
    #[serde(default)]
    pub video_ad_inventory_control: Option<JsonVideoAdInventoryControl>,
}

impl JsonVideoCampaignSettings {
    pub fn is_empty(&self) -> bool {
        self.video_ad_inventory_control
            .as_ref()
            .is_none_or(JsonVideoAdInventoryControl::is_empty)
    }
}

#[derive(Deserialize, Default)]
pub struct JsonVideoAdInventoryControl {
    #[serde(default)]
    pub allow_in_stream: Option<bool>,
    #[serde(default)]
    pub allow_in_feed: Option<bool>,
    #[serde(default)]
    pub allow_shorts: Option<bool>,
    #[serde(default)]
    pub allow_non_skippable_in_stream: Option<bool>,
}

impl JsonVideoAdInventoryControl {
    pub fn get(&self, field: &str) -> Option<bool> {
        match field {
            "allow_in_stream" => self.allow_in_stream,
            "allow_in_feed" => self.allow_in_feed,
            "allow_shorts" => self.allow_shorts,
            "allow_non_skippable_in_stream" => self.allow_non_skippable_in_stream,
            _ => None,
        }
    }

    pub fn set(&mut self, field: &str, value: Option<bool>) {
        let slot = match field {
            "allow_in_stream" => &mut self.allow_in_stream,
            "allow_in_feed" => &mut self.allow_in_feed,
            "allow_shorts" => &mut self.allow_shorts,
            "allow_non_skippable_in_stream" => &mut self.allow_non_skippable_in_stream,
            _ => return,
        };
        *slot = value;
    }

    pub fn is_empty(&self) -> bool {
        crate::schema::VIDEO_AD_INVENTORY_FIELDS
            .iter()
            .all(|(field, _)| self.get(field).is_none())
    }
}

/// `Campaign.asset_automation_settings`, flattened: the API's repeated
/// `(type, status)` list has one entry per type at most, so it reads as a
/// settings bag and a file can name only the automations it has an opinion on.
#[derive(Deserialize, Default)]
pub struct JsonAssetAutomationSettings {
    #[serde(default)]
    pub text_asset_automation: Option<String>,
    #[serde(default)]
    pub final_url_expansion_text_asset_automation: Option<String>,
    #[serde(default)]
    pub generate_image_enhancement: Option<String>,
    #[serde(default)]
    pub generate_image_extraction: Option<String>,
    #[serde(default)]
    pub generate_enhanced_youtube_videos: Option<String>,
    /// Live automations this build has no attribute for, by API type. The API
    /// replaces the whole list on write, so an automation bidsmith cannot name
    /// would revert to Google's default the moment a named one moves; carrying
    /// it through the write is what keeps a setting nobody touched where it is.
    #[serde(default)]
    pub unmodelled: BTreeMap<String, String>,
}

impl JsonAssetAutomationSettings {
    pub fn get(&self, field: &str) -> Option<&str> {
        match field {
            "text_asset_automation" => self.text_asset_automation.as_deref(),
            "final_url_expansion_text_asset_automation" => {
                self.final_url_expansion_text_asset_automation.as_deref()
            }
            "generate_image_enhancement" => self.generate_image_enhancement.as_deref(),
            "generate_image_extraction" => self.generate_image_extraction.as_deref(),
            "generate_enhanced_youtube_videos" => self.generate_enhanced_youtube_videos.as_deref(),
            _ => None,
        }
    }

    pub fn set(&mut self, field: &str, value: Option<String>) {
        let slot = match field {
            "text_asset_automation" => &mut self.text_asset_automation,
            "final_url_expansion_text_asset_automation" => {
                &mut self.final_url_expansion_text_asset_automation
            }
            "generate_image_enhancement" => &mut self.generate_image_enhancement,
            "generate_image_extraction" => &mut self.generate_image_extraction,
            "generate_enhanced_youtube_videos" => &mut self.generate_enhanced_youtube_videos,
            _ => return,
        };
        *slot = value;
    }

    /// Empty means "the file says nothing here". Carried-over live automations
    /// do not count: they are what the account already holds, not a claim.
    pub fn is_empty(&self) -> bool {
        crate::schema::ASSET_AUTOMATION_FIELDS
            .iter()
            .all(|(field, _)| self.get(field).is_none())
    }
}

/// `Campaign.ai_max_setting`. The read-only `bundling_required` is Google's
/// report of whether the campaign's AI Max features come as a set, not a switch
/// anyone can throw, so `enable_ai_max` is the whole message a `.bid` declares.
#[derive(Deserialize, Default)]
pub struct JsonAiMaxSetting {
    #[serde(default)]
    pub enable_ai_max: Option<bool>,
}

/// `AdGroup.ai_max_ad_group_setting`.
#[derive(Deserialize, Default)]
pub struct JsonAiMaxAdGroupSetting {
    #[serde(default)]
    pub disable_search_term_matching: Option<bool>,
}

/// `Campaign.dynamic_search_ads_setting` — the site Google may crawl to write
/// this campaign's ads, the language it reads it in, and whether it may pick
/// landing pages of its own or only the ones the file supplies.
#[derive(Deserialize, Default)]
pub struct JsonDynamicSearchAdsSetting {
    #[serde(default)]
    pub domain_name: Option<String>,
    #[serde(default)]
    pub language_code: Option<String>,
    #[serde(default)]
    pub use_supplied_urls_only: Option<bool>,
}

impl JsonDynamicSearchAdsSetting {
    /// The two identifiers as a reviewer reads them: `example.com (en)`, or
    /// `None` when the live campaign carries no setting worth naming.
    pub fn shown(&self) -> Option<String> {
        let domain = self.domain_name.as_deref()?;
        Some(match self.language_code.as_deref() {
            Some(lang) => format!("{domain} ({lang})"),
            None => domain.to_string(),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.domain_name.is_none()
            && self.language_code.is_none()
            && self.use_supplied_urls_only.is_none()
    }
}

#[derive(Deserialize, Default)]
pub struct JsonGeoTargetTypeSetting {
    #[serde(default)]
    pub positive_geo_target_type: Option<String>,
    #[serde(default)]
    pub negative_geo_target_type: Option<String>,
}

impl JsonGeoTargetTypeSetting {
    pub fn get(&self, field: &str) -> Option<&str> {
        match field {
            "positive_geo_target_type" => self.positive_geo_target_type.as_deref(),
            "negative_geo_target_type" => self.negative_geo_target_type.as_deref(),
            _ => None,
        }
    }

    pub fn set(&mut self, field: &str, value: Option<String>) {
        let slot = match field {
            "positive_geo_target_type" => &mut self.positive_geo_target_type,
            "negative_geo_target_type" => &mut self.negative_geo_target_type,
            _ => return,
        };
        *slot = value;
    }

    pub fn is_empty(&self) -> bool {
        self.positive_geo_target_type.is_none() && self.negative_geo_target_type.is_none()
    }
}

/// Whether each targeting dimension restricts who is eligible to see the ad
/// (`bid_only = false`) or merely informs bidding (`bid_only = true`). Carried
/// by both campaigns and ad groups, and written as one field: the API replaces
/// the whole list and removes whatever the body leaves out.
#[derive(Deserialize, Default, Clone)]
pub struct JsonTargetingSetting {
    #[serde(default)]
    pub target_restrictions: Vec<JsonTargetRestriction>,
}

#[derive(Deserialize, Clone, PartialEq, Eq)]
pub struct JsonTargetRestriction {
    pub targeting_dimension: String,
    pub bid_only: bool,
}

impl JsonTargetingSetting {
    /// The restrictions that say something an absent entry would not, sorted by
    /// dimension. Google fills in defaults nobody asked for, and the order of
    /// the list is not a setting, so this is the comparable form.
    pub fn effective(&self) -> Vec<(&str, bool)> {
        let mut out: Vec<(&str, bool)> = self
            .target_restrictions
            .iter()
            .filter(|r| r.bid_only != crate::schema::DEFAULT_BID_ONLY)
            .map(|r| (r.targeting_dimension.as_str(), r.bid_only))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// One restriction as a reviewer reads it, so a plan row says what a dimension
/// becomes rather than merely that it moves.
pub fn shown_target_restriction(dimension: &str, bid_only: bool) -> String {
    let how = if bid_only { "observation" } else { "targeting" };
    format!("{dimension} {how}")
}

#[derive(Deserialize, Clone, PartialEq, Eq)]
pub struct JsonFrequencyCap {
    pub event_type: String,
    pub time_unit: String,
    pub time_length: i64,
    pub cap: i64,
    #[serde(default)]
    pub level: Option<String>,
}

impl JsonFrequencyCap {
    pub fn level_or_default(&self) -> &str {
        self.level
            .as_deref()
            .unwrap_or(crate::schema::DEFAULT_FREQUENCY_CAP_LEVEL)
    }

    /// Order-insensitive identity: two campaigns declaring the same caps in a
    /// different order are the same campaign.
    pub fn sort_key(&self) -> (&str, &str, &str, i64, i64) {
        (
            self.level_or_default(),
            self.event_type.as_str(),
            self.time_unit.as_str(),
            self.time_length,
            self.cap,
        )
    }
}

#[derive(Default, Deserialize)]
pub struct JsonAdGroup {
    pub id: String,
    pub name: String,
    pub campaign: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    #[serde(default)]
    pub cpc_bid_micros: Option<i64>,
    #[serde(default)]
    pub cpv_bid_micros: Option<i64>,
    #[serde(default)]
    pub cpm_bid_micros: Option<i64>,
    #[serde(default)]
    pub target_cpa_micros: Option<i64>,
    #[serde(default)]
    pub target_cpm_micros: Option<i64>,
    #[serde(default)]
    pub target_cpv_micros: Option<i64>,
    #[serde(default)]
    pub percent_cpc_bid_micros: Option<i64>,
    #[serde(default)]
    pub fixed_cpm_micros: Option<i64>,
    #[serde(default)]
    pub final_url_suffix: Option<String>,
    #[serde(default)]
    pub custom_parameters: Option<Vec<JsonCustomParameter>>,
    #[serde(default)]
    pub targeting_setting: Option<JsonTargetingSetting>,
    #[serde(default)]
    pub ai_max_ad_group_setting: Option<JsonAiMaxAdGroupSetting>,
    #[serde(default)]
    pub managed_address: Option<String>,
}

impl JsonAdGroup {
    pub fn bid(&self, field: &str) -> Option<i64> {
        match field {
            "cpc_bid_micros" => self.cpc_bid_micros,
            "cpv_bid_micros" => self.cpv_bid_micros,
            "cpm_bid_micros" => self.cpm_bid_micros,
            "target_cpa_micros" => self.target_cpa_micros,
            "target_cpm_micros" => self.target_cpm_micros,
            "target_cpv_micros" => self.target_cpv_micros,
            "percent_cpc_bid_micros" => self.percent_cpc_bid_micros,
            "fixed_cpm_micros" => self.fixed_cpm_micros,
            _ => None,
        }
    }

    pub fn set_bid(&mut self, field: &str, value: Option<i64>) {
        let slot = match field {
            "cpc_bid_micros" => &mut self.cpc_bid_micros,
            "cpv_bid_micros" => &mut self.cpv_bid_micros,
            "cpm_bid_micros" => &mut self.cpm_bid_micros,
            "target_cpa_micros" => &mut self.target_cpa_micros,
            "target_cpm_micros" => &mut self.target_cpm_micros,
            "target_cpv_micros" => &mut self.target_cpv_micros,
            "percent_cpc_bid_micros" => &mut self.percent_cpc_bid_micros,
            "fixed_cpm_micros" => &mut self.fixed_cpm_micros,
            _ => return,
        };
        *slot = value;
    }
}

#[derive(Deserialize)]
pub struct JsonAdGroupAd {
    #[allow(dead_code)]
    pub id: String,
    pub ad_group: String,
    #[serde(default)]
    pub status: Option<String>,
    pub ad: JsonAd,
    #[serde(default)]
    pub managed_address: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonAd {
    #[serde(default)]
    pub name: Option<String>,
    pub final_urls: Vec<String>,
    #[serde(default)]
    pub final_mobile_urls: Vec<String>,
    #[serde(default)]
    pub display_url: Option<String>,
    #[serde(default)]
    pub final_url_suffix: Option<String>,
    #[serde(default)]
    pub custom_parameters: Option<Vec<JsonCustomParameter>>,
    #[serde(default)]
    pub responsive_search_ad: Option<JsonResponsiveSearchAd>,
    #[serde(default)]
    pub video_responsive_ad: Option<JsonVideoResponsiveAd>,
    #[serde(default)]
    pub video_ad: Option<JsonVideoAd>,
    #[serde(default)]
    pub demand_gen_video_responsive_ad: Option<JsonDemandGenVideoResponsiveAd>,
}

/// One `url_custom_parameters` entry. Kept sorted by name wherever it is built
/// so a map — which has no order — diffs deterministically against live state.
#[derive(Deserialize, Clone, PartialEq, Debug)]
pub struct JsonCustomParameter {
    pub key: String,
    pub value: String,
}

/// A YouTube video ad body. `video` is the address of a
/// `google_ads_youtube_video_asset` for declared state, and the live asset id
/// when read back off the account.
#[derive(Deserialize)]
pub struct JsonVideoResponsiveAd {
    pub video: String,
    #[serde(default)]
    pub headlines: Vec<String>,
    #[serde(default)]
    pub long_headlines: Vec<String>,
    #[serde(default)]
    pub descriptions: Vec<String>,
    #[serde(default)]
    pub call_to_actions: Vec<String>,
    #[serde(default)]
    pub breadcrumb1: Option<String>,
    #[serde(default)]
    pub breadcrumb2: Option<String>,
}

/// A plain `Ad.video_ad` creative — the shape a UI-built VIDEO campaign
/// carries. Adopt-only, so the only thing worth holding is which video it
/// plays; the format oneof (in-stream / bumper / in-feed) is not writable.
#[derive(Deserialize)]
pub struct JsonVideoAd {
    pub video: String,
}

/// A Demand Gen video responsive ad body (the ad type a DEMAND_GEN campaign
/// carries). `videos` holds asset ids of `google_ads_youtube_video_asset`s,
/// resolved to their addresses at render time.
#[derive(Deserialize)]
pub struct JsonDemandGenVideoResponsiveAd {
    #[serde(default)]
    pub videos: Vec<String>,
    #[serde(default)]
    pub headlines: Vec<String>,
    #[serde(default)]
    pub long_headlines: Vec<String>,
    #[serde(default)]
    pub descriptions: Vec<String>,
    /// Live Demand Gen CTAs are `AdCallToActionAsset` refs, not text, so this
    /// list only ever carries hand-authored values — it never round-trips.
    #[serde(default)]
    pub call_to_actions: Vec<String>,
    #[serde(default)]
    pub breadcrumb1: Option<String>,
    #[serde(default)]
    pub breadcrumb2: Option<String>,
    #[serde(default)]
    pub business_name: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonYoutubeVideoAsset {
    pub id: String,
    pub youtube_video_id: String,
    #[serde(default)]
    pub youtube_video_title: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonAdGroupCriterion {
    #[allow(dead_code)]
    pub id: String,
    pub ad_group: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub negative: Option<bool>,
    #[serde(default)]
    pub cpc_bid_micros: Option<i64>,
    #[serde(default)]
    pub bid_modifier: Option<f64>,
    #[serde(flatten)]
    pub target: JsonCriterion,
    #[serde(default)]
    pub managed_address: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonCampaignCriterion {
    pub id: String,
    pub campaign: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub negative: Option<bool>,
    #[serde(default)]
    pub bid_modifier: Option<f64>,
    #[serde(flatten)]
    pub target: JsonCriterion,
}

/// What a criterion targets — the API models this as a `oneof`, so exactly one
/// of these is ever set. Shared by ad-group and campaign criteria: the two
/// resources differ in which of the axes they accept (the schema is what says
/// so), not in how an axis is keyed, rendered, or mutated.
#[derive(Deserialize, Default)]
pub struct JsonCriterion {
    #[serde(default)]
    pub keyword: Option<JsonKeyword>,
    #[serde(default)]
    pub location: Option<JsonLocation>,
    #[serde(default)]
    pub language: Option<JsonLanguage>,
    #[serde(default)]
    pub proximity: Option<JsonProximity>,
    #[serde(default)]
    pub device: Option<JsonDevice>,
    #[serde(default)]
    pub youtube_channel: Option<JsonYoutubeChannel>,
    #[serde(default)]
    pub youtube_video: Option<JsonYoutubeVideo>,
    #[serde(default)]
    pub topic: Option<JsonTopic>,
    #[serde(default)]
    pub placement: Option<JsonPlacement>,
    #[serde(default)]
    pub user_interest: Option<JsonUserInterest>,
    #[serde(default)]
    pub age_range: Option<JsonAgeRange>,
    #[serde(default)]
    pub gender: Option<JsonGender>,
    #[serde(default)]
    pub parental_status: Option<JsonParentalStatus>,
    #[serde(default)]
    pub income_range: Option<JsonIncomeRange>,
    #[serde(default)]
    pub audience: Option<JsonAudience>,
}

impl JsonCriterion {
    /// True when no axis is set — a criterion resource that targets nothing.
    pub fn is_unset(&self) -> bool {
        self.keyword.is_none()
            && self.location.is_none()
            && self.language.is_none()
            && self.proximity.is_none()
            && self.device.is_none()
            && self.youtube_channel.is_none()
            && self.youtube_video.is_none()
            && self.topic.is_none()
            && self.placement.is_none()
            && self.user_interest.is_none()
            && self.age_range.is_none()
            && self.gender.is_none()
            && self.parental_status.is_none()
            && self.income_range.is_none()
            && self.audience.is_none()
    }
}

#[derive(Deserialize)]
pub struct JsonYoutubeChannel {
    pub channel_id: String,
}

#[derive(Deserialize)]
pub struct JsonYoutubeVideo {
    pub video_id: String,
}

#[derive(Deserialize)]
pub struct JsonTopic {
    pub topic_constant: String,
}

#[derive(Deserialize)]
pub struct JsonUserInterest {
    pub user_interest_category: String,
}

#[derive(Deserialize)]
pub struct JsonAgeRange {
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Deserialize)]
pub struct JsonGender {
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Deserialize)]
pub struct JsonParentalStatus {
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Deserialize)]
pub struct JsonIncomeRange {
    #[serde(rename = "type")]
    pub ty: String,
}

/// A managed placement — one site, app, or YouTube URL an ad may run on.
#[derive(Deserialize)]
pub struct JsonPlacement {
    pub url: String,
}

/// Exactly one field is ever set — three distinct API criterion messages that
/// all answer "which audience?". `custom_audience` holds a declared
/// `google_ads_custom_audience` address or a live resource name; the other two
/// are always resource names (bidsmith has no resource that builds them).
#[derive(Deserialize)]
pub struct JsonAudience {
    #[serde(default)]
    pub custom_audience: Option<String>,
    #[serde(default)]
    pub user_list: Option<String>,
    #[serde(default)]
    pub combined_audience: Option<String>,
}

impl JsonAudience {
    pub fn source(&self) -> Option<(&'static str, &str)> {
        if let Some(v) = &self.custom_audience {
            return Some(("custom_audience", v));
        }
        if let Some(v) = &self.user_list {
            return Some(("user_list", v));
        }
        self.combined_audience
            .as_deref()
            .map(|v| ("combined_audience", v))
    }
}

#[derive(Deserialize, Clone)]
pub struct JsonKeyword {
    pub text: String,
    pub match_type: String,
}

#[derive(Deserialize)]
pub struct JsonLocation {
    pub geo_target_constant: String,
}

#[derive(Deserialize)]
pub struct JsonLanguage {
    pub language_constant: String,
}

#[derive(Deserialize)]
pub struct JsonProximity {
    pub latitude: f64,
    pub longitude: f64,
    pub radius: f64,
    pub radius_units: String,
}

#[derive(Deserialize)]
pub struct JsonDevice {
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Deserialize)]
pub struct JsonConversionAction {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub category: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub counting_type: Option<String>,
    #[serde(default)]
    pub click_through_lookback_window_days: Option<i64>,
    #[serde(default)]
    pub view_through_lookback_window_days: Option<i64>,
    #[serde(default)]
    pub value_settings: Option<JsonValueSettings>,
}

#[derive(Deserialize)]
pub struct JsonValueSettings {
    #[serde(default)]
    pub default_value: Option<f64>,
    #[serde(default)]
    pub default_currency_code: Option<String>,
    #[serde(default)]
    pub always_use_default_value: Option<bool>,
}

#[derive(Deserialize)]
pub struct JsonCallAsset {
    pub id: String,
    pub country_code: String,
    pub phone_number: String,
    #[serde(default)]
    pub call_conversion_reporting_state: Option<String>,
    #[serde(default)]
    pub call_conversion_action: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonSitelinkAsset {
    pub id: String,
    pub link_text: String,
    #[serde(default)]
    pub description1: Option<String>,
    #[serde(default)]
    pub description2: Option<String>,
    #[serde(default)]
    pub final_urls: Vec<String>,
}

#[derive(Deserialize)]
pub struct JsonCalloutAsset {
    pub id: String,
    pub text: String,
}

#[derive(Deserialize)]
pub struct JsonStructuredSnippetAsset {
    pub id: String,
    pub header: String,
    #[serde(default)]
    pub values: Vec<String>,
}

/// `AssetSource` for a link Google's automation attached rather than a person.
/// `source` is output-only on every link resource, so bidsmith can never create
/// one — which is why the renderers leave them out of the declared set, and why
/// the only lever over them is the `status` of the link that carries them.
pub const AUTOMATICALLY_CREATED: &str = "AUTOMATICALLY_CREATED";

#[derive(Deserialize)]
pub struct JsonCustomerAsset {
    pub id: String,
    pub asset: String,
    pub field_type: String,
    /// `ADVERTISER` | `AUTOMATICALLY_CREATED`. Live-only — who attached the
    /// asset, which decides whether prune may detach it.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonCampaignAsset {
    pub id: String,
    pub campaign: String,
    pub asset: String,
    pub field_type: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonAdGroupAsset {
    pub id: String,
    pub ad_group: String,
    pub asset: String,
    pub field_type: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonSharedSet {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub negative_keywords: Vec<JsonKeyword>,
}

#[derive(Deserialize)]
pub struct JsonSharedCriterion {
    pub id: String,
    pub shared_set: String,
    pub keyword: JsonKeyword,
}

#[derive(Deserialize)]
pub struct JsonCustomAudience {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "type")]
    pub ty: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Managed as a whole set: the API replaces the repeated field wholesale.
    #[serde(default)]
    pub members: Vec<JsonCustomAudienceMember>,
}

#[derive(Deserialize, Clone, PartialEq, Eq)]
pub struct JsonCustomAudienceMember {
    #[serde(default)]
    pub keyword: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub place_category: Option<String>,
    #[serde(default)]
    pub app: Option<String>,
}

impl JsonCustomAudienceMember {
    /// The set attribute as (bidsmith name, API `member_type`, value).
    pub fn payload(&self) -> Option<(&'static str, &'static str, &str)> {
        if let Some(v) = &self.keyword {
            return Some(("keyword", "KEYWORD", v));
        }
        if let Some(v) = &self.url {
            return Some(("url", "URL", v));
        }
        if let Some(v) = &self.place_category {
            return Some(("place_category", "PLACE_CATEGORY", v));
        }
        self.app.as_deref().map(|v| ("app", "APP", v))
    }
}

#[derive(Deserialize)]
pub struct JsonCampaignSharedSet {
    pub id: String,
    pub campaign: String,
    pub shared_set: String,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct JsonResponsiveSearchAd {
    pub headlines: Vec<JsonRsaAsset>,
    pub descriptions: Vec<JsonRsaAsset>,
    #[serde(default)]
    pub path1: Option<String>,
    #[serde(default)]
    pub path2: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct JsonRsaAsset {
    pub text: String,
    #[serde(default)]
    pub pin: Option<String>,
}

impl ExportInput {
    /// Fill omitted optional attributes that carry a schema default with that
    /// default, so "omitted" means "managed at the default" for `plan` and the
    /// mutate builder. Applied to *both* the declared and the live state before
    /// diffing — never on the render path, where defaults are stripped instead.
    pub fn apply_schema_defaults(&mut self) {
        use crate::schema::{
            DEFAULT_BUDGET_PERIOD, DEFAULT_CUSTOM_AUDIENCE_TYPE, DEFAULT_DELIVERY_METHOD,
            DEFAULT_EU_POLITICAL, DEFAULT_EXPLICITLY_SHARED, DEFAULT_NEGATIVE, DEFAULT_STATUS,
        };
        let status = || DEFAULT_STATUS.to_string();

        for b in &mut self.campaign_budgets {
            b.delivery_method
                .get_or_insert_with(|| DEFAULT_DELIVERY_METHOD.to_string());
            b.explicitly_shared.get_or_insert(DEFAULT_EXPLICITLY_SHARED);
            b.period.get_or_insert_with(|| DEFAULT_BUDGET_PERIOD.to_string());
        }
        for c in &mut self.campaigns {
            c.status.get_or_insert_with(status);
            c.contains_eu_political_advertising
                .get_or_insert_with(|| DEFAULT_EU_POLITICAL.to_string());
        }
        for g in &mut self.ad_groups {
            g.status.get_or_insert_with(status);
        }
        for a in &mut self.ad_group_ads {
            a.status.get_or_insert_with(status);
        }
        for c in &mut self.ad_group_criteria {
            c.status.get_or_insert_with(status);
            c.negative.get_or_insert(DEFAULT_NEGATIVE);
        }
        for c in &mut self.campaign_criteria {
            c.status.get_or_insert_with(status);
            c.negative.get_or_insert(DEFAULT_NEGATIVE);
        }
        for c in &mut self.conversion_actions {
            c.status.get_or_insert_with(status);
        }
        for a in &mut self.customer_assets {
            a.status.get_or_insert_with(status);
        }
        for a in &mut self.campaign_assets {
            a.status.get_or_insert_with(status);
        }
        for a in &mut self.ad_group_assets {
            a.status.get_or_insert_with(status);
        }
        for s in &mut self.shared_sets {
            s.status.get_or_insert_with(status);
        }
        for a in &mut self.custom_audiences {
            a.status.get_or_insert_with(status);
            a.ty.get_or_insert_with(|| DEFAULT_CUSTOM_AUDIENCE_TYPE.to_string());
        }
        for s in &mut self.campaign_shared_sets {
            s.status.get_or_insert_with(status);
        }
    }
}

pub fn run(
    from_json: Option<&str>,
    from_gads_search_response: Option<&str>,
    output: Option<&str>,
    include_removed: bool,
    login_customer_id_override: Option<&str>,
    customer_id_override: Option<&str>,
) -> ExitCode {
    let mut input = match (from_json, from_gads_search_response) {
        (Some(path), None) => match load_flat_json(path) {
            Ok(v) => v,
            Err(code) => return code,
        },
        (None, Some(path)) => match load_gads_search_response(path) {
            Ok(v) => v,
            Err(code) => return code,
        },
        (None, None) => {
            eprintln!(
                "export: provide --from-json <PATH> or --from-gads-search-response <PATH>"
            );
            return ExitCode::from(2);
        }
        (Some(_), Some(_)) => {
            eprintln!("export: --from-json and --from-gads-search-response are mutually exclusive");
            return ExitCode::from(2);
        }
    };

    let customer_id_resolved = customer_id_override
        .map(str::to_string)
        .or_else(|| std::env::var("GOOGLE_ADS_CUSTOMER_ID").ok())
        .filter(|s| !s.is_empty());
    if let Some(id) = customer_id_resolved {
        input.customer_id = id;
    }
    let login_customer_id_resolved = login_customer_id_override
        .map(str::to_string)
        .or_else(|| std::env::var("GOOGLE_ADS_LOGIN_CUSTOMER_ID").ok())
        .filter(|s| !s.is_empty());
    if let Some(id) = login_customer_id_resolved {
        input.login_customer_id = Some(id);
    }
    if !include_removed {
        filter_removed(&mut input);
    }
    report_orphans("export", prune_orphans(&mut input));

    let rendered = canonicalize(&render(&input));

    match output {
        None => {
            print!("{rendered}");
            ExitCode::SUCCESS
        }
        Some(path) => match std::fs::write(path, &rendered) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("failed to write {path}: {e}");
                ExitCode::from(1)
            }
        },
    }
}

pub fn canonicalize(raw: &str) -> String {
    match raw.parse::<hcl_edit::structure::Body>() {
        Ok(body) => crate::commands::fmt::format_body_minimal(&body),
        Err(_) => raw.to_string(),
    }
}

pub fn filter_removed(input: &mut ExportInput) {
    let is_removed = |s: &Option<String>| s.as_deref() == Some("REMOVED");
    input.campaigns.retain(|c| !is_removed(&c.status));
    input.ad_groups.retain(|g| !is_removed(&g.status));
    input.ad_group_ads.retain(|a| !is_removed(&a.status));
    input.ad_group_criteria.retain(|c| !is_removed(&c.status));
    input.campaign_criteria.retain(|c| !is_removed(&c.status));
    input
        .conversion_actions
        .retain(|c| !is_removed(&c.status));
    input.customer_assets.retain(|a| !is_removed(&a.status));
    input.campaign_assets.retain(|a| !is_removed(&a.status));
    input.ad_group_assets.retain(|a| !is_removed(&a.status));
    input.shared_sets.retain(|s| !is_removed(&s.status));
    input.custom_audiences.retain(|a| !is_removed(&a.status));
    input
        .campaign_shared_sets
        .retain(|s| !is_removed(&s.status));
}

/// Asset links the renderer has no block for. The live queries ask for every
/// `field_type` so that `plan` can see what is attached to a campaign, but a
/// file can only describe the kinds bidsmith models an asset resource for —
/// rendering the rest would emit a reference to an asset the snapshot never
/// described. Links Google's automation attached go quietly: `source` is
/// output-only, so no `.bid` could declare one whatever its field type.
fn prune_unrenderable_links(input: &mut ExportInput) -> Vec<String> {
    let mut dropped: Vec<String> = Vec::new();
    let known: HashSet<String> = input
        .call_assets
        .iter()
        .map(|a| a.id.clone())
        .chain(input.sitelink_assets.iter().map(|a| a.id.clone()))
        .chain(input.callout_assets.iter().map(|a| a.id.clone()))
        .chain(
            input
                .structured_snippet_assets
                .iter()
                .map(|a| a.id.clone()),
        )
        .chain(input.youtube_video_assets.iter().map(|a| a.id.clone()))
        .collect();
    let mut keep = |kind: &str, id: &str, asset: &str, field_type: &str, source: &Option<String>| {
        if source.as_deref() == Some(AUTOMATICALLY_CREATED) {
            return false;
        }
        if known.contains(asset) {
            return true;
        }
        dropped.push(format!("{kind} {id} ({field_type} asset {asset} not in snapshot)"));
        false
    };
    input
        .customer_assets
        .retain(|a| keep("customer_asset", &a.id, &a.asset, &a.field_type, &a.source));
    input
        .campaign_assets
        .retain(|a| keep("campaign_asset", &a.id, &a.asset, &a.field_type, &a.source));
    input
        .ad_group_assets
        .retain(|a| keep("ad_group_asset", &a.id, &a.asset, &a.field_type, &a.source));
    dropped
}

/// The API can return children (ad groups, criteria) whose parent campaign was
/// filtered out, which would render as a dangling reference; drop them instead.
pub fn prune_orphans(input: &mut ExportInput) -> Vec<String> {
    let mut dropped: Vec<String> = Vec::new();
    dropped.extend(prune_unrenderable_links(input));

    let campaign_ids: HashSet<String> = input.campaigns.iter().map(|c| c.id.clone()).collect();
    input.ad_groups.retain(|g| {
        let keep = campaign_ids.contains(&g.campaign);
        if !keep {
            dropped.push(format!(
                "ad_group {} (campaign {} not in snapshot)",
                g.id, g.campaign
            ));
        }
        keep
    });
    input.campaign_criteria.retain(|c| {
        let keep = campaign_ids.contains(&c.campaign);
        if !keep {
            dropped.push(format!(
                "campaign_criterion {} (campaign {} not in snapshot)",
                c.id, c.campaign
            ));
        }
        keep
    });
    input.campaign_shared_sets.retain(|s| {
        let keep = campaign_ids.contains(&s.campaign);
        if !keep {
            dropped.push(format!(
                "campaign_shared_set {} (campaign {} not in snapshot)",
                s.id, s.campaign
            ));
        }
        keep
    });
    input.campaign_assets.retain(|a| {
        let keep = a.campaign.starts_with("customers/") || campaign_ids.contains(&a.campaign);
        if !keep {
            dropped.push(format!(
                "campaign_asset {} (campaign {} not in snapshot)",
                a.id, a.campaign
            ));
        }
        keep
    });

    let ad_group_ids: HashSet<String> = input.ad_groups.iter().map(|g| g.id.clone()).collect();
    input.ad_group_assets.retain(|a| {
        let keep = a.ad_group.starts_with("customers/") || ad_group_ids.contains(&a.ad_group);
        if !keep {
            dropped.push(format!(
                "ad_group_asset {} (ad_group {} not in snapshot)",
                a.id, a.ad_group
            ));
        }
        keep
    });
    input.ad_group_ads.retain(|a| {
        let keep = ad_group_ids.contains(&a.ad_group);
        if !keep {
            dropped.push(format!(
                "ad_group_ad {} (ad_group {} not in snapshot)",
                a.id, a.ad_group
            ));
        }
        keep
    });
    input.ad_group_criteria.retain(|c| {
        let keep = ad_group_ids.contains(&c.ad_group);
        if !keep {
            dropped.push(format!(
                "ad_group_criterion {} (ad_group {} not in snapshot)",
                c.id, c.ad_group
            ));
        }
        keep
    });

    dropped
}

pub fn report_orphans(command: &str, orphans: Vec<String>) {
    if orphans.is_empty() {
        return;
    }
    eprintln!(
        "{command}: dropped {} resource(s) referencing a parent not in the account snapshot:",
        orphans.len()
    );
    for o in &orphans {
        eprintln!("  - {o}");
    }
}

/// A CLI-facing notice naming the one thing bidsmith cannot do for YouTube
/// video advertising: put the video file on YouTube. Everything downstream of
/// that — the video asset, the creative that references it — is created and
/// updated by `apply` like any other resource.
///
/// Returns `None` when the desired state references no video. Surfaced by
/// `plan`/`apply` (which run `validate_files` but not the warning-level lints,
/// so this is how the boundary reaches those verbs).
pub fn video_upload_notice(input: &ExportInput) -> Option<String> {
    let video_ads = input
        .ad_group_ads
        .iter()
        .filter(|a| {
            a.ad.video_responsive_ad.is_some()
                || a.ad.video_ad.is_some()
                || a.ad.demand_gen_video_responsive_ad.is_some()
        })
        .count();
    let video_assets = input.youtube_video_assets.len();

    if video_ads == 0 && video_assets == 0 {
        return None;
    }

    let mut msg = String::from(
        "note: bidsmith creates the video asset and the video ad creative, but it cannot upload\n",
    );
    msg.push_str(
        "  video files. Every google_ads_youtube_video_asset must name a video that is already\n",
    );
    msg.push_str(
        "  published on your YouTube channel (upload via YouTube Studio or the YouTube Data API)\n",
    );
    msg.push_str("  and visible to the Google Ads account.\n");
    msg.push_str(&format!(
        "  found: {video_ads} video ad(s), {video_assets} youtube video asset(s).",
    ));
    Some(msg)
}

fn load_flat_json(path: &str) -> Result<ExportInput, ExitCode> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("failed to read {path}: {e}");
        ExitCode::from(1)
    })?;
    serde_json::from_str(&raw).map_err(|e| {
        eprintln!("failed to parse {path}: {e}");
        ExitCode::from(1)
    })
}

fn load_gads_search_response(path: &str) -> Result<ExportInput, ExitCode> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        eprintln!("failed to read {path}: {e}");
        ExitCode::from(1)
    })?;
    crate::commands::adapt::from_search_response(&raw).map_err(|e| {
        eprintln!("failed to adapt {path}: {e}");
        ExitCode::from(1)
    })
}

#[allow(clippy::too_many_arguments)]
fn write_account_assets(
    out: &mut String,
    input: &ExportInput,
    names: &mut NameAllocator,
    conversion_action_addr: &mut HashMap<String, String>,
    asset_addr: &mut HashMap<String, String>,
    youtube_asset_addr: &mut HashMap<String, String>,
    inline_assets: &InlineAssets,
) {
    for v in &input.youtube_video_assets {
        let base = v
            .youtube_video_title
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(slugify)
            .unwrap_or_else(|| format!("video_{}", slugify(&v.youtube_video_id)));
        let name = names.allocate("google_ads_youtube_video_asset", &base);
        youtube_asset_addr.insert(
            v.id.clone(),
            format!("google_ads_youtube_video_asset.{name}"),
        );
        write_youtube_video_asset(out, &name, v);
    }
    for c in &input.conversion_actions {
        let name = names.allocate("google_ads_conversion_action", &slugify(&c.name));
        conversion_action_addr.insert(c.id.clone(), format!("google_ads_conversion_action.{name}"));
        write_conversion_action(out, &name, c);
    }
    for a in &input.call_assets {
        let base = format!("call_{}_{}", a.country_code, a.phone_number);
        let name = names.allocate("google_ads_call_asset", &slugify(&base));
        asset_addr.insert(a.id.clone(), format!("google_ads_call_asset.{name}"));
        write_call_asset(out, &name, a, conversion_action_addr);
    }
    for a in &input.sitelink_assets {
        let base = format!("sitelink_{}", slugify(&a.link_text));
        let name = names.allocate("google_ads_sitelink_asset", &base);
        asset_addr.insert(a.id.clone(), format!("google_ads_sitelink_asset.{name}"));
        write_sitelink_asset(out, &name, a);
    }
    for a in &input.callout_assets {
        if inline_assets.folded_assets.contains(&a.id) {
            continue;
        }
        let base = format!("callout_{}", slugify(&a.text));
        let name = names.allocate("google_ads_callout_asset", &base);
        asset_addr.insert(a.id.clone(), format!("google_ads_callout_asset.{name}"));
        write_callout_asset(out, &name, a);
    }
    for a in &input.structured_snippet_assets {
        if inline_assets.folded_assets.contains(&a.id) {
            continue;
        }
        let base = format!("snippet_{}", slugify(&a.header));
        let name = names.allocate("google_ads_structured_snippet_asset", &base);
        asset_addr.insert(
            a.id.clone(),
            format!("google_ads_structured_snippet_asset.{name}"),
        );
        write_structured_snippet_asset(out, &name, a);
    }
    for a in &input.customer_assets {
        let base = asset_addr
            .get(&a.asset)
            .and_then(|addr| addr.rsplit('.').next())
            .map(|s| format!("link_{s}"))
            .unwrap_or_else(|| slugify(&a.id));
        let name = names.allocate("google_ads_customer_asset", &slugify(&base));
        write_customer_asset(out, &name, a, asset_addr);
    }
}

#[allow(clippy::too_many_arguments)]
fn write_campaign_assets(
    out: &mut String,
    input: &ExportInput,
    names: &mut NameAllocator,
    campaign_addr: &HashMap<String, String>,
    ad_group_addr: &HashMap<String, String>,
    asset_addr: &HashMap<String, String>,
    inline_assets: &InlineAssets,
) {
    for a in &input.campaign_assets {
        if inline_assets.folded_links.contains(&a.id) {
            continue;
        }
        let asset_local = asset_addr.get(&a.asset).and_then(|addr| addr.rsplit('.').next());
        let camp_local = campaign_addr
            .get(&a.campaign)
            .and_then(|addr| addr.rsplit('.').next());
        let base = match (camp_local, asset_local) {
            (Some(c), Some(s)) => format!("{c}_{s}"),
            _ => slugify(&a.id),
        };
        let name = names.allocate("google_ads_campaign_asset", &slugify(&base));
        write_campaign_asset(out, &name, a, campaign_addr, asset_addr);
    }
    for a in &input.ad_group_assets {
        if inline_assets.folded_links.contains(&a.id) {
            continue;
        }
        let asset_local = asset_addr.get(&a.asset).and_then(|addr| addr.rsplit('.').next());
        let ag_local = ad_group_addr
            .get(&a.ad_group)
            .and_then(|addr| addr.rsplit('.').next());
        let base = match (ag_local, asset_local) {
            (Some(g), Some(s)) => format!("{g}_{s}"),
            _ => slugify(&a.id),
        };
        let name = names.allocate("google_ads_ad_group_asset", &slugify(&base));
        write_ad_group_asset(out, &name, a, ad_group_addr, asset_addr);
    }
}

#[allow(clippy::too_many_arguments)]
fn write_custom_audiences(
    out: &mut String,
    input: &ExportInput,
    names: &mut NameAllocator,
    custom_audience_addr: &mut HashMap<String, String>,
) {
    for a in &input.custom_audiences {
        let name = names.allocate("google_ads_custom_audience", &slugify(&a.name));
        custom_audience_addr.insert(a.id.clone(), format!("google_ads_custom_audience.{name}"));
        write_custom_audience(out, &name, a);
    }
}

#[allow(clippy::too_many_arguments)]
fn write_campaign_tree(
    out: &mut String,
    input: &ExportInput,
    names: &mut NameAllocator,
    inline: &InlineTargeting,
    inline_assets: &InlineAssets,
    plan: Option<&FoldPlan>,
    budget_addr: &mut HashMap<String, String>,
    campaign_addr: &mut HashMap<String, String>,
    ad_group_addr: &mut HashMap<String, String>,
    youtube_asset_addr: &HashMap<String, String>,
    custom_audience_addr: &HashMap<String, String>,
) {
    for b in &input.campaign_budgets {
        let name = names.allocate("google_ads_campaign_budget", &slugify(&b.name));
        budget_addr.insert(b.id.clone(), format!("google_ads_campaign_budget.{name}"));
        write_budget(out, &name, b);
    }
    for c in &input.campaigns {
        let name = names.allocate("google_ads_campaign", &slugify(&c.name));
        campaign_addr.insert(c.id.clone(), format!("google_ads_campaign.{name}"));
        write_campaign(
            out,
            &name,
            c,
            budget_addr,
            inline.languages_for(&c.id),
            inline.locations_for(&c.id),
            inline.devices_for(&c.id),
            inline_assets.callouts_for(&c.id),
            inline_assets.snippets_for(&c.id),
        );
    }
    for g in &input.ad_groups {
        let name = names.allocate("google_ads_ad_group", &slugify(&g.name));
        ad_group_addr.insert(g.id.clone(), format!("google_ads_ad_group.{name}"));
        write_ad_group(
            out,
            &name,
            g,
            campaign_addr,
            inline_assets.callouts_for(&g.id),
            inline_assets.snippets_for(&g.id),
        );
    }
    for (i, a) in input.ad_group_ads.iter().enumerate() {
        let base = ad_ad_base(a, ad_group_addr);
        let name = names.allocate("google_ads_ad_group_ad", &base);
        write_ad_group_ad(out, &name, a, ad_group_addr, youtube_asset_addr, plan, i);
    }
    let (keyword_groups, ag_singletons) = partition_ad_group_criteria(&input.ad_group_criteria);
    for group in keyword_groups {
        let base = ad_group_criterion_group_base(&group, ad_group_addr);
        let name = names.allocate("google_ads_ad_group_criterion", &slugify(&base));
        write_ad_group_criterion_group(out, &name, &group, ad_group_addr);
    }
    for c in ag_singletons {
        let base = ad_group_criterion_base(c, ad_group_addr);
        let name = names.allocate("google_ads_ad_group_criterion", &slugify(&base));
        write_ad_group_criterion(out, &name, c, ad_group_addr, custom_audience_addr);
    }
    let remaining: Vec<&JsonCampaignCriterion> = input
        .campaign_criteria
        .iter()
        .filter(|c| !inline.folded.contains(&c.id))
        .collect();
    let (negative_groups, singletons) = partition_campaign_criteria(&remaining);
    for group in negative_groups {
        let base = campaign_negative_group_base(&group, campaign_addr);
        let name = names.allocate("google_ads_campaign_criterion", &slugify(&base));
        write_campaign_negative_group(out, &name, &group, campaign_addr, plan);
    }
    for c in singletons {
        let base = criterion_base(&c.target, &c.id);
        let name = names.allocate("google_ads_campaign_criterion", &slugify(&base));
        write_campaign_criterion(out, &name, c, campaign_addr, custom_audience_addr);
    }
}

fn write_campaign_shared_sets(
    out: &mut String,
    input: &ExportInput,
    names: &mut NameAllocator,
    campaign_addr: &HashMap<String, String>,
    shared_set_addr: &HashMap<String, String>,
) {
    for s in &input.campaign_shared_sets {
        let base = match (
            campaign_addr
                .get(&s.campaign)
                .and_then(|a| a.strip_prefix("google_ads_campaign.")),
            shared_set_addr
                .get(&s.shared_set)
                .and_then(|a| a.strip_prefix("google_ads_shared_set.")),
        ) {
            (Some(c), Some(ss)) => format!("{c}_{ss}"),
            _ => slugify(&s.id),
        };
        let name = names.allocate("google_ads_campaign_shared_set", &slugify(&base));
        write_campaign_shared_set(out, &name, s, campaign_addr, shared_set_addr);
    }
}

pub fn render_split(input: &ExportInput) -> (String, String) {
    let mut account = String::new();
    let mut campaigns = String::new();
    let mut names = NameAllocator::default();
    let inline = compute_inline_targeting(input);
    let plan = plan_fold(input);

    let mut budget_addr: HashMap<String, String> = HashMap::new();
    let mut campaign_addr: HashMap<String, String> = HashMap::new();
    let mut ad_group_addr: HashMap<String, String> = HashMap::new();
    let mut conversion_action_addr: HashMap<String, String> = HashMap::new();
    let mut asset_addr: HashMap<String, String> = HashMap::new();
    let mut youtube_asset_addr: HashMap<String, String> = HashMap::new();
    let mut shared_set_addr: HashMap<String, String> = HashMap::new();
    let mut custom_audience_addr: HashMap<String, String> = HashMap::new();

    let inline_assets = compute_inline_assets(input);
    write_provider(&mut account, input);
    write_account_assets(
        &mut account,
        input,
        &mut names,
        &mut conversion_action_addr,
        &mut asset_addr,
        &mut youtube_asset_addr,
        &inline_assets,
    );
    write_custom_audiences(&mut account, input, &mut names, &mut custom_audience_addr);
    write_shared_sets_and_criteria(&mut account, input, &mut names, &mut shared_set_addr);

    let has_campaign_resources = !input.campaign_budgets.is_empty()
        || !input.campaigns.is_empty()
        || !input.ad_groups.is_empty()
        || !input.ad_group_ads.is_empty()
        || !input.ad_group_criteria.is_empty()
        || !input.campaign_criteria.is_empty()
        || !input.campaign_shared_sets.is_empty()
        || !input.campaign_assets.is_empty()
        || !input.ad_group_assets.is_empty();

    if has_campaign_resources {
        write_provider(&mut campaigns, input);
        if plan.has_decls() {
            write_fold_decls(&mut campaigns, &plan);
        }
        write_campaign_tree(
            &mut campaigns,
            input,
            &mut names,
            &inline,
            &inline_assets,
            Some(&plan),
            &mut budget_addr,
            &mut campaign_addr,
            &mut ad_group_addr,
            &youtube_asset_addr,
            &custom_audience_addr,
        );
        write_campaign_shared_sets(
            &mut campaigns,
            input,
            &mut names,
            &campaign_addr,
            &shared_set_addr,
        );
        write_campaign_assets(
            &mut campaigns,
            input,
            &mut names,
            &campaign_addr,
            &ad_group_addr,
            &asset_addr,
            &inline_assets,
        );
    }

    while account.ends_with("\n\n\n") {
        account.pop();
    }
    while campaigns.ends_with("\n\n\n") {
        campaigns.pop();
    }

    (account, campaigns)
}

/// The resource types `import` can adopt: the ones the API refuses to label, so
/// `plan` has no way to reach them until a `.bid` block declares them. The
/// labelable kinds (campaign, ad group, ad, ad-group criterion) already adopt
/// themselves by content on the next `apply`, and `refresh -d` bootstraps them.
pub const IMPORTABLE_TYPES: &[&str] = &[
    "google_ads_sitelink_asset",
    "google_ads_callout_asset",
    "google_ads_structured_snippet_asset",
    "google_ads_call_asset",
    "google_ads_youtube_video_asset",
    "google_ads_customer_asset",
    "google_ads_campaign_asset",
    "google_ads_ad_group_asset",
    "google_ads_campaign_criterion",
    "google_ads_ad_group_criterion",
];

/// Addresses the tree already declares, keyed by the live id each one matched.
/// An imported block points at those instead of repeating the live resource, so
/// adoption lands as `asset = google_ads_sitelink_asset.shop.id` rather than a
/// second declaration of the same asset.
#[derive(Default)]
pub struct KnownAddresses {
    pub assets: HashMap<String, String>,
    pub campaigns: HashMap<String, String>,
    pub ad_groups: HashMap<String, String>,
    pub conversion_actions: HashMap<String, String>,
    pub custom_audiences: HashMap<String, String>,
    /// `<type>.<name>` pairs already used in the file being written into, so a
    /// dependency block cannot collide with a resource that is already there.
    pub taken: HashSet<(String, String)>,
}

#[derive(Debug)]
pub struct Imported {
    /// HCL text to append — dependency blocks first, the requested one last.
    pub text: String,
    /// `<type>.<name>` of every block rendered, in the order they appear.
    pub added: Vec<String>,
}

/// Render one live resource as a `.bid` block named `name`, plus any block it
/// references that the tree does not declare yet. Pure: the caller supplies the
/// live snapshot and what is already declared.
pub fn render_import(
    live: &ExportInput,
    ty: &str,
    id: &str,
    name: &str,
    known: &KnownAddresses,
) -> Result<Imported, String> {
    let mut names = NameAllocator::default();
    for (t, n) in &known.taken {
        names.allocate(t, n);
    }
    let mut out = String::new();
    let mut added: Vec<String> = Vec::new();
    let mut asset_addr = known.assets.clone();

    match ty {
        "google_ads_sitelink_asset" => {
            let a = find_by_id(&live.sitelink_assets, id, |a| &a.id, ty, id)?;
            write_sitelink_asset(&mut out, name, a);
        }
        "google_ads_callout_asset" => {
            let a = find_by_id(&live.callout_assets, id, |a| &a.id, ty, id)?;
            write_callout_asset(&mut out, name, a);
        }
        "google_ads_structured_snippet_asset" => {
            let a = find_by_id(&live.structured_snippet_assets, id, |a| &a.id, ty, id)?;
            write_structured_snippet_asset(&mut out, name, a);
        }
        "google_ads_call_asset" => {
            let a = find_by_id(&live.call_assets, id, |a| &a.id, ty, id)?;
            write_call_asset(&mut out, name, a, &known.conversion_actions);
        }
        "google_ads_youtube_video_asset" => {
            let a = find_by_id(&live.youtube_video_assets, id, |a| &a.id, ty, id)?;
            write_youtube_video_asset(&mut out, name, a);
        }
        "google_ads_customer_asset" => {
            let l = find_by_id(&live.customer_assets, id, |a| &a.id, ty, id)?;
            ensure_asset(&mut out, live, &l.asset, known, &mut asset_addr, &mut names, &mut added)?;
            write_customer_asset(&mut out, name, l, &asset_addr);
        }
        "google_ads_campaign_asset" => {
            let l = find_by_id(&live.campaign_assets, id, |a| &a.id, ty, id)?;
            ensure_asset(&mut out, live, &l.asset, known, &mut asset_addr, &mut names, &mut added)?;
            // An undeclared campaign is still expressible: `campaign` takes a
            // literal resource name as well as a reference.
            let link = JsonCampaignAsset {
                id: l.id.clone(),
                campaign: match known.campaigns.contains_key(&l.campaign) {
                    true => l.campaign.clone(),
                    false => format!("customers/{}/campaigns/{}", live.customer_id, l.campaign),
                },
                asset: l.asset.clone(),
                field_type: l.field_type.clone(),
                source: l.source.clone(),
                status: l.status.clone(),
            };
            write_campaign_asset(&mut out, name, &link, &known.campaigns, &asset_addr);
        }
        "google_ads_ad_group_asset" => {
            let l = find_by_id(&live.ad_group_assets, id, |a| &a.id, ty, id)?;
            ensure_asset(&mut out, live, &l.asset, known, &mut asset_addr, &mut names, &mut added)?;
            let link = JsonAdGroupAsset {
                id: l.id.clone(),
                ad_group: match known.ad_groups.contains_key(&l.ad_group) {
                    true => l.ad_group.clone(),
                    false => format!("customers/{}/adGroups/{}", live.customer_id, l.ad_group),
                },
                asset: l.asset.clone(),
                field_type: l.field_type.clone(),
                source: l.source.clone(),
                status: l.status.clone(),
            };
            write_ad_group_asset(&mut out, name, &link, &known.ad_groups, &asset_addr);
        }
        "google_ads_campaign_criterion" => {
            let c = find_by_id(&live.campaign_criteria, id, |c| &c.id, ty, id)?;
            if !known.campaigns.contains_key(&c.campaign) {
                return Err(undeclared_parent("campaign", &c.campaign));
            }
            write_campaign_criterion(&mut out, name, c, &known.campaigns, &known.custom_audiences);
        }
        "google_ads_ad_group_criterion" => {
            let c = find_by_id(&live.ad_group_criteria, id, |c| &c.id, ty, id)?;
            if !known.ad_groups.contains_key(&c.ad_group) {
                return Err(undeclared_parent("ad group", &c.ad_group));
            }
            write_ad_group_criterion(&mut out, name, c, &known.ad_groups, &known.custom_audiences);
        }
        other => {
            return Err(format!(
                "import does not handle '{other}'; it adopts {}",
                IMPORTABLE_TYPES.join(", ")
            ))
        }
    }

    added.push(format!("{ty}.{name}"));
    Ok(Imported { text: out, added })
}

fn find_by_id<'a, T>(
    items: &'a [T],
    id: &str,
    key: impl Fn(&T) -> &String,
    ty: &str,
    shown: &str,
) -> Result<&'a T, String> {
    items
        .iter()
        .find(|it| key(it) == id)
        .ok_or_else(|| format!("no live {ty} with id '{shown}' in this account"))
}

fn undeclared_parent(noun: &str, id: &str) -> String {
    format!(
        "the {noun} this criterion belongs to ({id}) is not declared in these files — \
         a criterion can only reference a declared parent, so declare or import the \
         {noun} first"
    )
}

/// Emit the asset a link points at, unless the tree already declares it.
fn ensure_asset(
    out: &mut String,
    live: &ExportInput,
    asset_id: &str,
    known: &KnownAddresses,
    asset_addr: &mut HashMap<String, String>,
    names: &mut NameAllocator,
    added: &mut Vec<String>,
) -> Result<(), String> {
    if asset_addr.contains_key(asset_id) {
        return Ok(());
    }
    let ty;
    let name;
    if let Some(a) = live.sitelink_assets.iter().find(|a| a.id == asset_id) {
        ty = "google_ads_sitelink_asset";
        name = names.allocate(ty, &format!("sitelink_{}", slugify(&a.link_text)));
        write_sitelink_asset(out, &name, a);
    } else if let Some(a) = live.callout_assets.iter().find(|a| a.id == asset_id) {
        ty = "google_ads_callout_asset";
        name = names.allocate(ty, &format!("callout_{}", slugify(&a.text)));
        write_callout_asset(out, &name, a);
    } else if let Some(a) = live
        .structured_snippet_assets
        .iter()
        .find(|a| a.id == asset_id)
    {
        ty = "google_ads_structured_snippet_asset";
        name = names.allocate(ty, &format!("snippet_{}", slugify(&a.header)));
        write_structured_snippet_asset(out, &name, a);
    } else if let Some(a) = live.call_assets.iter().find(|a| a.id == asset_id) {
        ty = "google_ads_call_asset";
        let base = slugify(&format!("call_{}_{}", a.country_code, a.phone_number));
        name = names.allocate(ty, &base);
        write_call_asset(out, &name, a, &known.conversion_actions);
    } else {
        return Err(format!(
            "the asset this link points at ({asset_id}) is not one bidsmith models \
             (sitelink, callout, structured snippet, call)"
        ));
    }
    let addr = format!("{ty}.{name}");
    asset_addr.insert(asset_id.to_string(), addr.clone());
    added.push(addr);
    Ok(())
}

fn render(input: &ExportInput) -> String {
    render_inner(input, true)
}

fn render_inner(input: &ExportInput, fold: bool) -> String {
    let mut out = String::new();
    let mut names = NameAllocator::default();
    let inline = compute_inline_targeting(input);
    let inline_assets = compute_inline_assets(input);
    let plan = fold.then(|| plan_fold(input));

    let mut budget_addr: HashMap<String, String> = HashMap::new();
    let mut campaign_addr: HashMap<String, String> = HashMap::new();
    let mut ad_group_addr: HashMap<String, String> = HashMap::new();
    let mut conversion_action_addr: HashMap<String, String> = HashMap::new();
    let mut asset_addr: HashMap<String, String> = HashMap::new();
    let mut youtube_asset_addr: HashMap<String, String> = HashMap::new();
    let mut shared_set_addr: HashMap<String, String> = HashMap::new();
    let mut custom_audience_addr: HashMap<String, String> = HashMap::new();

    write_provider(&mut out, input);
    write_account_assets(
        &mut out,
        input,
        &mut names,
        &mut conversion_action_addr,
        &mut asset_addr,
        &mut youtube_asset_addr,
        &inline_assets,
    );
    write_custom_audiences(&mut out, input, &mut names, &mut custom_audience_addr);
    if let Some(p) = &plan {
        if p.has_decls() {
            write_fold_decls(&mut out, p);
        }
    }
    write_campaign_tree(
        &mut out,
        input,
        &mut names,
        &inline,
        &inline_assets,
        plan.as_ref(),
        &mut budget_addr,
        &mut campaign_addr,
        &mut ad_group_addr,
        &youtube_asset_addr,
        &custom_audience_addr,
    );
    write_shared_sets_and_criteria(&mut out, input, &mut names, &mut shared_set_addr);
    write_campaign_shared_sets(&mut out, input, &mut names, &campaign_addr, &shared_set_addr);
    write_campaign_assets(
        &mut out,
        input,
        &mut names,
        &campaign_addr,
        &ad_group_addr,
        &asset_addr,
        &inline_assets,
    );

    while out.ends_with("\n\n\n") {
        out.pop();
    }
    out
}

fn write_provider(out: &mut String, input: &ExportInput) {
    out.push_str("provider \"google_ads\" {\n");
    write_attr(out, 1, "customer_id", &fmt_string(&input.customer_id));
    if let Some(lc) = &input.login_customer_id {
        write_attr(out, 1, "login_customer_id", &fmt_string(lc));
    }
    let owns: Vec<String> = crate::schema::ACCOUNT_OWNS
        .iter()
        .filter(|token| match crate::schema::account_owns_field_type(token) {
            Some(ft) => input.owned_account_assets.contains(ft),
            None => input.owns_account_automatic_assets,
        })
        .map(|token| fmt_string(token))
        .collect();
    if !owns.is_empty() {
        write_attr(out, 1, "owns", &format!("[{}]", owns.join(", ")));
    }
    out.push_str("}\n\n");
}

/// The budget type an ordinary campaign gets; rendering it on every budget
/// would be noise, and it is what Google picks when `type` is omitted.
const STANDARD_BUDGET_TYPE: &str = "STANDARD";

fn write_budget(out: &mut String, name: &str, b: &JsonBudget) {
    let _ = writeln!(out, "resource \"google_ads_campaign_budget\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&b.name));
    // Only the amount the period actually spends: a live custom-period budget
    // can still carry a stale `amount_micros`, and rendering both would produce
    // a file the validator rejects.
    if b.is_custom_period() {
        write_attr(
            out,
            1,
            "total_amount_micros",
            &b.total_amount_micros.unwrap_or(0).to_string(),
        );
        write_attr(out, 1, "period", &fmt_string(crate::schema::CUSTOM_PERIOD));
    } else {
        write_attr(
            out,
            1,
            "amount_micros",
            &b.amount_micros.unwrap_or(0).to_string(),
        );
    }
    if let Some(t) = b.ty.as_deref().filter(|t| *t != STANDARD_BUDGET_TYPE) {
        write_attr(out, 1, "type", &fmt_string(t));
    }
    if let Some(dm) = &b.delivery_method {
        write_attr(out, 1, "delivery_method", &fmt_string(dm));
    }
    if let Some(es) = b.explicitly_shared {
        write_attr(out, 1, "explicitly_shared", &es.to_string());
    }
    out.push_str("}\n\n");
}

/// Callouts and structured snippets folded onto their owner. Shared by the
/// campaign and ad group renderers so the two spellings cannot drift.
fn write_inline_text_assets(
    out: &mut String,
    callouts: &[String],
    snippets: &[(String, Vec<String>)],
) {
    if !callouts.is_empty() {
        write_attr(out, 1, "callouts", &fmt_string_list(callouts));
    }
    for (header, values) in snippets {
        out.push_str("\n  structured_snippet {\n");
        write_attr(out, 2, "header", &fmt_string(header));
        write_attr(out, 2, "values", &fmt_string_list(values));
        out.push_str("  }\n");
    }
}

/// The `final_url_suffix` / `custom_parameters` pair, rendered wherever it
/// appears (campaign, ad group, ad body).
fn write_tracking(
    out: &mut String,
    indent: usize,
    suffix: &Option<String>,
    params: &Option<Vec<JsonCustomParameter>>,
) {
    if let Some(s) = suffix {
        write_attr(out, indent, "final_url_suffix", &fmt_string(s));
    }
    let Some(params) = params else { return };
    if params.is_empty() {
        return;
    }
    let body = params
        .iter()
        .map(|p| format!("{} = {}", p.key, fmt_string(&p.value)))
        .collect::<Vec<_>>()
        .join(", ");
    write_attr(out, indent, "custom_parameters", &format!("{{ {body} }}"));
}

fn write_campaign(
    out: &mut String,
    name: &str,
    c: &JsonCampaign,
    budget_addr: &HashMap<String, String>,
    languages: &[String],
    locations: &[String],
    devices: Option<&InlineDevices>,
    callouts: &[String],
    snippets: &[(String, Vec<String>)],
) {
    let _ = writeln!(out, "resource \"google_ads_campaign\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&c.name));
    if let Some(s) = &c.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    write_attr(
        out,
        1,
        "advertising_channel_type",
        &fmt_string(&c.advertising_channel_type),
    );
    if let Some(v) = &c.advertising_channel_sub_type {
        write_attr(out, 1, "advertising_channel_sub_type", &fmt_string(v));
    }
    let budget_ref = match budget_addr.get(&c.campaign_budget) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved budget {}>\"", c.campaign_budget),
    };
    write_attr(out, 1, "campaign_budget", &budget_ref);
    if let Some(v) = &c.contains_eu_political_advertising {
        write_attr(out, 1, "contains_eu_political_advertising", &fmt_string(v));
    }
    if let Some(v) = &c.start_date {
        write_attr(out, 1, "start_date", &fmt_string(v));
    }
    if let Some(v) = &c.end_date {
        write_attr(out, 1, "end_date", &fmt_string(v));
    }
    if !languages.is_empty() {
        write_attr(out, 1, "languages", &fmt_string_list(languages));
    }
    if !locations.is_empty() {
        write_attr(out, 1, "locations", &fmt_string_list(locations));
    }
    if let Some(d) = devices {
        write_attr(out, 1, d.attr, &fmt_string_list(&d.values));
    }
    if c.owns_automatic_assets {
        write_attr(
            out,
            1,
            "owns",
            &fmt_string_list(&[crate::schema::AUTOMATIC_ASSETS_OWNS.to_string()]),
        );
    }
    write_tracking(out, 1, &c.final_url_suffix, &c.custom_parameters);
    write_inline_text_assets(out, callouts, snippets);

    if let Some(m) = &c.manual_cpc {
        match m.enhanced_cpc_enabled {
            Some(e) => {
                out.push_str("\n  manual_cpc {\n");
                write_attr(out, 2, "enhanced_cpc_enabled", &e.to_string());
                out.push_str("  }\n");
            }
            None => out.push_str("\n  manual_cpc {}\n"),
        }
    }
    for (name, set) in [
        ("manual_cpm", c.manual_cpm.is_some()),
        ("manual_cpv", c.manual_cpv.is_some()),
        ("target_cpm", c.target_cpm.is_some()),
        ("target_cpv", c.target_cpv.is_some()),
    ] {
        if set {
            let _ = writeln!(out, "\n  {name} {{}}");
        }
    }
    if let Some(t) = &c.target_impression_share {
        out.push_str("\n  target_impression_share {\n");
        if let Some(l) = &t.location {
            write_attr(out, 2, "location", &fmt_string(l));
        }
        if let Some(f) = t.location_fraction_micros {
            write_attr(out, 2, "location_fraction_micros", &f.to_string());
        }
        if let Some(v) = t.cpc_bid_ceiling_micros {
            write_attr(out, 2, "cpc_bid_ceiling_micros", &v.to_string());
        }
        out.push_str("  }\n");
    }
    if let Some(t) = &c.target_spend {
        match t.cpc_bid_ceiling_micros {
            Some(v) => {
                out.push_str("\n  target_spend {\n");
                write_attr(out, 2, "cpc_bid_ceiling_micros", &v.to_string());
                out.push_str("  }\n");
            }
            None => out.push_str("\n  target_spend {}\n"),
        }
    }
    if let Some(n) = &c.network_settings {
        out.push_str("\n  network_settings {\n");
        for (field, _) in crate::schema::NETWORK_SETTINGS_FIELDS {
            if let Some(v) = n.get(field) {
                write_attr(out, 2, field, &v.to_string());
            }
        }
        out.push_str("  }\n");
    }
    if let Some(g) = c.geo_target_type_setting.as_ref().filter(|g| !g.is_empty()) {
        out.push_str("\n  geo_target_type_setting {\n");
        for (field, _) in crate::schema::GEO_TARGET_TYPE_FIELDS {
            if let Some(v) = g.get(field) {
                write_attr(out, 2, field, &fmt_string(v));
            }
        }
        out.push_str("  }\n");
    }
    if let Some(v) = c.video_campaign_settings.as_ref().filter(|v| !v.is_empty()) {
        out.push_str("\n  video_campaign_settings {\n");
        if let Some(i) = &v.video_ad_inventory_control {
            out.push_str("    video_ad_inventory_control {\n");
            for (field, _) in crate::schema::VIDEO_AD_INVENTORY_FIELDS {
                if let Some(v) = i.get(field) {
                    write_attr(out, 3, field, &v.to_string());
                }
            }
            out.push_str("    }\n");
        }
        out.push_str("  }\n");
    }
    if let Some(a) = c.asset_automation_settings.as_ref().filter(|a| !a.is_empty()) {
        out.push_str("\n  asset_automation_settings {\n");
        for (field, _) in crate::schema::ASSET_AUTOMATION_FIELDS {
            if let Some(v) = a.get(field) {
                write_attr(out, 2, field, &fmt_string(v));
            }
        }
        out.push_str("  }\n");
    }
    if let Some(d) = c.dynamic_search_ads_setting.as_ref().filter(|d| !d.is_empty()) {
        out.push_str("\n  dynamic_search_ads_setting {\n");
        if let Some(v) = &d.domain_name {
            write_attr(out, 2, "domain_name", &fmt_string(v));
        }
        if let Some(v) = &d.language_code {
            write_attr(out, 2, "language_code", &fmt_string(v));
        }
        if let Some(v) = d.use_supplied_urls_only {
            write_attr(out, 2, "use_supplied_urls_only", &v.to_string());
        }
        out.push_str("  }\n");
    }
    if let Some(v) = c.ai_max_setting.as_ref().and_then(|a| a.enable_ai_max) {
        out.push_str("\n  ai_max_setting {\n");
        write_attr(out, 2, "enable_ai_max", &v.to_string());
        out.push_str("  }\n");
    }
    write_targeting_setting(out, c.targeting_setting.as_ref());
    for f in &c.frequency_caps {
        out.push_str("\n  frequency_caps {\n");
        write_attr(out, 2, "event_type", &fmt_string(&f.event_type));
        write_attr(out, 2, "time_unit", &fmt_string(&f.time_unit));
        write_attr(out, 2, "time_length", &f.time_length.to_string());
        write_attr(out, 2, "cap", &f.cap.to_string());
        if f.level_or_default() != crate::schema::DEFAULT_FREQUENCY_CAP_LEVEL {
            write_attr(out, 2, "level", &fmt_string(f.level_or_default()));
        }
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn write_ad_group(
    out: &mut String,
    name: &str,
    g: &JsonAdGroup,
    campaign_addr: &HashMap<String, String>,
    callouts: &[String],
    snippets: &[(String, Vec<String>)],
) {
    let _ = writeln!(out, "resource \"google_ads_ad_group\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&g.name));
    let campaign_ref = match campaign_addr.get(&g.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved campaign {}>\"", g.campaign),
    };
    write_attr(out, 1, "campaign", &campaign_ref);
    if let Some(s) = &g.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if let Some(t) = &g.ty {
        write_attr(out, 1, "type", &fmt_string(t));
    }
    // Google returns 0 for every bid field the ad group's strategy doesn't use,
    // so rendering them all would bury the one live bid under five zeroes — and
    // a declared zero is a real value the create path would send, which the API
    // rejects when it doesn't match the strategy. `cpc_bid_micros` is emitted
    // even at zero, as it always has been.
    for (field, _) in crate::schema::AD_GROUP_BID_FIELDS {
        if let Some(c) = g.bid(field) {
            if c != 0 || *field == "cpc_bid_micros" {
                write_attr(out, 1, field, &c.to_string());
            }
        }
    }
    write_tracking(out, 1, &g.final_url_suffix, &g.custom_parameters);
    write_inline_text_assets(out, callouts, snippets);
    write_targeting_setting(out, g.targeting_setting.as_ref());
    if let Some(v) = g
        .ai_max_ad_group_setting
        .as_ref()
        .and_then(|a| a.disable_search_term_matching)
    {
        out.push_str("\n  ai_max_ad_group_setting {\n");
        write_attr(out, 2, "disable_search_term_matching", &v.to_string());
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

/// Only the restrictions that differ from what an absent entry would mean —
/// Google reports a default for every dimension it has an opinion about, and
/// rendering those would put a dozen lines of boilerplate in every ad group.
fn write_targeting_setting(out: &mut String, setting: Option<&JsonTargetingSetting>) {
    let Some(setting) = setting else { return };
    let effective = setting.effective();
    if effective.is_empty() {
        out.push_str("\n  targeting_setting {}\n");
        return;
    }
    out.push_str("\n  targeting_setting {\n");
    for (i, (dimension, bid_only)) in effective.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str("    target_restriction {\n");
        write_attr(out, 3, "targeting_dimension", &fmt_string(dimension));
        write_attr(out, 3, "bid_only", &bid_only.to_string());
        out.push_str("    }\n");
    }
    out.push_str("  }\n");
}

fn write_ad_group_ad(
    out: &mut String,
    name: &str,
    a: &JsonAdGroupAd,
    ad_group_addr: &HashMap<String, String>,
    youtube_asset_addr: &HashMap<String, String>,
    plan: Option<&FoldPlan>,
    idx: usize,
) {
    let _ = writeln!(out, "resource \"google_ads_ad_group_ad\" \"{name}\" {{");
    let ad_group_ref = match ad_group_addr.get(&a.ad_group) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved ad_group {}>\"", a.ad_group),
    };
    write_attr(out, 1, "ad_group", &ad_group_ref);
    if let Some(s) = &a.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }

    if let Some(ov) = plan.and_then(|p| p.ad_emit.get(&idx)) {
        write_attr(out, 1, "template", &format!("ad_template.{}", ov.template));
        if let Some(urls) = &ov.final_urls {
            write_attr(out, 1, "final_urls", &fmt_string_list(urls));
        }
        if let Some(p) = &ov.path1 {
            write_attr(out, 1, "path1", &fmt_string(p));
        }
        if let Some(p) = &ov.path2 {
            write_attr(out, 1, "path2", &fmt_string(p));
        }
        out.push_str("}\n\n");
        return;
    }

    out.push_str("\n  ad {\n");
    if let Some(n) = &a.ad.name {
        write_attr(out, 2, "name", &fmt_string(n));
    }
    write_attr(out, 2, "final_urls", &fmt_string_list(&a.ad.final_urls));
    if !a.ad.final_mobile_urls.is_empty() {
        write_attr(out, 2, "final_mobile_urls", &fmt_string_list(&a.ad.final_mobile_urls));
    }
    if let Some(u) = &a.ad.display_url {
        write_attr(out, 2, "display_url", &fmt_string(u));
    }
    write_tracking(out, 2, &a.ad.final_url_suffix, &a.ad.custom_parameters);
    if let Some(rsa) = &a.ad.responsive_search_ad {
        out.push_str("\n    responsive_search_ad {\n");
        write_rsa_list_attr(out, 3, "headlines", &rsa.headlines, plan.map(|p| &p.headline_local_by_key));
        write_rsa_list_attr(out, 3, "descriptions", &rsa.descriptions, plan.map(|p| &p.description_local_by_key));
        if let Some(p) = &rsa.path1 {
            write_attr(out, 3, "path1", &fmt_string(p));
        }
        if let Some(p) = &rsa.path2 {
            write_attr(out, 3, "path2", &fmt_string(p));
        }
        out.push_str("    }\n");
    }
    if let Some(video) = &a.ad.video_responsive_ad {
        out.push_str("\n    video_responsive_ad {\n");
        let video_ref = match youtube_asset_addr.get(&video.video) {
            Some(addr) => format!("{addr}.id"),
            None => format!("\"<unresolved video {}>\"", video.video),
        };
        write_attr(out, 3, "video", &video_ref);
        for (attr, items) in [
            ("headlines", &video.headlines),
            ("long_headlines", &video.long_headlines),
            ("descriptions", &video.descriptions),
            ("call_to_actions", &video.call_to_actions),
        ] {
            if !items.is_empty() {
                write_attr(out, 3, attr, &fmt_string_list(items));
            }
        }
        if let Some(b) = &video.breadcrumb1 {
            write_attr(out, 3, "breadcrumb1", &fmt_string(b));
        }
        if let Some(b) = &video.breadcrumb2 {
            write_attr(out, 3, "breadcrumb2", &fmt_string(b));
        }
        out.push_str("    }\n");
    }
    if let Some(video) = &a.ad.video_ad {
        out.push_str("\n    video_ad {\n");
        let video_ref = match youtube_asset_addr.get(&video.video) {
            Some(addr) => format!("{addr}.id"),
            None => format!("\"<unresolved video {}>\"", video.video),
        };
        write_attr(out, 3, "video", &video_ref);
        out.push_str("    }\n");
    }
    if let Some(dg) = &a.ad.demand_gen_video_responsive_ad {
        out.push_str("\n    demand_gen_video_responsive_ad {\n");
        if !dg.videos.is_empty() {
            let refs: Vec<String> = dg
                .videos
                .iter()
                .map(|id| match youtube_asset_addr.get(id) {
                    Some(addr) => format!("{addr}.id"),
                    None => format!("\"<unresolved video {id}>\""),
                })
                .collect();
            write_attr(out, 3, "videos", &format!("[{}]", refs.join(", ")));
        }
        for (attr, items) in [
            ("headlines", &dg.headlines),
            ("long_headlines", &dg.long_headlines),
            ("descriptions", &dg.descriptions),
            ("call_to_actions", &dg.call_to_actions),
        ] {
            if !items.is_empty() {
                write_attr(out, 3, attr, &fmt_string_list(items));
            }
        }
        if let Some(b) = &dg.business_name {
            write_attr(out, 3, "business_name", &fmt_string(b));
        }
        if let Some(b) = &dg.breadcrumb1 {
            write_attr(out, 3, "breadcrumb1", &fmt_string(b));
        }
        if let Some(b) = &dg.breadcrumb2 {
            write_attr(out, 3, "breadcrumb2", &fmt_string(b));
        }
        out.push_str("    }\n");
    }
    out.push_str("  }\n");
    out.push_str("}\n\n");
}

fn write_youtube_video_asset(out: &mut String, name: &str, v: &JsonYoutubeVideoAsset) {
    let _ = writeln!(
        out,
        "resource \"google_ads_youtube_video_asset\" \"{name}\" {{"
    );
    write_attr(out, 1, "youtube_video_id", &fmt_string(&v.youtube_video_id));
    if let Some(t) = &v.youtube_video_title {
        write_attr(out, 1, "youtube_video_title", &fmt_string(t));
    }
    out.push_str("}\n\n");
}

fn write_rsa_list_attr(
    out: &mut String,
    indent: usize,
    name: &str,
    assets: &[JsonRsaAsset],
    by_key: Option<&HashMap<AssetKey, String>>,
) {
    if assets.is_empty() {
        return;
    }
    match by_key.and_then(|m| m.get(&asset_key(assets))) {
        Some(local) => write_attr(out, indent, name, &format!("local.{local}")),
        None => write_attr(out, indent, name, &fmt_rsa_asset_list(assets)),
    }
}

fn write_fold_decls(out: &mut String, plan: &FoldPlan) {
    if plan.has_locals() {
        out.push_str("locals {\n");
        for (name, key) in &plan.headline_locals {
            write_attr(out, 1, name, &fmt_rsa_asset_list(&assets_from_key(key)));
        }
        for (name, key) in &plan.description_locals {
            write_attr(out, 1, name, &fmt_rsa_asset_list(&assets_from_key(key)));
        }
        for (name, texts) in &plan.negative_locals {
            write_attr(out, 1, name, &fmt_string_list(texts));
        }
        out.push_str("}\n\n");
    }
    for t in &plan.templates {
        write_template(out, t, plan);
    }
}

fn write_template(out: &mut String, t: &TemplateDecl, plan: &FoldPlan) {
    let _ = writeln!(out, "ad_template \"{}\" {{", t.name);
    let mut wrote_attr = false;
    if let Some(n) = &t.ad_name {
        write_attr(out, 1, "name", &fmt_string(n));
        wrote_attr = true;
    }
    if let Some(urls) = &t.final_urls {
        write_attr(out, 1, "final_urls", &fmt_string_list(urls));
        wrote_attr = true;
    }
    if wrote_attr {
        out.push('\n');
    }
    out.push_str("  responsive_search_ad {\n");
    write_rsa_list_attr(out, 2, "headlines", &t.headlines, Some(&plan.headline_local_by_key));
    write_rsa_list_attr(out, 2, "descriptions", &t.descriptions, Some(&plan.description_local_by_key));
    if let Some(p) = &t.path1 {
        write_attr(out, 2, "path1", &fmt_string(p));
    }
    if let Some(p) = &t.path2 {
        write_attr(out, 2, "path2", &fmt_string(p));
    }
    out.push_str("  }\n}\n\n");
}

type AdGroupCriterionKey = (String, bool, Option<String>, Option<i64>, Option<String>);

/// Keywords group into one resource per (ad group, polarity, status, bid, match
/// type); every other axis is a singleton, for the same reason campaign criteria
/// are — it carries its own polarity and would round-trip as positive targeting
/// if folded into a grouped form.
fn partition_ad_group_criteria(
    items: &[JsonAdGroupCriterion],
) -> (Vec<Vec<&JsonAdGroupCriterion>>, Vec<&JsonAdGroupCriterion>) {
    let mut groups: Vec<Vec<&JsonAdGroupCriterion>> = Vec::new();
    let mut index: HashMap<AdGroupCriterionKey, usize> = HashMap::new();
    let mut singletons: Vec<&JsonAdGroupCriterion> = Vec::new();
    for c in items {
        let Some(kw) = &c.target.keyword else {
            singletons.push(c);
            continue;
        };
        let neg = c.negative.unwrap_or(false);
        let match_type_key = if neg {
            None
        } else {
            Some(kw.match_type.clone())
        };
        let key = (
            c.ad_group.clone(),
            neg,
            c.status.clone(),
            c.cpc_bid_micros,
            match_type_key,
        );
        let idx = match index.get(&key) {
            Some(&i) => i,
            None => {
                let i = groups.len();
                index.insert(key, i);
                groups.push(Vec::new());
                i
            }
        };
        groups[idx].push(c);
    }
    (groups, singletons)
}

fn ad_group_slug<'a>(ad_group_addr: &'a HashMap<String, String>, ad_group: &'a str) -> &'a str {
    ad_group_addr
        .get(ad_group)
        .and_then(|s| s.strip_prefix("google_ads_ad_group."))
        .unwrap_or(ad_group)
}

fn ad_group_criterion_group_base(
    group: &[&JsonAdGroupCriterion],
    ad_group_addr: &HashMap<String, String>,
) -> String {
    let first = group[0];
    let ag_slug = ad_group_slug(ad_group_addr, &first.ad_group);
    if first.negative.unwrap_or(false) {
        format!("{ag_slug}_negatives")
    } else {
        let match_type = first
            .target
            .keyword
            .as_ref()
            .map(|kw| kw.match_type.to_ascii_lowercase())
            .unwrap_or_default();
        format!("{ag_slug}_{match_type}")
    }
}

fn ad_group_criterion_base(
    c: &JsonAdGroupCriterion,
    ad_group_addr: &HashMap<String, String>,
) -> String {
    format!(
        "{}_{}",
        ad_group_slug(ad_group_addr, &c.ad_group),
        criterion_base(&c.target, &c.id),
    )
}

fn write_ad_group_criterion_group(
    out: &mut String,
    name: &str,
    group: &[&JsonAdGroupCriterion],
    ad_group_addr: &HashMap<String, String>,
) {
    let first = group[0];
    let _ = writeln!(
        out,
        "resource \"google_ads_ad_group_criterion\" \"{name}\" {{"
    );
    let ag_ref = match ad_group_addr.get(&first.ad_group) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved ad_group {}>\"", first.ad_group),
    };
    write_attr(out, 1, "ad_group", &ag_ref);
    if let Some(s) = &first.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if let Some(cpc) = first.cpc_bid_micros {
        write_attr(out, 1, "cpc_bid_micros", &cpc.to_string());
    }
    let block_name = if first.negative.unwrap_or(false) {
        "negative_keyword"
    } else {
        "keyword"
    };
    for c in group {
        let Some(kw) = &c.target.keyword else { continue };
        out.push('\n');
        let _ = writeln!(out, "  {block_name} {{");
        write_attr(out, 2, "text", &fmt_string(&kw.text));
        write_attr(out, 2, "match_type", &fmt_string(&kw.match_type));
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

/// A non-keyword ad-group criterion: audience, placement, demographic, or the
/// geo / language axes that intersect with the campaign's own targeting.
fn write_ad_group_criterion(
    out: &mut String,
    name: &str,
    c: &JsonAdGroupCriterion,
    ad_group_addr: &HashMap<String, String>,
    custom_audience_addr: &HashMap<String, String>,
) {
    let _ = writeln!(
        out,
        "resource \"google_ads_ad_group_criterion\" \"{name}\" {{"
    );
    let ag_ref = match ad_group_addr.get(&c.ad_group) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved ad_group {}>\"", c.ad_group),
    };
    write_attr(out, 1, "ad_group", &ag_ref);
    if let Some(s) = &c.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if c.negative.unwrap_or(false) {
        write_attr(out, 1, "negative", "true");
    }
    if let Some(cpc) = c.cpc_bid_micros {
        write_attr(out, 1, "cpc_bid_micros", &cpc.to_string());
    }
    if let Some(bm) = c.bid_modifier {
        write_attr(out, 1, "bid_modifier", &format_number(bm));
    }
    write_criterion_blocks(out, &c.target, custom_audience_addr);
    out.push_str("}\n\n");
}

/// Text assets a campaign is the only user of, folded back onto it as
/// `callouts` / `structured_snippet`. A shared asset — attached to a second
/// campaign, an ad group, or the account — keeps its resource form, because
/// that is the thing the inline spelling cannot express (issue #145).
#[derive(Default)]
pub struct InlineAssets {
    callouts: HashMap<String, Vec<String>>,
    snippets: HashMap<String, Vec<(String, Vec<String>)>>,
    folded_assets: HashSet<String>,
    folded_links: HashSet<String>,
}

impl InlineAssets {
    fn callouts_for(&self, owner_id: &str) -> &[String] {
        self.callouts.get(owner_id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn snippets_for(&self, owner_id: &str) -> &[(String, Vec<String>)] {
        self.snippets.get(owner_id).map(Vec::as_slice).unwrap_or(&[])
    }
}

fn compute_inline_assets(input: &ExportInput) -> InlineAssets {
    let campaign_ids: HashSet<&str> = input.campaigns.iter().map(|c| c.id.as_str()).collect();
    // How many links of any kind point at each asset.
    let mut uses: HashMap<&str, usize> = HashMap::new();
    for a in &input.campaign_assets {
        *uses.entry(a.asset.as_str()).or_default() += 1;
    }
    for a in &input.ad_group_assets {
        *uses.entry(a.asset.as_str()).or_default() += 1;
    }
    for a in &input.customer_assets {
        *uses.entry(a.asset.as_str()).or_default() += 1;
    }

    let callout_text: HashMap<&str, &str> = input
        .callout_assets
        .iter()
        .map(|a| (a.id.as_str(), a.text.as_str()))
        .collect();
    let snippet_body: HashMap<&str, (&str, &[String])> = input
        .structured_snippet_assets
        .iter()
        .map(|a| (a.id.as_str(), (a.header.as_str(), a.values.as_slice())))
        .collect();

    let ad_group_ids: HashSet<&str> = input.ad_groups.iter().map(|g| g.id.as_str()).collect();

    let mut out = InlineAssets::default();
    // Owner id, link id, asset id, and whether the owner is still in this tree.
    let owned = input
        .campaign_assets
        .iter()
        .map(|l| {
            (
                &l.campaign,
                &l.id,
                &l.asset,
                &l.status,
                campaign_ids.contains(l.campaign.as_str()),
            )
        })
        .chain(input.ad_group_assets.iter().map(|l| {
            (
                &l.ad_group,
                &l.id,
                &l.asset,
                &l.status,
                ad_group_ids.contains(l.ad_group.as_str()),
            )
        }));
    for (owner, link_id, asset_id, status, owner_present) in owned {
        if !owner_present {
            continue;
        }
        if uses.get(asset_id.as_str()).copied().unwrap_or(0) != 1 {
            continue;
        }
        if !matches!(status.as_deref(), None | Some("ENABLED")) {
            continue;
        }
        if let Some(text) = callout_text.get(asset_id.as_str()) {
            out.callouts.entry(owner.clone()).or_default().push((*text).to_string());
        } else if let Some((header, values)) = snippet_body.get(asset_id.as_str()) {
            out.snippets
                .entry(owner.clone())
                .or_default()
                .push(((*header).to_string(), values.to_vec()));
        } else {
            continue;
        }
        out.folded_assets.insert(asset_id.clone());
        out.folded_links.insert(link_id.clone());
    }
    out
}

/// Positive, ENABLED language / location criteria fold onto their campaign as
/// inline `languages` / `locations` attributes; everything else (negatives,
/// proximity, keywords, paused/removed) stays an explicit criterion resource.
#[derive(Default)]
struct InlineTargeting {
    languages: HashMap<String, Vec<String>>,
    locations: HashMap<String, Vec<String>>,
    devices: HashMap<String, InlineDevices>,
    folded: HashSet<String>,
}

/// Which spelling a campaign's device criteria fold back to, and its values.
struct InlineDevices {
    attr: &'static str,
    values: Vec<String>,
}

impl InlineTargeting {
    fn languages_for(&self, campaign_id: &str) -> &[String] {
        self.languages.get(campaign_id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn locations_for(&self, campaign_id: &str) -> &[String] {
        self.locations.get(campaign_id).map(Vec::as_slice).unwrap_or(&[])
    }

    fn devices_for(&self, campaign_id: &str) -> Option<&InlineDevices> {
        self.devices.get(campaign_id)
    }
}

fn compute_inline_targeting(input: &ExportInput) -> InlineTargeting {
    let campaign_ids: HashSet<&str> = input.campaigns.iter().map(|c| c.id.as_str()).collect();
    let mut t = InlineTargeting::default();
    for c in &input.campaign_criteria {
        if !campaign_ids.contains(c.campaign.as_str()) {
            continue;
        }
        let foldable = !c.negative.unwrap_or(false)
            && matches!(c.status.as_deref(), None | Some("ENABLED"))
            && c.bid_modifier.is_none()
            && c.target.keyword.is_none()
            && c.target.proximity.is_none()
            && c.target.device.is_none();
        if !foldable {
            continue;
        }
        if let Some(loc) = &c.target.location {
            let entry = crate::targeting::location_code(&loc.geo_target_constant)
                .map(str::to_string)
                .unwrap_or_else(|| loc.geo_target_constant.clone());
            t.locations.entry(c.campaign.clone()).or_default().push(entry);
            t.folded.insert(c.id.clone());
        } else if let Some(lang) = &c.target.language {
            let entry = crate::targeting::language_code(&lang.language_constant)
                .map(str::to_string)
                .unwrap_or_else(|| lang.language_constant.clone());
            t.languages.entry(c.campaign.clone()).or_default().push(entry);
            t.folded.insert(c.id.clone());
        }
    }
    for v in t.languages.values_mut() {
        v.sort();
        v.dedup();
    }
    for v in t.locations.values_mut() {
        v.sort();
        v.dedup();
    }
    fold_devices(input, &campaign_ids, &mut t);
    t
}

/// Collapse a campaign's device criteria back to `devices` / `excluded_devices`
/// when — and only when — they are exactly what one of those attributes expands
/// to. A real bid adjustment (`0.8` on mobile) is not an exclusion and keeps its
/// explicit criterion, as does any set the closed `devices` form would misstate.
fn fold_devices(input: &ExportInput, campaign_ids: &HashSet<&str>, t: &mut InlineTargeting) {
    let mut by_campaign: HashMap<&str, Vec<&JsonCampaignCriterion>> = HashMap::new();
    for c in &input.campaign_criteria {
        if c.target.device.is_some() && campaign_ids.contains(c.campaign.as_str()) {
            by_campaign.entry(c.campaign.as_str()).or_default().push(c);
        }
    }
    for (campaign, criteria) in by_campaign {
        let foldable = criteria.iter().all(|c| {
            !c.negative.unwrap_or(false) && matches!(c.status.as_deref(), None | Some("ENABLED"))
        });
        if !foldable {
            continue;
        }
        let mut targeted: Vec<String> = Vec::new();
        let mut excluded: Vec<String> = Vec::new();
        let mut adjusted = false;
        for c in &criteria {
            let ty = c.target.device.as_ref().expect("filtered above").ty.clone();
            match c.bid_modifier {
                None => targeted.push(ty),
                Some(m) if m.abs() < 1e-6 => excluded.push(ty),
                Some(m) if (m - 1.0).abs() < 1e-6 => targeted.push(ty),
                Some(_) => adjusted = true,
            }
        }
        if adjusted {
            continue;
        }
        targeted.sort();
        excluded.sort();
        let folded = if targeted.is_empty() {
            (!excluded.is_empty()).then(|| InlineDevices {
                attr: "excluded_devices",
                values: excluded.clone(),
            })
        } else {
            // The closed form only survives a round trip when the criteria
            // cover every core device type: anything missing would come back
            // as an exclusion that live state does not actually carry.
            let mut covered: Vec<&str> =
                targeted.iter().chain(excluded.iter()).map(String::as_str).collect();
            covered.sort();
            let mut core: Vec<&str> = crate::schema::CORE_DEVICE_TYPES.to_vec();
            core.sort();
            core.iter()
                .all(|d| covered.binary_search(d).is_ok())
                .then(|| InlineDevices { attr: "devices", values: targeted.clone() })
        };
        let Some(folded) = folded else { continue };
        for c in &criteria {
            t.folded.insert(c.id.clone());
        }
        t.devices.insert(campaign.to_string(), folded);
    }
}

// ---- Folding (issue #57): collapse repeated structure into `ad_template` + `locals` ----
//
// `refresh` / `export` are bootstrap emitters: every run reflects live state, so
// folding is a pure *source representation* change. The constructs all expand back
// to the identical mutate at import time (templates → #40/#58, list `locals` → #39),
// so the folded tree round-trips through `validate` / `plan` exactly like the
// verbose one. This is enforced by `fold_roundtrips_to_verbose` in the test module.

type AssetKey = Vec<(String, Option<String>)>;

fn asset_key(assets: &[JsonRsaAsset]) -> AssetKey {
    assets.iter().map(|a| (a.text.clone(), a.pin.clone())).collect()
}

fn assets_from_key(key: &AssetKey) -> Vec<JsonRsaAsset> {
    key.iter()
        .map(|(text, pin)| JsonRsaAsset {
            text: text.clone(),
            pin: pin.clone(),
        })
        .collect()
}

struct TemplateDecl {
    name: String,
    ad_name: Option<String>,
    final_urls: Option<Vec<String>>,
    headlines: Vec<JsonRsaAsset>,
    descriptions: Vec<JsonRsaAsset>,
    path1: Option<String>,
    path2: Option<String>,
}

#[derive(Clone)]
struct AdOverride {
    template: String,
    final_urls: Option<Vec<String>>,
    path1: Option<String>,
    path2: Option<String>,
}

#[derive(Default)]
struct FoldPlan {
    templates: Vec<TemplateDecl>,
    ad_emit: HashMap<usize, AdOverride>,
    headline_locals: Vec<(String, AssetKey)>,
    description_locals: Vec<(String, AssetKey)>,
    headline_local_by_key: HashMap<AssetKey, String>,
    description_local_by_key: HashMap<AssetKey, String>,
    negative_locals: Vec<(String, Vec<String>)>,
    campaign_negative_fold: HashMap<(String, String), (String, String)>,
}

impl FoldPlan {
    fn has_decls(&self) -> bool {
        !self.templates.is_empty()
            || !self.headline_locals.is_empty()
            || !self.description_locals.is_empty()
            || !self.negative_locals.is_empty()
    }

    fn has_locals(&self) -> bool {
        !self.headline_locals.is_empty()
            || !self.description_locals.is_empty()
            || !self.negative_locals.is_empty()
    }
}

fn truncate_slug(s: &str) -> String {
    const MAX: usize = 40;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let mut t: String = s.chars().take(MAX).collect();
    while t.ends_with('_') {
        t.pop();
    }
    t
}

fn template_base(a: &JsonAdGroupAd) -> String {
    if let Some(rsa) = &a.ad.responsive_search_ad {
        if let Some(h) = rsa.headlines.first() {
            return format!("{}_rsa", truncate_slug(&slugify(&h.text)));
        }
    }
    if let Some(name) = a.ad.name.as_deref().filter(|s| !s.trim().is_empty()) {
        return format!("{}_rsa", truncate_slug(&slugify(name)));
    }
    "rsa".to_string()
}

type CreativeKey = (Option<String>, AssetKey, AssetKey);

fn plan_fold(input: &ExportInput) -> FoldPlan {
    let mut plan = FoldPlan::default();
    let mut names = NameAllocator::default();

    // Group RSA-bearing ads by creative content (name + headlines + descriptions);
    // `final_urls` / `path1` / `path2` are deliberately excluded — they become
    // per-instance overrides (#58) so URL-variant ads collapse onto one template.
    let mut order: Vec<CreativeKey> = Vec::new();
    let mut members: HashMap<CreativeKey, Vec<usize>> = HashMap::new();
    for (i, a) in input.ad_group_ads.iter().enumerate() {
        let Some(rsa) = &a.ad.responsive_search_ad else {
            continue;
        };
        let key: CreativeKey = (
            a.ad.name.clone(),
            asset_key(&rsa.headlines),
            asset_key(&rsa.descriptions),
        );
        if !members.contains_key(&key) {
            order.push(key.clone());
        }
        members.entry(key).or_default().push(i);
    }

    for key in &order {
        let idxs = &members[key];
        if idxs.len() < 2 {
            continue;
        }
        let first = &input.ad_group_ads[idxs[0]];
        let first_rsa = first.ad.responsive_search_ad.as_ref().unwrap();

        let final_uniform = idxs
            .iter()
            .all(|&i| input.ad_group_ads[i].ad.final_urls == first.ad.final_urls);
        let path1_uniform = idxs.iter().all(|&i| {
            rsa_path(input, i, |r| &r.path1) == first_rsa.path1.as_deref()
        });
        let path2_uniform = idxs.iter().all(|&i| {
            rsa_path(input, i, |r| &r.path2) == first_rsa.path2.as_deref()
        });

        let tname = names.allocate("ad_template", &template_base(first));
        plan.templates.push(TemplateDecl {
            name: tname.clone(),
            ad_name: first.ad.name.clone(),
            final_urls: final_uniform.then(|| first.ad.final_urls.clone()),
            headlines: first_rsa.headlines.clone(),
            descriptions: first_rsa.descriptions.clone(),
            path1: path1_uniform.then(|| first_rsa.path1.clone()).flatten(),
            path2: path2_uniform.then(|| first_rsa.path2.clone()).flatten(),
        });

        for &i in idxs {
            let a = &input.ad_group_ads[i];
            let rsa = a.ad.responsive_search_ad.as_ref().unwrap();
            plan.ad_emit.insert(
                i,
                AdOverride {
                    template: tname.clone(),
                    final_urls: (!final_uniform).then(|| a.ad.final_urls.clone()),
                    path1: if path1_uniform { None } else { rsa.path1.clone() },
                    path2: if path2_uniform { None } else { rsa.path2.clone() },
                },
            );
        }
    }

    // Lift RSA arrays used by >= 2 emission sites (templates + still-inline ads) into
    // `locals`. A template emits its array once regardless of how many ads it backs,
    // so the count is per emission site, not per ad.
    let mut h_sites: Vec<AssetKey> = Vec::new();
    let mut d_sites: Vec<AssetKey> = Vec::new();
    for t in &plan.templates {
        h_sites.push(asset_key(&t.headlines));
        d_sites.push(asset_key(&t.descriptions));
    }
    for (i, a) in input.ad_group_ads.iter().enumerate() {
        if plan.ad_emit.contains_key(&i) {
            continue;
        }
        if let Some(rsa) = &a.ad.responsive_search_ad {
            h_sites.push(asset_key(&rsa.headlines));
            d_sites.push(asset_key(&rsa.descriptions));
        }
    }
    lift_locals(&mut names, &h_sites, "headlines", &mut plan.headline_locals, &mut plan.headline_local_by_key);
    lift_locals(&mut names, &d_sites, "descriptions", &mut plan.description_locals, &mut plan.description_local_by_key);

    plan_negative_locals(input, &mut names, &mut plan);
    plan
}

// Campaign negative-keyword text lists shared by >= 2 campaigns become one shared
// `local`, referenced via the compact `negative_keywords { texts = local.x }` form.
// Deliberately NOT a `google_ads_shared_set`: live negatives are per-campaign
// criteria, so emitting a SharedSet would plan as create-set + attach + destroy the
// criteria — a real migration, not the zero-drift representation change a refresh must be.
fn plan_negative_locals(input: &ExportInput, names: &mut NameAllocator, plan: &mut FoldPlan) {
    type GroupKey = (String, String);
    let mut order: Vec<GroupKey> = Vec::new();
    let mut groups: HashMap<GroupKey, Vec<&JsonKeyword>> = HashMap::new();
    for c in &input.campaign_criteria {
        if !c.negative.unwrap_or(false) {
            continue;
        }
        let Some(kw) = &c.target.keyword else { continue };
        let key = (c.campaign.clone(), c.status.clone().unwrap_or_default());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(kw);
    }

    // A group folds only if all its negatives share one match_type (the shape the
    // compact `texts` list collapses to).
    type Combo = (String, Vec<String>);
    let mut candidates: Vec<(GroupKey, Combo)> = Vec::new();
    for key in &order {
        let kws = &groups[key];
        let mt = kws[0].match_type.clone();
        if !kws.iter().all(|k| k.match_type == mt) {
            continue;
        }
        let texts: Vec<String> = kws.iter().map(|k| k.text.clone()).collect();
        candidates.push((key.clone(), (mt, texts)));
    }

    let mut counts: HashMap<&Combo, usize> = HashMap::new();
    for (_, combo) in &candidates {
        *counts.entry(combo).or_default() += 1;
    }

    let mut local_for: HashMap<Combo, String> = HashMap::new();
    for (key, combo) in &candidates {
        if counts[combo] < 2 {
            continue;
        }
        let name = match local_for.get(combo) {
            Some(n) => n.clone(),
            None => {
                let (_, texts) = combo;
                let base = format!("{}_negatives", truncate_slug(&slugify(&texts[0])));
                let n = names.allocate("local", &base);
                local_for.insert(combo.clone(), n.clone());
                plan.negative_locals.push((n.clone(), texts.clone()));
                n
            }
        };
        plan.campaign_negative_fold
            .insert(key.clone(), (name, combo.0.clone()));
    }
}

fn rsa_path<'a>(
    input: &'a ExportInput,
    i: usize,
    f: impl Fn(&'a JsonResponsiveSearchAd) -> &'a Option<String>,
) -> Option<&'a str> {
    input.ad_group_ads[i]
        .ad
        .responsive_search_ad
        .as_ref()
        .and_then(|r| f(r).as_deref())
}

fn lift_locals(
    names: &mut NameAllocator,
    sites: &[AssetKey],
    suffix: &str,
    locals: &mut Vec<(String, AssetKey)>,
    by_key: &mut HashMap<AssetKey, String>,
) {
    let mut counts: HashMap<&AssetKey, usize> = HashMap::new();
    for k in sites {
        if !k.is_empty() {
            *counts.entry(k).or_default() += 1;
        }
    }
    for k in sites {
        if k.is_empty() || counts[k] < 2 || by_key.contains_key(k) {
            continue;
        }
        let base = format!("{}_{suffix}", truncate_slug(&slugify(&k[0].0)));
        let name = names.allocate("local", &base);
        by_key.insert(k.clone(), name.clone());
        locals.push((name, k.clone()));
    }
}

fn partition_campaign_criteria<'a>(
    items: &[&'a JsonCampaignCriterion],
) -> (Vec<Vec<&'a JsonCampaignCriterion>>, Vec<&'a JsonCampaignCriterion>) {
    let mut groups: Vec<Vec<&'a JsonCampaignCriterion>> = Vec::new();
    let mut index: HashMap<(String, Option<String>), usize> = HashMap::new();
    let mut singletons: Vec<&'a JsonCampaignCriterion> = Vec::new();
    for &c in items {
        let is_negative_keyword = c.negative.unwrap_or(false) && c.target.keyword.is_some();
        if is_negative_keyword {
            let key = (c.campaign.clone(), c.status.clone());
            let idx = match index.get(&key) {
                Some(&i) => i,
                None => {
                    let i = groups.len();
                    index.insert(key, i);
                    groups.push(Vec::new());
                    i
                }
            };
            groups[idx].push(c);
        } else {
            singletons.push(c);
        }
    }
    (groups, singletons)
}

fn campaign_negative_group_base(
    group: &[&JsonCampaignCriterion],
    campaign_addr: &HashMap<String, String>,
) -> String {
    let first = group[0];
    let camp_slug = campaign_addr
        .get(&first.campaign)
        .and_then(|s| s.strip_prefix("google_ads_campaign."))
        .unwrap_or(&first.campaign);
    format!("{camp_slug}_negatives")
}

fn write_campaign_negative_group(
    out: &mut String,
    name: &str,
    group: &[&JsonCampaignCriterion],
    campaign_addr: &HashMap<String, String>,
    plan: Option<&FoldPlan>,
) {
    let first = group[0];
    let _ = writeln!(
        out,
        "resource \"google_ads_campaign_criterion\" \"{name}\" {{"
    );
    let camp_ref = match campaign_addr.get(&first.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved campaign {}>\"", first.campaign),
    };
    write_attr(out, 1, "campaign", &camp_ref);
    if let Some(s) = &first.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    let key = (first.campaign.clone(), first.status.clone().unwrap_or_default());
    if let Some((local, match_type)) = plan.and_then(|p| p.campaign_negative_fold.get(&key)) {
        out.push_str("\n  negative_keywords {\n");
        write_attr(out, 2, "texts", &format!("local.{local}"));
        write_attr(out, 2, "match_type", &fmt_string(match_type));
        out.push_str("  }\n");
    } else {
        for c in group {
            if let Some(kw) = &c.target.keyword {
                out.push_str("\n  negative_keyword {\n");
                write_attr(out, 2, "text", &fmt_string(&kw.text));
                write_attr(out, 2, "match_type", &fmt_string(&kw.match_type));
                out.push_str("  }\n");
            }
        }
    }
    out.push_str("}\n\n");
}

fn write_campaign_criterion(
    out: &mut String,
    name: &str,
    c: &JsonCampaignCriterion,
    campaign_addr: &HashMap<String, String>,
    custom_audience_addr: &HashMap<String, String>,
) {
    let _ = writeln!(
        out,
        "resource \"google_ads_campaign_criterion\" \"{name}\" {{"
    );
    let camp_ref = match campaign_addr.get(&c.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved campaign {}>\"", c.campaign),
    };
    write_attr(out, 1, "campaign", &camp_ref);
    if let Some(s) = &c.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    // Negative keywords render through the grouped form; every other exclusion
    // (placement, topic, demographic, audience) is a singleton and has to carry
    // its own polarity or the render round-trips as positive targeting.
    if c.negative.unwrap_or(false) {
        write_attr(out, 1, "negative", "true");
    }
    if let Some(bm) = c.bid_modifier {
        write_attr(out, 1, "bid_modifier", &format_number(bm));
    }
    write_criterion_blocks(out, &c.target, custom_audience_addr);
    out.push_str("}\n\n");
}

/// The `oneof` body of a criterion resource — one block for whichever axis is
/// set. Shared by both criterion resources so a new axis renders the same way
/// wherever it is declared.
fn write_criterion_blocks(
    out: &mut String,
    c: &JsonCriterion,
    custom_audience_addr: &HashMap<String, String>,
) {
    if let Some(kw) = &c.keyword {
        write_keyword(out, kw);
    }
    if let Some(loc) = &c.location {
        out.push_str("\n  location {\n");
        write_attr(
            out,
            2,
            "geo_target_constant",
            &fmt_string(&loc.geo_target_constant),
        );
        out.push_str("  }\n");
    }
    if let Some(lang) = &c.language {
        out.push_str("\n  language {\n");
        write_attr(
            out,
            2,
            "language_constant",
            &fmt_string(&lang.language_constant),
        );
        out.push_str("  }\n");
    }
    if let Some(prox) = &c.proximity {
        out.push_str("\n  proximity {\n");
        write_attr(out, 2, "latitude", &format_number(prox.latitude));
        write_attr(out, 2, "longitude", &format_number(prox.longitude));
        write_attr(out, 2, "radius", &format_number(prox.radius));
        write_attr(out, 2, "radius_units", &fmt_string(&prox.radius_units));
        out.push_str("  }\n");
    }
    if let Some(dev) = &c.device {
        out.push_str("\n  device {\n");
        write_attr(out, 2, "type", &fmt_string(&dev.ty));
        out.push_str("  }\n");
    }
    if let Some(ch) = &c.youtube_channel {
        out.push_str("\n  youtube_channel {\n");
        write_attr(out, 2, "channel_id", &fmt_string(&ch.channel_id));
        out.push_str("  }\n");
    }
    if let Some(v) = &c.youtube_video {
        out.push_str("\n  youtube_video {\n");
        write_attr(out, 2, "video_id", &fmt_string(&v.video_id));
        out.push_str("  }\n");
    }
    if let Some(t) = &c.topic {
        out.push_str("\n  topic {\n");
        write_attr(out, 2, "topic_constant", &fmt_string(&t.topic_constant));
        out.push_str("  }\n");
    }
    if let Some(p) = &c.placement {
        out.push_str("\n  placement {\n");
        write_attr(out, 2, "url", &fmt_string(&p.url));
        out.push_str("  }\n");
    }
    if let Some(u) = &c.user_interest {
        out.push_str("\n  user_interest {\n");
        write_attr(
            out,
            2,
            "user_interest_category",
            &fmt_string(&u.user_interest_category),
        );
        out.push_str("  }\n");
    }
    if let Some(a) = &c.age_range {
        out.push_str("\n  age_range {\n");
        write_attr(out, 2, "type", &fmt_string(&a.ty));
        out.push_str("  }\n");
    }
    if let Some(g) = &c.gender {
        out.push_str("\n  gender {\n");
        write_attr(out, 2, "type", &fmt_string(&g.ty));
        out.push_str("  }\n");
    }
    if let Some(p) = &c.parental_status {
        out.push_str("\n  parental_status {\n");
        write_attr(out, 2, "type", &fmt_string(&p.ty));
        out.push_str("  }\n");
    }
    if let Some(i) = &c.income_range {
        out.push_str("\n  income_range {\n");
        write_attr(out, 2, "type", &fmt_string(&i.ty));
        out.push_str("  }\n");
    }
    if let Some((field, value)) = c.audience.as_ref().and_then(JsonAudience::source) {
        out.push_str("\n  audience {\n");
        let rendered = if field == "custom_audience" {
            custom_audience_ref(value, custom_audience_addr)
        } else {
            fmt_string(value)
        };
        write_attr(out, 2, field, &rendered);
        out.push_str("  }\n");
    }
}

/// A declared `google_ads_custom_audience.<name>.id` reference when the target
/// is one bidsmith renders, else the raw resource name.
fn custom_audience_ref(value: &str, custom_audience_addr: &HashMap<String, String>) -> String {
    let bare = value.rsplit('/').next().unwrap_or(value);
    match custom_audience_addr
        .get(value)
        .or_else(|| custom_audience_addr.get(bare))
    {
        Some(addr) => format!("{addr}.id"),
        None => fmt_string(value),
    }
}

fn write_custom_audience(out: &mut String, name: &str, a: &JsonCustomAudience) {
    let _ = writeln!(out, "resource \"google_ads_custom_audience\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&a.name));
    if let Some(d) = &a.description {
        write_attr(out, 1, "description", &fmt_string(d));
    }
    if let Some(t) = &a.ty {
        write_attr(out, 1, "type", &fmt_string(t));
    }
    if let Some(s) = &a.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    for m in &a.members {
        let Some((field, _, value)) = m.payload() else {
            continue;
        };
        out.push_str("\n  member {\n");
        write_attr(out, 2, field, &fmt_string(value));
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn write_conversion_action(out: &mut String, name: &str, c: &JsonConversionAction) {
    let _ = writeln!(out, "resource \"google_ads_conversion_action\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&c.name));
    write_attr(out, 1, "type", &fmt_string(&c.ty));
    write_attr(out, 1, "category", &fmt_string(&c.category));
    if let Some(s) = &c.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    if let Some(ct) = &c.counting_type {
        write_attr(out, 1, "counting_type", &fmt_string(ct));
    }
    if let Some(d) = c.click_through_lookback_window_days {
        write_attr(out, 1, "click_through_lookback_window_days", &d.to_string());
    }
    if let Some(d) = c.view_through_lookback_window_days {
        write_attr(out, 1, "view_through_lookback_window_days", &d.to_string());
    }
    if let Some(vs) = &c.value_settings {
        out.push_str("\n  value_settings {\n");
        if let Some(v) = vs.default_value {
            write_attr(out, 2, "default_value", &format_number(v));
        }
        if let Some(s) = &vs.default_currency_code {
            write_attr(out, 2, "default_currency_code", &fmt_string(s));
        }
        if let Some(b) = vs.always_use_default_value {
            write_attr(out, 2, "always_use_default_value", &b.to_string());
        }
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn write_call_asset(
    out: &mut String,
    name: &str,
    a: &JsonCallAsset,
    conversion_action_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_call_asset\" \"{name}\" {{");
    write_attr(out, 1, "country_code", &fmt_string(&a.country_code));
    write_attr(out, 1, "phone_number", &fmt_string(&a.phone_number));
    if let Some(s) = &a.call_conversion_reporting_state {
        write_attr(out, 1, "call_conversion_reporting_state", &fmt_string(s));
    }
    if let Some(action) = &a.call_conversion_action {
        let bare_id = action.rsplit('/').next().unwrap_or(action);
        let action_ref = match conversion_action_addr
            .get(action)
            .or_else(|| conversion_action_addr.get(bare_id))
        {
            Some(addr) => format!("{addr}.id"),
            None => fmt_string(action),
        };
        write_attr(out, 1, "call_conversion_action", &action_ref);
    }
    out.push_str("}\n\n");
}

fn write_sitelink_asset(out: &mut String, name: &str, a: &JsonSitelinkAsset) {
    let _ = writeln!(out, "resource \"google_ads_sitelink_asset\" \"{name}\" {{");
    write_attr(out, 1, "link_text", &fmt_string(&a.link_text));
    if let Some(s) = &a.description1 {
        write_attr(out, 1, "description1", &fmt_string(s));
    }
    if let Some(s) = &a.description2 {
        write_attr(out, 1, "description2", &fmt_string(s));
    }
    write_attr(out, 1, "final_urls", &fmt_string_list(&a.final_urls));
    out.push_str("}\n\n");
}

fn write_callout_asset(out: &mut String, name: &str, a: &JsonCalloutAsset) {
    let _ = writeln!(out, "resource \"google_ads_callout_asset\" \"{name}\" {{");
    write_attr(out, 1, "text", &fmt_string(&a.text));
    out.push_str("}\n\n");
}

fn write_structured_snippet_asset(out: &mut String, name: &str, a: &JsonStructuredSnippetAsset) {
    let _ = writeln!(
        out,
        "resource \"google_ads_structured_snippet_asset\" \"{name}\" {{"
    );
    write_attr(out, 1, "header", &fmt_string(&a.header));
    write_attr(out, 1, "values", &fmt_string_list(&a.values));
    out.push_str("}\n\n");
}

fn asset_reference(asset_addr: &HashMap<String, String>, asset: &str) -> String {
    match asset_addr.get(asset) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved asset {asset}>\""),
    }
}

fn write_customer_asset(
    out: &mut String,
    name: &str,
    a: &JsonCustomerAsset,
    asset_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_customer_asset\" \"{name}\" {{");
    let asset_ref = asset_reference(asset_addr, &a.asset);
    write_attr(out, 1, "asset", &asset_ref);
    write_inferable_field_type(out, &asset_ref, &a.field_type);
    if let Some(s) = &a.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    out.push_str("}\n\n");
}

fn write_campaign_asset(
    out: &mut String,
    name: &str,
    a: &JsonCampaignAsset,
    campaign_addr: &HashMap<String, String>,
    asset_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_campaign_asset\" \"{name}\" {{");
    let campaign_ref = match campaign_addr.get(&a.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => fmt_string(&a.campaign),
    };
    write_attr(out, 1, "campaign", &campaign_ref);
    let asset_ref = asset_reference(asset_addr, &a.asset);
    write_attr(out, 1, "asset", &asset_ref);
    write_inferable_field_type(out, &asset_ref, &a.field_type);
    if let Some(s) = &a.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    out.push_str("}\n\n");
}

/// `field_type` is 1:1 with the asset's resource type, so emitting it adds a
/// line that can only ever repeat what the reference already says. Anything
/// unusual (or an unresolved reference) still renders it.
fn write_inferable_field_type(out: &mut String, asset_ref: &str, field_type: &str) {
    let ty = asset_ref.split('.').next().unwrap_or("");
    if crate::schema::field_type_for_asset(ty) == Some(field_type) {
        return;
    }
    write_attr(out, 1, "field_type", &fmt_string(field_type));
}

fn write_ad_group_asset(
    out: &mut String,
    name: &str,
    a: &JsonAdGroupAsset,
    ad_group_addr: &HashMap<String, String>,
    asset_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_ad_group_asset\" \"{name}\" {{");
    let ad_group_ref = match ad_group_addr.get(&a.ad_group) {
        Some(addr) => format!("{addr}.id"),
        None => fmt_string(&a.ad_group),
    };
    write_attr(out, 1, "ad_group", &ad_group_ref);
    let asset_ref = asset_reference(asset_addr, &a.asset);
    write_attr(out, 1, "asset", &asset_ref);
    write_inferable_field_type(out, &asset_ref, &a.field_type);
    if let Some(s) = &a.status {
        write_attr(out, 1, "status", &fmt_string(s));
    }
    out.push_str("}\n\n");
}

fn write_shared_set(out: &mut String, name: &str, s: &JsonSharedSet, extra_members: &[&JsonKeyword]) {
    let _ = writeln!(out, "resource \"google_ads_shared_set\" \"{name}\" {{");
    write_attr(out, 1, "name", &fmt_string(&s.name));
    if let Some(t) = &s.ty {
        write_attr(out, 1, "type", &fmt_string(t));
    }
    if let Some(st) = &s.status {
        write_attr(out, 1, "status", &fmt_string(st));
    }
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    for kw in s.negative_keywords.iter().chain(extra_members.iter().copied()) {
        if !seen.insert((kw.text.as_str(), kw.match_type.as_str())) {
            continue;
        }
        out.push_str("\n  negative_keyword {\n");
        write_attr(out, 2, "text", &fmt_string(&kw.text));
        write_attr(out, 2, "match_type", &fmt_string(&kw.match_type));
        out.push_str("  }\n");
    }
    out.push_str("}\n\n");
}

fn write_shared_criterion(
    out: &mut String,
    name: &str,
    c: &JsonSharedCriterion,
    shared_set_addr: &HashMap<String, String>,
) {
    let _ = writeln!(out, "resource \"google_ads_shared_criterion\" \"{name}\" {{");
    let shared_set_ref = match shared_set_addr.get(&c.shared_set) {
        Some(addr) => format!("{addr}.id"),
        None => fmt_string(&c.shared_set),
    };
    write_attr(out, 1, "shared_set", &shared_set_ref);
    out.push_str("\n  keyword {\n");
    write_attr(out, 2, "text", &fmt_string(&c.keyword.text));
    write_attr(out, 2, "match_type", &fmt_string(&c.keyword.match_type));
    out.push_str("  }\n");
    out.push_str("}\n\n");
}

// Members can arrive both folded into the set and as standalone criteria; render the deduped union inline, only external sets standalone.
fn write_shared_sets_and_criteria(
    out: &mut String,
    input: &ExportInput,
    names: &mut NameAllocator,
    shared_set_addr: &mut HashMap<String, String>,
) {
    let mut members_by_set: HashMap<&str, Vec<&JsonKeyword>> = HashMap::new();
    for c in &input.shared_criteria {
        members_by_set
            .entry(c.shared_set.as_str())
            .or_default()
            .push(&c.keyword);
    }
    for s in &input.shared_sets {
        let name = names.allocate("google_ads_shared_set", &slugify(&s.name));
        shared_set_addr.insert(s.id.clone(), format!("google_ads_shared_set.{name}"));
        let extra = members_by_set
            .get(s.id.as_str())
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        write_shared_set(out, &name, s, extra);
    }
    let rendered: HashSet<&str> = input.shared_sets.iter().map(|s| s.id.as_str()).collect();
    for c in &input.shared_criteria {
        if rendered.contains(c.shared_set.as_str()) {
            continue;
        }
        let name = names.allocate("google_ads_shared_criterion", &slugify(&c.keyword.text));
        write_shared_criterion(out, &name, c, shared_set_addr);
    }
}

fn write_campaign_shared_set(
    out: &mut String,
    name: &str,
    s: &JsonCampaignSharedSet,
    campaign_addr: &HashMap<String, String>,
    shared_set_addr: &HashMap<String, String>,
) {
    let _ = writeln!(
        out,
        "resource \"google_ads_campaign_shared_set\" \"{name}\" {{"
    );
    let campaign_ref = match campaign_addr.get(&s.campaign) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved campaign {}>\"", s.campaign),
    };
    write_attr(out, 1, "campaign", &campaign_ref);
    let shared_set_ref = match shared_set_addr.get(&s.shared_set) {
        Some(addr) => format!("{addr}.id"),
        None => format!("\"<unresolved shared_set {}>\"", s.shared_set),
    };
    write_attr(out, 1, "shared_set", &shared_set_ref);
    if let Some(st) = &s.status {
        write_attr(out, 1, "status", &fmt_string(st));
    }
    out.push_str("}\n\n");
}

fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 2f64.powi(53) {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn write_keyword(out: &mut String, kw: &JsonKeyword) {
    out.push_str("\n  keyword {\n");
    write_attr(out, 2, "text", &fmt_string(&kw.text));
    write_attr(out, 2, "match_type", &fmt_string(&kw.match_type));
    out.push_str("  }\n");
}

fn ad_ad_base(a: &JsonAdGroupAd, ad_group_addr: &HashMap<String, String>) -> String {
    if let Some(name) = a
        .ad
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return slugify(name);
    }
    if let Some(slug) = ad_group_addr
        .get(&a.ad_group)
        .and_then(|s| s.strip_prefix("google_ads_ad_group."))
        .filter(|s| !s.is_empty())
    {
        let suffix = if a.ad.responsive_search_ad.is_some() {
            "rsa"
        } else {
            "ad"
        };
        return format!("{slug}_{suffix}");
    }
    slugify(&a.id)
}

fn criterion_base(c: &JsonCriterion, fallback: &str) -> String {
    if let Some(kw) = &c.keyword {
        return format!("{}_{}", kw.match_type.to_ascii_lowercase(), kw.text);
    }
    if let Some(loc) = &c.location {
        return format!("location_{}", loc.geo_target_constant);
    }
    if let Some(lang) = &c.language {
        return format!("language_{}", lang.language_constant);
    }
    if let Some(prox) = &c.proximity {
        return format!(
            "proximity_{}_{}",
            prox.radius_units.to_ascii_lowercase(),
            format_number(prox.radius),
        );
    }
    if let Some(dev) = &c.device {
        return format!("device_{}", dev.ty.to_ascii_lowercase());
    }
    if let Some(ch) = &c.youtube_channel {
        return format!("channel_{}", ch.channel_id);
    }
    if let Some(v) = &c.youtube_video {
        return format!("video_{}", v.video_id);
    }
    if let Some(t) = &c.topic {
        return format!("topic_{}", last_segment(&t.topic_constant));
    }
    if let Some(p) = &c.placement {
        return format!("placement_{}", p.url);
    }
    if let Some(u) = &c.user_interest {
        return format!("interest_{}", last_segment(&u.user_interest_category));
    }
    if let Some(a) = &c.age_range {
        return a.ty.to_ascii_lowercase();
    }
    if let Some(g) = &c.gender {
        return format!("gender_{}", g.ty.to_ascii_lowercase());
    }
    if let Some(p) = &c.parental_status {
        return format!("parental_status_{}", p.ty.to_ascii_lowercase());
    }
    if let Some(i) = &c.income_range {
        return i.ty.to_ascii_lowercase();
    }
    if let Some((field, value)) = c.audience.as_ref().and_then(JsonAudience::source) {
        return format!("{field}_{}", last_segment(value));
    }
    fallback.to_string()
}

fn last_segment(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s)
}

fn fmt_rsa_asset_list(assets: &[JsonRsaAsset]) -> String {
    let mut out = String::from("[");
    for (i, asset) in assets.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        match &asset.pin {
            None => out.push_str(&fmt_string(&asset.text)),
            Some(pin) => {
                out.push_str("{ text = ");
                out.push_str(&fmt_string(&asset.text));
                out.push_str(", pin = ");
                out.push_str(&fmt_string(pin));
                out.push_str(" }");
            }
        }
    }
    out.push(']');
    out
}

fn fmt_string_list(items: &[String]) -> String {
    let mut out = String::from("[");
    for (i, s) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(&fmt_string(s));
    }
    out.push(']');
    out
}

fn write_attr(out: &mut String, indent: usize, name: &str, value: &str) {
    for _ in 0..indent {
        out.push_str("  ");
    }
    let _ = writeln!(out, "{name} = {value}");
}

pub fn fmt_string(s: &str) -> String {
    let mut escaped = String::with_capacity(s.len() + 2);
    escaped.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_underscore = true;
    for ch in s.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("resource");
    }
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        out.insert(0, '_');
    }
    out
}

#[derive(Default)]
struct NameAllocator {
    by_type: HashMap<String, HashSet<String>>,
}

impl NameAllocator {
    fn allocate(&mut self, resource_type: &str, base: &str) -> String {
        let set = self.by_type.entry(resource_type.to_string()).or_default();
        if set.insert(base.to_string()) {
            return base.to_string();
        }
        let mut i = 2;
        loop {
            let candidate = format!("{base}_{i}");
            if set.insert(candidate.clone()) {
                return candidate;
            }
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::adapt::from_search_response;

    const FULL_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget A", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Camp A", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "conversionAction": { "resourceName": "customers/9/conversionActions/3001", "id": "3001", "name": "Lead", "type": "WEBPAGE", "category": "SUBMIT_LEAD_FORM", "status": "ENABLED" } },
                { "asset": { "resourceName": "customers/9/assets/4001", "id": "4001", "callAsset": { "countryCode": "PL", "phoneNumber": "510019081" } } },
                { "customerAsset": { "resourceName": "customers/9/customerAssets/4001~CALL", "asset": "customers/9/assets/4001", "fieldType": "CALL", "status": "ENABLED" } }
            ]
        }
    ]"#;

    // Four ads, two distinct creatives. The first two share headlines+descriptions
    // but differ by final_urls/path1 (→ one URL-agnostic template + per-instance
    // overrides). The second two are byte-identical (→ one template, no overrides).
    // Both creatives reuse the same headline set (→ a lifted `local`).
    const FOLD_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Privacy Search", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Chrome", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3002", "id": "3002", "name": "Safari", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3003", "id": "3003", "name": "Edge", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3004", "id": "3004", "name": "Brave", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3001~4001", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com/chrome"], "responsiveSearchAd": { "headlines": [{"text": "Block Ads Now"}, {"text": "Privacy First Browser"}, {"text": "Stop Trackers Fast"}], "descriptions": [{"text": "Fast private browsing."}, {"text": "Trusted by millions."}], "path1": "chrome" } } } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3002~4002", "adGroup": "customers/9/adGroups/3002", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com/safari"], "responsiveSearchAd": { "headlines": [{"text": "Block Ads Now"}, {"text": "Privacy First Browser"}, {"text": "Stop Trackers Fast"}], "descriptions": [{"text": "Fast private browsing."}, {"text": "Trusted by millions."}], "path1": "safari" } } } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3003~4003", "adGroup": "customers/9/adGroups/3003", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com/get"], "responsiveSearchAd": { "headlines": [{"text": "Block Ads Now"}, {"text": "Privacy First Browser"}, {"text": "Stop Trackers Fast"}], "descriptions": [{"text": "Switch in one click."}, {"text": "Free and open source."}], "path1": "get" } } } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3004~4004", "adGroup": "customers/9/adGroups/3004", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com/get"], "responsiveSearchAd": { "headlines": [{"text": "Block Ads Now"}, {"text": "Privacy First Browser"}, {"text": "Stop Trackers Fast"}], "descriptions": [{"text": "Switch in one click."}, {"text": "Free and open source."}], "path1": "get" } } } }
            ]
        }
    ]"#;

    // Two campaigns carrying the same three-term negative list (all BROAD) →
    // one lifted `local`, referenced via the compact `negative_keywords` form on each.
    const NEG_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Search One", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2002", "id": "2002", "name": "Search Two", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~6001", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "negative": true, "keyword": { "text": "free", "matchType": "BROAD" } } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~6002", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "negative": true, "keyword": { "text": "crack", "matchType": "BROAD" } } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~6003", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "negative": true, "keyword": { "text": "torrent", "matchType": "BROAD" } } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2002~6001", "campaign": "customers/9/campaigns/2002", "status": "ENABLED", "negative": true, "keyword": { "text": "free", "matchType": "BROAD" } } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2002~6002", "campaign": "customers/9/campaigns/2002", "status": "ENABLED", "negative": true, "keyword": { "text": "crack", "matchType": "BROAD" } } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2002~6003", "campaign": "customers/9/campaigns/2002", "status": "ENABLED", "negative": true, "keyword": { "text": "torrent", "matchType": "BROAD" } } }
            ]
        }
    ]"#;

    fn assert_fold_roundtrips(raw: &str) {
        let input = from_search_response(raw).expect("adapter");
        let folded = canonicalize(&render(&input));
        let pf = crate::parser::parse_str(std::path::Path::new("rt.bid"), &folded)
            .expect("folded parses");
        let mut input2 = crate::api::import::import_files(
            std::slice::from_ref(&pf),
            &crate::schema::InputBindings::default(),
        )
        .expect("folded imports")
        .input;
        // Import resolves the provider target from ambient config (env / bidsmith.toml);
        // folding never touches the provider, so normalize it out of the comparison.
        input2.customer_id = input.customer_id.clone();
        input2.login_customer_id = input.login_customer_id.clone();
        // The folded tree must mean exactly what the input did: its verbose (unfolded)
        // re-render is byte-identical to the input's. This is the offline stand-in for
        // "plan reports zero drift after a refresh".
        let v1 = canonicalize(&render_inner(&input, false));
        let v2 = canonicalize(&render_inner(&input2, false));
        assert_eq!(
            v1, v2,
            "fold did not round-trip\n=== folded ===\n{folded}\n=== verbose(original) ===\n{v1}\n=== verbose(roundtrip) ===\n{v2}"
        );
    }

    #[test]
    fn frequency_caps_render_as_repeated_blocks() {
        let raw = r#"[{"results":[
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Preroll", "status": "PAUSED", "advertisingChannelType": "VIDEO", "campaignBudget": "customers/9/campaignBudgets/1001", "frequencyCaps": [
                { "key": { "level": "CAMPAIGN", "eventType": "IMPRESSION", "timeUnit": "DAY", "timeLength": 1 }, "cap": 3 },
                { "key": { "level": "AD_GROUP", "eventType": "VIDEO_VIEW", "timeUnit": "WEEK", "timeLength": 1 }, "cap": 2 }
            ] } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        assert_eq!(input.campaigns[0].frequency_caps.len(), 2);
        let out = render(&input);
        assert_eq!(out.matches("frequency_caps {").count(), 2, "{out}");
        assert!(out.contains("event_type = \"IMPRESSION\""), "{out}");
        assert!(out.contains("cap = 3"), "{out}");
        // The default level stays implicit; a non-default one is written out.
        assert_eq!(out.matches("level = ").count(), 1, "{out}");
        assert!(out.contains("level = \"AD_GROUP\""), "{out}");
    }

    /// Pulling an account is how the opt-out gets into the repo in the first
    /// place, so what Google reports has to come back as a block someone can
    /// commit and CI can hold to (issue #152).
    #[test]
    fn asset_automation_renders_as_the_block_that_declares_it() {
        let raw = r#"[{"results":[
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Brand", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001", "assetAutomationSettings": [
                { "assetAutomationType": "FINAL_URL_EXPANSION_TEXT_ASSET_AUTOMATION", "assetAutomationStatus": "OPTED_OUT" },
                { "assetAutomationType": "TEXT_ASSET_AUTOMATION", "assetAutomationStatus": "OPTED_IN" }
            ] } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        let out = render(&input);
        assert_eq!(out.matches("asset_automation_settings {").count(), 1, "{out}");
        assert!(out.contains("text_asset_automation = \"OPTED_IN\""), "{out}");
        assert!(
            out.contains("final_url_expansion_text_asset_automation = \"OPTED_OUT\""),
            "{out}"
        );
    }

    /// Pulling an account is how a DSA setting nobody declared gets into the
    /// repo, which is the first step to managing it (issue #159).
    #[test]
    fn dynamic_search_ads_renders_as_the_block_that_declares_it() {
        let raw = r#"[{"results":[
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Site wide", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001", "dynamicSearchAdsSetting": { "domainName": "example.com", "languageCode": "en", "useSuppliedUrlsOnly": false } } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        let out = render(&input);
        assert!(out.contains("dynamic_search_ads_setting {"), "{out}");
        assert!(out.contains("domain_name = \"example.com\""), "{out}");
        assert!(out.contains("language_code = \"en\""), "{out}");
        assert!(out.contains("use_supplied_urls_only = false"), "{out}");
    }

    /// Pulling an account that has AI Max explicitly off is how the opt-out
    /// reaches the repo, on both halves of the setting (issue #158).
    #[test]
    fn ai_max_renders_as_the_blocks_that_declare_it() {
        let raw = r#"[{"results":[
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Brand", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001", "aiMaxSetting": { "enableAiMax": false } } },
            { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Brand terms", "campaign": "customers/9/campaigns/2001", "type": "SEARCH_STANDARD", "aiMaxAdGroupSetting": { "disableSearchTermMatching": true } } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        let out = render(&input);
        assert!(out.contains("ai_max_setting {"), "{out}");
        assert!(out.contains("enable_ai_max = false"), "{out}");
        assert!(out.contains("ai_max_ad_group_setting {"), "{out}");
        assert!(out.contains("disable_search_term_matching = true"), "{out}");
    }

    /// A campaign the account never set AI Max on has nothing to render — an
    /// empty block would read as a declaration nobody made.
    #[test]
    fn an_unset_ai_max_renders_nothing() {
        let raw = r#"[{"results":[
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Brand", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        let out = render(&input);
        assert!(!out.contains("ai_max_setting"), "{out}");
    }

    /// The automations this build has no attribute for are remembered off the
    /// live campaign so the write can put them back, but there is no name to
    /// render them under — so a campaign carrying only those renders no block
    /// at all, rather than an empty one or an invented attribute.
    #[test]
    fn an_automation_with_no_attribute_renders_nothing() {
        let raw = r#"[{"results":[
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Brand", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001", "assetAutomationSettings": [
                { "assetAutomationType": "GENERATE_VERTICAL_YOUTUBE_VIDEOS", "assetAutomationStatus": "OPTED_OUT" }
            ] } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        assert_eq!(
            input.campaigns[0]
                .asset_automation_settings
                .as_ref()
                .map(|s| s.unmodelled.len()),
            Some(1)
        );
        let out = render(&input);
        assert!(!out.contains("asset_automation_settings"), "{out}");
        assert!(!out.contains("GENERATE_VERTICAL_YOUTUBE_VIDEOS"), "{out}");
    }

    /// `owns` has no live counterpart — nothing on the account records it —
    /// so it survives a round trip only through the declared-JSON path.
    #[test]
    fn a_campaigns_automation_claim_round_trips() {
        let input: ExportInput = serde_json::from_value(serde_json::json!({
            "customer_id": "9",
            "campaign_budgets": [{"id": "m.b", "name": "B", "amount_micros": 5000000}],
            "campaigns": [{
                "id": "m.c",
                "name": "Brand",
                "advertising_channel_type": "SEARCH",
                "campaign_budget": "m.b",
                "owns_automatic_assets": true
            }]
        }))
        .expect("valid ExportInput");
        let out = canonicalize(&render(&input));
        assert!(out.contains(r#"owns = ["automatically_created_assets"]"#), "{out}");
    }

    /// Google fills in a restriction for every dimension it has an opinion
    /// about, so what reads back is only the part that says something an absent
    /// entry would not — otherwise every ad group carries a dozen lines nobody
    /// wrote and nobody can act on (issue #135).
    #[test]
    fn only_the_restrictions_that_say_something_render() {
        let raw = r#"[{"results":[
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Brand", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
            { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Observed", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "targetingSetting": { "targetRestrictions": [
                { "targetingDimension": "AGE_RANGE", "bidOnly": true },
                { "targetingDimension": "GENDER", "bidOnly": false },
                { "targetingDimension": "KEYWORD", "bidOnly": false }
            ] } } },
            { "adGroup": { "resourceName": "customers/9/adGroups/3002", "id": "3002", "name": "Plain", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "targetingSetting": { "targetRestrictions": [
                { "targetingDimension": "AGE_RANGE", "bidOnly": false }
            ] } } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        let out = render(&input);

        assert_eq!(out.matches("targeting_setting {").count(), 1, "{out}");
        assert_eq!(out.matches("target_restriction {").count(), 1, "{out}");
        assert!(out.contains("targeting_dimension = \"AGE_RANGE\""), "{out}");
        assert!(out.contains("bid_only = true"), "{out}");
        // An all-defaults ad group reads as one that says nothing at all.
        assert!(
            input.ad_groups[1].targeting_setting.is_none(),
            "an all-default live setting must read as absent"
        );
    }

    #[test]
    fn video_targeting_renders_and_links_the_declared_segment() {
        let raw = r#"[{"results":[
            { "customAudience": { "resourceName": "customers/9/customAudiences/501", "id": "501", "name": "Ad blocker searchers", "type": "SEARCH", "status": "ENABLED", "members": [
                { "memberType": "KEYWORD", "keyword": "ad blocker" },
                { "memberType": "URL", "url": "https://example.com/privacy" }
            ] } },
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Preroll", "status": "PAUSED", "advertisingChannelType": "VIDEO", "campaignBudget": "customers/9/campaignBudgets/1001" } },
            { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~1", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "negative": false, "customAudience": { "customAudience": "customers/9/customAudiences/501" } } },
            { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~2", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "negative": true, "youtubeChannel": { "channelId": "UCabc" } } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        let out = render(&input);

        assert_eq!(out.matches("member {").count(), 2, "{out}");
        assert!(out.contains("keyword = \"ad blocker\""), "{out}");
        // The criterion links the rendered segment by address, not by the
        // resource name a fresh account would not have.
        assert!(
            out.contains("custom_audience = google_ads_custom_audience.ad_blocker_searchers.id"),
            "{out}"
        );
        // A singleton exclusion has to carry its own polarity.
        assert!(out.contains("negative = true"), "{out}");
        assert!(out.contains("channel_id = \"UCabc\""), "{out}");
    }

    #[test]
    fn ad_group_targeting_renders_one_resource_per_axis() {
        // Issue #110: one video campaign, one ad group per cohort. Keywords
        // still group; every other axis renders as its own resource.
        let raw = r#"[{"results":[
            { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
            { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Preroll", "status": "PAUSED", "advertisingChannelType": "VIDEO", "campaignBudget": "customers/9/campaignBudgets/1001" } },
            { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Cohort 35 up", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
            { "adGroupCriterion": { "resourceName": "customers/9/adGroupCriteria/3001~1", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "negative": false, "bidModifier": 1.2, "ageRange": { "type": "AGE_RANGE_35_44" } } },
            { "adGroupCriterion": { "resourceName": "customers/9/adGroupCriteria/3001~2", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "negative": true, "placement": { "url": "https://example.com/x" } } },
            { "adGroupCriterion": { "resourceName": "customers/9/adGroupCriteria/3001~3", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "negative": false, "userList": { "userList": "customers/9/userLists/987" } } },
            { "adGroupCriterion": { "resourceName": "customers/9/adGroupCriteria/3001~4", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "negative": false, "location": { "geoTargetConstant": "geoTargetConstants/2702" } } }
        ]}]"#;
        let input = from_search_response(raw).expect("adapter");
        let out = render(&input);

        assert_eq!(
            out.matches("resource \"google_ads_ad_group_criterion\"").count(),
            4,
            "{out}"
        );
        assert!(out.contains("bid_modifier = 1.2"), "{out}");
        assert!(out.contains("age_range {"), "{out}");
        assert!(out.contains("url = \"https://example.com/x\""), "{out}");
        assert!(out.contains("user_list = \"customers/9/userLists/987\""), "{out}");
        assert!(out.contains("geo_target_constant = \"geoTargetConstants/2702\""), "{out}");
        // A singleton exclusion has to carry its own polarity.
        assert_eq!(out.matches("negative = true").count(), 1, "{out}");
        assert_fold_roundtrips(raw);
    }

    #[test]
    fn video_notice_fires_only_on_video_content() {
        // A VIDEO campaign with no video of its own is a scaffold like any
        // other campaign — nothing about uploading applies to it.
        let plain: ExportInput = serde_json::from_value(json!({
            "customer_id": "1",
            "campaigns": [{ "id": "c", "name": "Preroll", "advertising_channel_type": "VIDEO", "campaign_budget": "b" }]
        }))
        .unwrap();
        assert!(video_upload_notice(&plain).is_none());

        let video: ExportInput = serde_json::from_value(json!({
            "customer_id": "1",
            "youtube_video_assets": [{ "id": "v", "youtube_video_id": "abc12345678" }],
            "campaigns": [{ "id": "c", "name": "Preroll", "advertising_channel_type": "VIDEO", "campaign_budget": "b" }]
        }))
        .unwrap();
        let notice = video_upload_notice(&video).expect("video content triggers the notice");
        assert!(notice.contains("cannot upload"));
        assert!(notice.contains("1 youtube video asset(s)"));
        assert!(!notice.contains("bidsmith refresh"));
    }

    use serde_json::json;

    #[test]
    fn video_asset_and_video_ad_round_trip() {
        let input: ExportInput = serde_json::from_value(json!({
            "customer_id": "9",
            "youtube_video_assets": [
                { "id": "m.google_ads_youtube_video_asset.brand", "youtube_video_id": "dQw4w9WgXcQ", "youtube_video_title": "Brand 12s" }
            ],
            "campaign_budgets": [{ "id": "m.google_ads_campaign_budget.b", "name": "Preroll", "amount_micros": 10000000 }],
            "campaigns": [{ "id": "m.google_ads_campaign.c", "name": "Preroll", "advertising_channel_type": "VIDEO", "campaign_budget": "m.google_ads_campaign_budget.b", "status": "PAUSED" }],
            "ad_groups": [{ "id": "m.google_ads_ad_group.g", "name": "In-stream", "campaign": "m.google_ads_campaign.c", "type": "VIDEO_TRUE_VIEW_IN_STREAM" }],
            "ad_group_ads": [{
                "id": "m.google_ads_ad_group_ad.ad",
                "ad_group": "m.google_ads_ad_group.g",
                "status": "PAUSED",
                "ad": {
                    "name": "Preroll",
                    "final_urls": ["https://example.com"],
                    "video_responsive_ad": {
                        "video": "m.google_ads_youtube_video_asset.brand",
                        "headlines": ["Block Ads"],
                        "call_to_actions": ["Install"]
                    }
                }
            }]
        }))
        .unwrap();

        let rendered = canonicalize(&render(&input));
        assert!(rendered.contains("resource \"google_ads_youtube_video_asset\""), "{rendered}");
        assert!(rendered.contains("video_responsive_ad {"), "{rendered}");

        let pf = crate::parser::parse_str(std::path::Path::new("video.bid"), &rendered)
            .expect("rendered video parses");
        let diags = crate::schema::validate_files(
            std::slice::from_ref(&pf),
            &crate::schema::InputBindings::default(),
        );
        assert!(
            diags.iter().all(|d| !d.is_error()),
            "validate errors: {:?}",
            diags.iter().filter(|d| d.is_error()).map(|d| &d.message).collect::<Vec<_>>()
        );

        let mut input2 = crate::api::import::import_files(
            std::slice::from_ref(&pf),
            &crate::schema::InputBindings::default(),
        )
        .expect("rendered video imports")
        .input;
        input2.customer_id = input.customer_id.clone();
        input2.login_customer_id = input.login_customer_id.clone();

        // Re-render the imported tree: identical means the video asset + video ad
        // body survived render → import → render with no drift.
        assert_eq!(
            canonicalize(&render_inner(&input, false)),
            canonicalize(&render_inner(&input2, false)),
        );
    }

    // The regression for issue #80: a Demand Gen video responsive ad pulled from
    // the live account must round-trip through the search-response adapter and the
    // exporter — headlines, long headlines, descriptions, CTAs, breadcrumbs, and
    // the video asset reference.
    const DEMAND_GEN_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Shorts", "amountMicros": "15000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "GH_Shorts_3", "status": "ENABLED", "advertisingChannelType": "DEMAND_GEN", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Shorts AG", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "asset": { "resourceName": "customers/9/assets/7001", "id": "7001", "youtubeVideoAsset": { "youtubeVideoId": "dQw4w9WgXcQ", "youtubeVideoTitle": "Shorts v2" } } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3001~4001", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "ad": {
                    "name": "Ad 1",
                    "finalUrls": ["https://www.ghostery.com/ghostery-ad-blocker?utm_source=youtube"],
                    "demandGenVideoResponsiveAd": {
                        "headlines": [{"text": "Block trackers"}],
                        "longHeadlines": [{"text": "Block trackers and ads everywhere"}],
                        "descriptions": [{"text": "Private browsing made simple."}],
                        "callToActions": [{"text": "Install"}],
                        "videos": [{"asset": "customers/9/assets/7001"}],
                        "breadcrumb1": "ghostery",
                        "breadcrumb2": "download"
                    }
                } } }
            ]
        }
    ]"#;

    #[test]
    fn adapter_captures_demand_gen_video_ad() {
        let input = from_search_response(DEMAND_GEN_FIXTURE).expect("adapter");

        let dg = input.ad_group_ads[0]
            .ad
            .demand_gen_video_responsive_ad
            .as_ref()
            .expect("demand gen ad body captured");
        assert_eq!(dg.headlines, vec!["Block trackers".to_string()]);
        assert_eq!(dg.long_headlines, vec!["Block trackers and ads everywhere".to_string()]);
        assert_eq!(dg.descriptions, vec!["Private browsing made simple.".to_string()]);
        assert_eq!(dg.call_to_actions, vec!["Install".to_string()]);
        assert_eq!(dg.breadcrumb1.as_deref(), Some("ghostery"));
        assert_eq!(dg.breadcrumb2.as_deref(), Some("download"));
        assert_eq!(dg.videos, vec!["7001".to_string()]);

        let rendered = canonicalize(&render(&input));
        assert!(rendered.contains("demand_gen_video_responsive_ad {"), "{rendered}");
        assert!(rendered.contains("breadcrumb1 = \"ghostery\""), "{rendered}");
        assert!(rendered.contains("google_ads_youtube_video_asset."), "{rendered}");
        assert!(
            rendered.contains("videos") && !rendered.contains("<unresolved video"),
            "video ref should resolve to a youtube asset:\n{rendered}"
        );

        // The rendered .bid validates.
        let pf = crate::parser::parse_str(std::path::Path::new("dg.bid"), &rendered)
            .expect("rendered demand gen parses");
        let diags = crate::schema::validate_files(
            std::slice::from_ref(&pf),
            &crate::schema::InputBindings::default(),
        );
        assert!(
            diags.iter().all(|d| !d.is_error()),
            "validate errors: {:?}",
            diags.iter().filter(|d| d.is_error()).map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    // Issue #136: a UI-built in-stream ad, exactly as `pull` reads it. The point
    // is the tracking URL — until `video_ad` and `display_url` were modelled the
    // creative could not be declared at all, so the UTM slug the whole video
    // test is measured on existed only in the Google Ads UI.
    const VIDEO_AD_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "YouTube US", "amountMicros": "50000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "GH_YouTubeUS_v1", "status": "ENABLED", "advertisingChannelType": "VIDEO", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "US in-stream", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "asset": { "resourceName": "customers/9/assets/75804823141", "id": "75804823141", "youtubeVideoAsset": { "youtubeVideoId": "dQw4w9WgXcQ", "youtubeVideoTitle": "Ghostery 12s" } } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3001~4001", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "ad": {
                    "finalUrls": ["https://www.ghostery.com/?utm_campaign=GH_YouTubeUS_v1_0811-instream"],
                    "finalMobileUrls": ["https://m.ghostery.com/?utm_campaign=GH_YouTubeUS_v1_0811-instream"],
                    "displayUrl": "www.ghostery.com",
                    "videoAd": { "video": { "asset": "customers/9/assets/75804823141" } }
                } } }
            ]
        }
    ]"#;

    #[test]
    fn a_ui_built_video_ad_pulls_into_a_reviewable_file() {
        let input = from_search_response(VIDEO_AD_FIXTURE).expect("adapter");

        let ad = &input.ad_group_ads[0].ad;
        assert_eq!(ad.display_url.as_deref(), Some("www.ghostery.com"));
        assert_eq!(ad.video_ad.as_ref().expect("video_ad body").video, "75804823141");

        let rendered = canonicalize(&render(&input));
        assert!(rendered.contains("video_ad {"), "{rendered}");
        assert!(rendered.contains("display_url = \"www.ghostery.com\""), "{rendered}");
        assert!(
            rendered.contains("utm_campaign=GH_YouTubeUS_v1_0811-instream"),
            "the measurement key has to be in the file: {rendered}"
        );
        assert!(!rendered.contains("<unresolved video"), "{rendered}");

        let pf = crate::parser::parse_str(std::path::Path::new("video.bid"), &rendered)
            .expect("rendered video ad parses");
        let diags = crate::schema::validate_files(
            std::slice::from_ref(&pf),
            &crate::schema::InputBindings::default(),
        );
        assert!(
            diags.iter().all(|d| !d.is_error()),
            "validate errors: {:?}",
            diags.iter().filter(|d| d.is_error()).map(|d| &d.message).collect::<Vec<_>>()
        );

        let mut input2 = crate::api::import::import_files(
            std::slice::from_ref(&pf),
            &crate::schema::InputBindings::default(),
        )
        .expect("rendered video ad imports")
        .input;
        input2.customer_id = input.customer_id.clone();
        input2.login_customer_id = input.login_customer_id.clone();
        assert_eq!(
            canonicalize(&render_inner(&input, false)),
            canonicalize(&render_inner(&input2, false)),
        );
    }

    #[test]
    fn fold_collapses_url_variant_ads_into_one_template() {
        let input = from_search_response(FOLD_FIXTURE).expect("adapter");
        let out = render(&input);
        assert_eq!(out.matches("ad_template \"").count(), 2, "{out}");
        assert_eq!(out.matches("template = ad_template.").count(), 4, "{out}");
        assert!(out.contains("locals {"), "expected a lifted local:\n{out}");
        assert!(!out.contains("\n  ad {\n"), "no inline ad blocks should remain:\n{out}");
    }

    // Three ads: two share a creative (→ template), the third has the same
    // headlines but a different description (→ stays inline, yet still references
    // the headline list lifted into a local for the template).
    const MIXED_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Privacy Search", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Chrome", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3002", "id": "3002", "name": "Safari", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3003", "id": "3003", "name": "Edge", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3001~4001", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com/a"], "responsiveSearchAd": { "headlines": [{"text": "Block Ads Now"}, {"text": "Privacy First Browser"}, {"text": "Stop Trackers Fast"}], "descriptions": [{"text": "Fast private browsing."}, {"text": "Trusted by millions."}] } } } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3002~4002", "adGroup": "customers/9/adGroups/3002", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com/b"], "responsiveSearchAd": { "headlines": [{"text": "Block Ads Now"}, {"text": "Privacy First Browser"}, {"text": "Stop Trackers Fast"}], "descriptions": [{"text": "Fast private browsing."}, {"text": "Trusted by millions."}] } } } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3003~4003", "adGroup": "customers/9/adGroups/3003", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com/c"], "responsiveSearchAd": { "headlines": [{"text": "Block Ads Now"}, {"text": "Privacy First Browser"}, {"text": "Stop Trackers Fast"}], "descriptions": [{"text": "One-of-a-kind copy."}, {"text": "Only on this ad group."}] } } } }
            ]
        }
    ]"#;

    // The mandated desktop-only trio as three live criteria (campaign 2001),
    // plus a campaign whose mobile criterion carries a real bid adjustment
    // rather than an exclusion (campaign 2002).
    const DEVICE_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Desktop Only", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2002", "id": "2002", "name": "Mobile Adjusted", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~30000", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "device": { "type": "DESKTOP" } } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~30001", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "bidModifier": 0, "device": { "type": "MOBILE" } } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~30002", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "bidModifier": 0, "device": { "type": "TABLET" } } },
                { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2002~30003", "campaign": "customers/9/campaigns/2002", "status": "ENABLED", "bidModifier": 0.8, "device": { "type": "MOBILE" } } }
            ]
        }
    ]"#;

    // Suffix + custom parameters at all three levels, which is how a UTM
    // convention is actually spread across a tree.
    const TRACKING_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Search", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001", "finalUrlSuffix": "utm_source=google&utm_campaign=search_{_slug}", "urlCustomParameters": [{ "key": "region", "value": "us" }] } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Chrome", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "finalUrlSuffix": "utm_term=chrome" } },
                { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3001~4001", "adGroup": "customers/9/adGroups/3001", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com/chrome"], "urlCustomParameters": [{ "key": "slug", "value": "rsa_a" }, { "key": "creative", "value": "v2" }], "responsiveSearchAd": { "headlines": [{"text": "A"}, {"text": "B"}, {"text": "C"}], "descriptions": [{"text": "D1"}, {"text": "D2"}] } } } }
            ]
        }
    ]"#;

    #[test]
    fn tracking_fields_render_at_every_level() {
        let input = from_search_response(TRACKING_FIXTURE).expect("adapter");
        let out = render(&input);
        assert!(
            out.contains(r#"final_url_suffix = "utm_source=google&utm_campaign=search_{_slug}""#),
            "{out}"
        );
        assert!(out.contains(r#"custom_parameters = { region = "us" }"#), "{out}");
        assert!(out.contains(r#"final_url_suffix = "utm_term=chrome""#), "{out}");
        // Sorted by name, so a map with no inherent order still round-trips.
        assert!(
            out.contains(r#"custom_parameters = { creative = "v2", slug = "rsa_a" }"#),
            "{out}"
        );
    }

    // Three callouts and a snippet used by one campaign (2001, foldable), plus
    // a callout shared with a second campaign (2002, must stay a resource).
    const ASSET_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Facebook Ads", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2002", "id": "2002", "name": "Cookies", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "asset": { "resourceName": "customers/9/assets/5001", "id": "5001", "calloutAsset": { "calloutText": "Blocks feed ads" } } },
                { "asset": { "resourceName": "customers/9/assets/5002", "id": "5002", "calloutAsset": { "calloutText": "Open source" } } },
                { "asset": { "resourceName": "customers/9/assets/5003", "id": "5003", "calloutAsset": { "calloutText": "Free forever" } } },
                { "asset": { "resourceName": "customers/9/assets/5004", "id": "5004", "structuredSnippetAsset": { "header": "Types", "values": ["Ad blocker", "Tracker blocker"] } } },
                { "campaignAsset": { "resourceName": "customers/9/campaignAssets/2001~5001~CALLOUT", "campaign": "customers/9/campaigns/2001", "asset": "customers/9/assets/5001", "fieldType": "CALLOUT", "status": "ENABLED" } },
                { "campaignAsset": { "resourceName": "customers/9/campaignAssets/2001~5002~CALLOUT", "campaign": "customers/9/campaigns/2001", "asset": "customers/9/assets/5002", "fieldType": "CALLOUT", "status": "ENABLED" } },
                { "campaignAsset": { "resourceName": "customers/9/campaignAssets/2001~5004~STRUCTURED_SNIPPET", "campaign": "customers/9/campaigns/2001", "asset": "customers/9/assets/5004", "fieldType": "STRUCTURED_SNIPPET", "status": "ENABLED" } },
                { "campaignAsset": { "resourceName": "customers/9/campaignAssets/2001~5003~CALLOUT", "campaign": "customers/9/campaigns/2001", "asset": "customers/9/assets/5003", "fieldType": "CALLOUT", "status": "ENABLED" } },
                { "campaignAsset": { "resourceName": "customers/9/campaignAssets/2002~5003~CALLOUT", "campaign": "customers/9/campaigns/2002", "asset": "customers/9/assets/5003", "fieldType": "CALLOUT", "status": "ENABLED" } }
            ]
        }
    ]"#;

    // A callout owned by one ad group (foldable there), and one shared between
    // an ad group and its campaign (must stay a resource).
    const AD_GROUP_ASSET_FIXTURE: &str = r#"[
        {
            "results": [
                { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget", "amountMicros": "5000000" } },
                { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Search", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Chrome", "campaign": "customers/9/campaigns/2001", "status": "ENABLED" } },
                { "asset": { "resourceName": "customers/9/assets/5001", "id": "5001", "calloutAsset": { "calloutText": "Works in Chrome" } } },
                { "asset": { "resourceName": "customers/9/assets/5002", "id": "5002", "calloutAsset": { "calloutText": "Free forever" } } },
                { "asset": { "resourceName": "customers/9/assets/5003", "id": "5003", "structuredSnippetAsset": { "header": "Brands", "values": ["Chrome", "Firefox"] } } },
                { "adGroupAsset": { "resourceName": "customers/9/adGroupAssets/3001~5001~CALLOUT", "adGroup": "customers/9/adGroups/3001", "asset": "customers/9/assets/5001", "fieldType": "CALLOUT", "status": "ENABLED" } },
                { "adGroupAsset": { "resourceName": "customers/9/adGroupAssets/3001~5003~STRUCTURED_SNIPPET", "adGroup": "customers/9/adGroups/3001", "asset": "customers/9/assets/5003", "fieldType": "STRUCTURED_SNIPPET", "status": "ENABLED" } },
                { "adGroupAsset": { "resourceName": "customers/9/adGroupAssets/3001~5002~CALLOUT", "adGroup": "customers/9/adGroups/3001", "asset": "customers/9/assets/5002", "fieldType": "CALLOUT", "status": "ENABLED" } },
                { "campaignAsset": { "resourceName": "customers/9/campaignAssets/2001~5002~CALLOUT", "campaign": "customers/9/campaigns/2001", "asset": "customers/9/assets/5002", "fieldType": "CALLOUT", "status": "ENABLED" } }
            ]
        }
    ]"#;

    #[test]
    fn an_ad_groups_own_text_assets_fold_onto_it() {
        let input = from_search_response(AD_GROUP_ASSET_FIXTURE).expect("adapter");
        let out = render(&input);
        assert!(out.contains(r#"callouts = ["Works in Chrome"]"#), "{out}");
        assert!(out.contains(r#"values = ["Chrome", "Firefox"]"#), "{out}");
        // The one the campaign also uses keeps its resource and both links.
        assert!(out.contains(r#"text = "Free forever""#), "{out}");
        assert_eq!(out.matches("google_ads_ad_group_asset").count(), 1, "{out}");
        assert_eq!(out.matches("google_ads_campaign_asset").count(), 1, "{out}");
    }

    #[test]
    fn a_campaigns_own_text_assets_fold_onto_it() {
        let input = from_search_response(ASSET_FIXTURE).expect("adapter");
        let out = render(&input);
        assert!(
            out.contains(r#"callouts = ["Blocks feed ads", "Open source"]"#),
            "{out}"
        );
        assert!(out.contains("structured_snippet {"), "{out}");
        assert!(out.contains(r#"values = ["Ad blocker", "Tracker blocker"]"#), "{out}");
    }

    #[test]
    fn an_asset_two_campaigns_share_keeps_its_resource() {
        // The inline form has no way to say "this one is shared", so folding it
        // would silently split one asset into two.
        let input = from_search_response(ASSET_FIXTURE).expect("adapter");
        let out = render(&input);
        assert!(
            out.contains(r#"text = "Free forever""#),
            "the shared callout keeps its resource:\n{out}"
        );
        assert_eq!(
            out.matches("google_ads_campaign_asset").count(),
            2,
            "one attachment per campaign for the shared asset only:\n{out}"
        );
    }

    #[test]
    fn an_attachment_does_not_repeat_the_field_type_its_asset_implies() {
        let input = from_search_response(ASSET_FIXTURE).expect("adapter");
        let out = render(&input);
        assert!(
            !out.contains(r#"field_type = "CALLOUT""#),
            "inferable field_type is noise:\n{out}"
        );
    }

    #[test]
    fn fold_roundtrips_to_verbose() {
        assert_fold_roundtrips(FULL_FIXTURE);
        assert_fold_roundtrips(FOLD_FIXTURE);
        assert_fold_roundtrips(NEG_FIXTURE);
        assert_fold_roundtrips(MIXED_FIXTURE);
        assert_fold_roundtrips(DEVICE_FIXTURE);
        assert_fold_roundtrips(TRACKING_FIXTURE);
        assert_fold_roundtrips(ASSET_FIXTURE);
        assert_fold_roundtrips(AD_GROUP_ASSET_FIXTURE);
    }

    #[test]
    fn the_mandated_device_trio_folds_to_one_attribute() {
        // Three resources per campaign encoding an account-wide constant was
        // the single biggest repeated block in the measured tree (issue #145).
        let input = from_search_response(DEVICE_FIXTURE).expect("adapter");
        let out = render(&input);
        assert!(out.contains(r#"devices = ["DESKTOP"]"#), "{out}");
        assert_eq!(
            out.matches("device {").count(),
            1,
            "only the bid-adjusted criterion keeps its explicit block:\n{out}"
        );
    }

    #[test]
    fn a_device_bid_adjustment_is_not_an_exclusion() {
        // 0.8 on mobile means "bid less here", not "do not serve here", and no
        // spelling of the inline attribute can say it.
        let input = from_search_response(DEVICE_FIXTURE).expect("adapter");
        let out = render(&input);
        assert!(out.contains("bid_modifier = 0.8"), "{out}");
        assert!(
            !out.contains("excluded_devices"),
            "an adjusted campaign must not fold:\n{out}"
        );
    }

    #[test]
    fn fold_inline_ad_shares_lifted_local_with_template() {
        let input = from_search_response(MIXED_FIXTURE).expect("adapter");
        let out = render(&input);
        assert_eq!(out.matches("ad_template \"").count(), 1, "{out}");
        assert!(out.contains("\n  ad {\n"), "singleton stays inline:\n{out}");
        assert_eq!(out.matches("headlines = local.").count(), 2, "template + inline both ref the local:\n{out}");
    }

    #[test]
    fn fold_lifts_shared_campaign_negatives_into_a_local() {
        let input = from_search_response(NEG_FIXTURE).expect("adapter");
        let out = render(&input);
        assert_eq!(out.matches("_negatives = [").count(), 1, "one shared local:\n{out}");
        assert_eq!(out.matches("texts = local.").count(), 2, "both campaigns reference it:\n{out}");
        assert!(!out.contains("negative_keyword {"), "no expanded blocks:\n{out}");
    }

    #[test]
    fn folded_output_validates_as_hcl() {
        let input = from_search_response(FOLD_FIXTURE).expect("adapter");
        let folded = canonicalize(&render(&input));
        let pf = crate::parser::parse_str(std::path::Path::new("folded.bid"), &folded)
            .expect("parses");
        let diags = crate::schema::validate_files(
            std::slice::from_ref(&pf),
            &crate::schema::InputBindings::default(),
        );
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "validate errors: {:?}\n{folded}",
            errors.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn render_split_separates_account_and_campaign_buckets() {
        let input = from_search_response(FULL_FIXTURE).expect("adapter");
        let (account, campaigns) = render_split(&input);

        assert!(account.contains("google_ads_conversion_action"));
        assert!(account.contains("google_ads_call_asset"));
        assert!(account.contains("google_ads_customer_asset"));
        assert!(!account.contains("resource \"google_ads_campaign\""));
        assert!(!account.contains("google_ads_campaign_budget"));

        assert!(campaigns.contains("resource \"google_ads_campaign\""));
        assert!(campaigns.contains("google_ads_campaign_budget"));
        assert!(!campaigns.contains("google_ads_conversion_action"));
        assert!(!campaigns.contains("google_ads_call_asset"));
        assert!(!campaigns.contains("google_ads_customer_asset"));

        assert!(account.starts_with("provider \"google_ads\""));
        assert!(campaigns.starts_with("provider \"google_ads\""));
    }

    #[test]
    fn render_split_account_only_produces_no_campaigns_file() {
        let raw = r#"[
            {
                "results": [
                    { "conversionAction": { "resourceName": "customers/9/conversionActions/3001", "id": "3001", "name": "Lead", "type": "WEBPAGE", "category": "SUBMIT_LEAD_FORM" } }
                ]
            }
        ]"#;
        let input = from_search_response(raw).unwrap();
        let (account, campaigns) = render_split(&input);
        assert!(account.contains("google_ads_conversion_action"));
        assert!(campaigns.is_empty(), "campaigns should be empty, got {campaigns:?}");
    }

    #[test]
    fn render_split_emits_parseable_hcl2() {
        let input = from_search_response(FULL_FIXTURE).expect("adapter");
        let (account, campaigns) = render_split(&input);
        let _: hcl_edit::structure::Body =
            account.parse().expect("account.bid parses as HCL2");
        let _: hcl_edit::structure::Body =
            campaigns.parse().expect("campaigns.bid parses as HCL2");
    }

    #[test]
    fn render_split_validates_as_a_directory() {
        let raw = r#"[
            {
                "results": [
                    { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget A", "amountMicros": "5000000" } },
                    { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Camp A", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001", "containsEuPoliticalAdvertising": "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING" } },
                    { "sharedSet": { "resourceName": "customers/9/sharedSets/9001", "id": "9001", "name": "negs", "type": "NEGATIVE_KEYWORDS", "status": "ENABLED" } },
                    { "campaignSharedSet": { "resourceName": "customers/9/campaignSharedSets/2001~9001", "campaign": "customers/9/campaigns/2001", "sharedSet": "customers/9/sharedSets/9001", "status": "ENABLED" } }
                ]
            }
        ]"#;
        let input = from_search_response(raw).expect("adapter");
        let (account, campaigns) = render_split(&input);

        let dir = std::env::temp_dir()
            .join(format!("bidsmith-rs-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let account_path = dir.join("account.bid");
        let campaigns_path = dir.join("campaigns.bid");
        std::fs::write(&account_path, &account).unwrap();
        std::fs::write(&campaigns_path, &campaigns).unwrap();

        let parsed = vec![
            crate::parser::parse_file(&account_path).expect("account parses"),
            crate::parser::parse_file(&campaigns_path).expect("campaigns parses"),
        ];
        let diags = crate::schema::validate_files(&parsed, &crate::schema::InputBindings::default());
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "validate_files produced errors:\n{}\n--- account.bid ---\n{}\n--- campaigns.bid ---\n{}",
            errors
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            account,
            campaigns
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn render_split_validates_video_campaign_round_trip() {
        let raw = r#"[
            {
                "results": [
                    { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Video Budget", "amountMicros": "5000000" } },
                    { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Brand Awareness Video", "status": "ENABLED", "advertisingChannelType": "VIDEO", "advertisingChannelSubType": "VIDEO_NON_SKIPPABLE", "campaignBudget": "customers/9/campaignBudgets/1001", "videoCampaignSettings": { "videoAdInventoryControl": { "allowInStream": false, "allowInFeed": false, "allowShorts": false, "allowNonSkippableInStream": true } } } },
                    { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Non-skippable", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "type": "VIDEO_NON_SKIPPABLE_IN_STREAM" } },
                    { "adGroup": { "resourceName": "customers/9/adGroups/3002", "id": "3002", "name": "Responsive", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "type": "VIDEO_RESPONSIVE" } },
                    { "conversionAction": { "resourceName": "customers/9/conversionActions/4001", "id": "4001", "name": "Engaged View", "type": "UNKNOWN", "category": "UNKNOWN", "status": "ENABLED" } },
                    { "customAudience": { "resourceName": "customers/9/customAudiences/5001", "id": "5001", "name": "Ad blocker searchers", "type": "SEARCH", "status": "ENABLED", "members": [ { "memberType": "KEYWORD", "keyword": "ad blocker" } ] } },
                    { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2001~1", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "negative": false, "customAudience": { "customAudience": "customers/9/customAudiences/5001" } } }
                ]
            }
        ]"#;
        let input = from_search_response(raw).expect("adapter");
        let (account, campaigns) = render_split(&input);

        assert!(!campaigns.contains("<unresolved"), "dangling ref:\n{campaigns}");
        // The format the campaign runs, which is the thing an adopted video
        // campaign most needs its file to record (issue #133).
        assert!(
            campaigns.contains(r#"advertising_channel_sub_type = "VIDEO_NON_SKIPPABLE""#),
            "{campaigns}"
        );
        assert!(campaigns.contains("allow_non_skippable_in_stream = true"), "{campaigns}");
        assert!(campaigns.contains("allow_shorts = false"), "{campaigns}");

        let dir = std::env::temp_dir()
            .join(format!("bidsmith-rs-test-video-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let account_path = dir.join("account.bid");
        let campaigns_path = dir.join("campaigns.bid");
        std::fs::write(&account_path, &account).unwrap();
        std::fs::write(&campaigns_path, &campaigns).unwrap();

        let parsed = vec![
            crate::parser::parse_file(&account_path).expect("account parses"),
            crate::parser::parse_file(&campaigns_path).expect("campaigns parses"),
        ];
        let diags = crate::schema::validate_files(&parsed, &crate::schema::InputBindings::default());
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "validate_files produced errors:\n{}\n--- account.bid ---\n{}\n--- campaigns.bid ---\n{}",
            errors
                .iter()
                .map(|d| d.message.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
            account,
            campaigns
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_custom_period_budget_renders_its_lifetime_total() {
        // The live budget carries a stale `amountMicros` alongside the total it
        // actually spends; rendering both would not validate.
        let raw = r#"[
            {
                "results": [
                    { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Q3 Flight", "amountMicros": "0", "totalAmountMicros": "91000000", "period": "CUSTOM_PERIOD", "type": "STANDARD" } }
                ]
            }
        ]"#;
        let input = from_search_response(raw).expect("adapter");
        let out = render(&input);

        assert!(out.contains("total_amount_micros = 91000000"), "{out}");
        assert!(out.contains(r#"period = "CUSTOM_PERIOD""#), "{out}");
        assert!(!out.contains("amount_micros = 0"), "{out}");
        assert!(!out.contains(r#"type = "STANDARD""#), "{out}");

        let pf = crate::parser::parse_str(std::path::Path::new("rt.bid"), &out)
            .expect("rendered file parses");
        let diags =
            crate::schema::validate_files(&[pf], &crate::schema::InputBindings::default());
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(errors.is_empty(), "{:?}\n{out}", errors);
    }

    #[test]
    fn prune_orphans_drops_children_with_missing_parent() {
        let raw = r#"[
            {
                "results": [
                    { "campaignBudget": { "resourceName": "customers/9/campaignBudgets/1001", "id": "1001", "name": "Budget A", "amountMicros": "5000000" } },
                    { "campaign": { "resourceName": "customers/9/campaigns/2001", "id": "2001", "name": "Camp A", "status": "ENABLED", "advertisingChannelType": "SEARCH", "campaignBudget": "customers/9/campaignBudgets/1001" } },
                    { "adGroup": { "resourceName": "customers/9/adGroups/3001", "id": "3001", "name": "Keep", "campaign": "customers/9/campaigns/2001", "status": "ENABLED", "type": "SEARCH_STANDARD" } },
                    { "adGroup": { "resourceName": "customers/9/adGroups/3999", "id": "3999", "name": "Orphan", "campaign": "customers/9/campaigns/2999", "status": "ENABLED", "type": "VIDEO_RESPONSIVE" } },
                    { "adGroupAd": { "resourceName": "customers/9/adGroupAds/3999~5001", "adGroup": "customers/9/adGroups/3999", "status": "ENABLED", "ad": { "finalUrls": ["https://example.com"] } } },
                    { "adGroupCriterion": { "resourceName": "customers/9/adGroupCriteria/3999~111", "adGroup": "customers/9/adGroups/3999", "status": "ENABLED", "keyword": { "text": "running shoes", "matchType": "EXACT" } } },
                    { "campaignCriterion": { "resourceName": "customers/9/campaignCriteria/2999~222", "campaign": "customers/9/campaigns/2999", "status": "ENABLED", "negative": true, "keyword": { "text": "free", "matchType": "BROAD" } } }
                ]
            }
        ]"#;
        let mut input = from_search_response(raw).expect("adapter");
        assert_eq!(input.ad_groups.len(), 2);

        let dropped = prune_orphans(&mut input);

        assert_eq!(input.ad_groups.len(), 1);
        assert_eq!(input.ad_groups[0].id, "3001");
        assert!(input.ad_group_ads.is_empty());
        assert!(input.ad_group_criteria.is_empty());
        assert!(input.campaign_criteria.is_empty());
        assert_eq!(dropped.len(), 4, "unexpected dropped set: {dropped:?}");

        let (_, campaigns) = render_split(&input);
        assert!(
            !campaigns.contains("<unresolved"),
            "pruned render still has dangling refs:\n{campaigns}"
        );
    }

    #[test]
    fn shared_criteria_survive_export_and_round_trip() {
        // The --from-json shape: members live only in shared_criteria (the set's
        // negative_keywords is empty). They must not be dropped on export.
        let input: ExportInput = serde_json::from_str(
            r#"{
            "customer_id": "9",
            "shared_sets": [
                {"id":"google_ads_shared_set.negs","name":"Negatives","type":"NEGATIVE_KEYWORDS","status":"ENABLED"}
            ],
            "shared_criteria": [
                {"id":"a","shared_set":"google_ads_shared_set.negs","keyword":{"text":"free","match_type":"BROAD"}},
                {"id":"b","shared_set":"customers/9/sharedSets/77","keyword":{"text":"cheap","match_type":"PHRASE"}}
            ]
        }"#,
        )
        .expect("input");

        let out = render(&input);
        assert!(out.contains(r#""free""#), "in-snapshot member dropped:\n{out}");
        assert!(out.contains("negative_keyword {"), "member not inlined:\n{out}");
        assert!(out.contains(r#""cheap""#), "external-set member dropped:\n{out}");
        assert!(
            out.contains("resource \"google_ads_shared_criterion\""),
            "criterion for a set outside the snapshot should be standalone:\n{out}"
        );

        let dir = std::env::temp_dir().join(format!("bidsmith-sc-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("main.bid");
        std::fs::write(&path, &out).unwrap();
        let parsed = vec![crate::parser::parse_file(&path).expect("parses")];
        let diags = crate::schema::validate_files(&parsed, &crate::schema::InputBindings::default());
        let errors: Vec<_> = diags.iter().filter(|d| d.is_error()).collect();
        assert!(
            errors.is_empty(),
            "round-trip validate errors: {:?}\n{out}",
            errors.iter().map(|d| d.message.clone()).collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_number_does_not_saturate_large_integers() {
        assert_eq!(format_number(5.0), "5");
        assert_eq!(format_number(1_000_000.0), "1000000");
        assert_eq!(format_number(2.5), "2.5");
        let big = 1e20;
        assert_ne!(format_number(big), i64::MAX.to_string());
        assert_eq!(format_number(big).parse::<f64>().unwrap(), big);
    }

    #[test]
    fn short_address_label_is_verbatim() {
        let addr = "main.google_ads_campaign.summer_search";
        assert_eq!(address_label_payload(addr), addr);
        assert!(address_label_name(addr) == format!("{ADDRESS_LABEL_PREFIX}{addr}"));
        assert!(address_label_name(addr).len() <= MAX_LABEL_NAME_LEN);
    }

    #[test]
    fn long_address_label_fits_eighty_chars() {
        let addr =
            "instream.google_ads_campaign.instream_preroll_a_rather_long_campaign_family_identifier_2026";
        assert!(addr.len() + ADDRESS_LABEL_PREFIX.len() > MAX_LABEL_NAME_LEN);
        let name = address_label_name(addr);
        assert!(name.len() <= MAX_LABEL_NAME_LEN, "label {name:?} is {} chars", name.len());
        assert_ne!(address_label_payload(addr), addr, "long address must be encoded");
        assert!(
            name.starts_with(ADDRESS_LABEL_PREFIX),
            "still a bidsmith address label"
        );
    }

    #[test]
    fn long_address_label_is_deterministic_and_unique() {
        let a = "m.google_ads_campaign.instream_preroll_a_rather_long_campaign_family_identifier_aaa";
        let b = "m.google_ads_campaign.instream_preroll_a_rather_long_campaign_family_identifier_bbb";
        assert_eq!(address_label_payload(a), address_label_payload(a));
        assert_ne!(
            address_label_payload(a),
            address_label_payload(b),
            "distinct addresses must not collide even when their truncated head matches"
        );
    }

    #[test]
    fn multibyte_address_truncates_on_char_boundary() {
        let addr = format!("m.google_ads_campaign.{}", "é".repeat(60));
        let name = address_label_name(&addr);
        assert!(name.len() <= MAX_LABEL_NAME_LEN);
        assert!(name.is_char_boundary(name.len()));
    }

    fn address_label_name(address: &str) -> String {
        format!("{ADDRESS_LABEL_PREFIX}{}", address_label_payload(address))
    }
}
