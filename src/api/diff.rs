use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::commands::export::{
    address_label_payload, ExportInput, JsonAdGroup, JsonAdGroupAd, JsonAdGroupAsset,
    JsonAdGroupCriterion, JsonAssetAutomationSettings, JsonBudget, JsonCallAsset, JsonCampaign,
    JsonCampaignAsset, JsonCampaignCriterion, JsonCampaignSharedSet, JsonConversionAction,
    JsonAudience, JsonCriterion, JsonCustomAudience, JsonCustomParameter, JsonCustomerAsset,
    JsonSharedCriterion,
    JsonSharedSet, JsonSitelinkAsset, JsonStructuredSnippetAsset, JsonTargetingSetting,
    JsonYoutubeVideoAsset, AUTOMATICALLY_CREATED,
};
use crate::schema::CUSTOM_PERIOD;

/// Claim category token for `Campaign.frequency_caps` — see `diff_campaign`.
pub const FREQUENCY_CAPS_CATEGORY: &str = "frequency_caps";

#[derive(Debug, Clone)]
pub enum Action {
    NoOp {
        live_id: String,
    },
    Create,
    Update {
        live_id: String,
        changed_fields: Vec<FieldChange>,
    },
    /// A live resource that is no longer declared and should be removed. Only
    /// emitted for criteria members whose declared parent still exists, so the
    /// parent scopes the pruning (no `bidsmith:address` labels needed).
    Delete {
        live_id: String,
    },
    /// A live asset link Google's automation attached, inside a scope the file
    /// owns. It is paused rather than destroyed: `source` is output-only, so
    /// the automation keeps reattaching what it created, and a destroy would
    /// come back as the same destroy on the next plan and never converge.
    /// A paused link stays where it is and stops serving, which is the whole
    /// of what the file is asking for (issue #153).
    Pause {
        live_id: String,
    },
}

impl Action {
    pub fn live_id(&self) -> Option<&str> {
        match self {
            Action::NoOp { live_id }
            | Action::Update { live_id, .. }
            | Action::Delete { live_id }
            | Action::Pause { live_id } => Some(live_id.as_str()),
            Action::Create => None,
        }
    }
}

/// One field an update writes, carrying both sides of the comparison. The
/// field name alone answers "what does this touch"; a reviewer of a serving
/// campaign needs "what does it become" — so the value the account holds now
/// and the value the file asserts travel with it (issue #112).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldChange {
    /// The API field path — also the `updateMask` entry, so it is never
    /// prettified.
    pub field: String,
    pub live: String,
    pub desired: String,
}

impl FieldChange {
    /// `name: "Winter" -> "Winter Sale"`, the form plan rows print.
    pub fn render(&self) -> String {
        format!("{}: {} -> {}", self.field, self.live, self.desired)
    }

    /// For tests that exercise the update mask, where the values are beside
    /// the point.
    #[cfg(test)]
    pub fn named(field: &str) -> Self {
        FieldChange {
            field: field.to_string(),
            live: "(live)".to_string(),
            desired: "(desired)".to_string(),
        }
    }
}

/// The `updateMask` entries for a set of changes.
pub fn field_names(changes: &[FieldChange]) -> Vec<String> {
    changes.iter().map(|c| c.field.clone()).collect()
}

/// Longest value a plan row prints before eliding — a 300-keyword audience
/// member list would otherwise bury the row it belongs to.
const MAX_SHOWN_VALUE: usize = 60;

fn change(field: impl Into<String>, live: impl Shown, desired: impl Shown) -> FieldChange {
    FieldChange {
        field: field.into(),
        live: elide(live.shown()),
        desired: elide(desired.shown()),
    }
}

/// A change whose value must not be cut short. Eliding is right for a value a
/// reviewer only needs to recognise, and wrong for one that is the whole point
/// of the row: an asset-automation write replaces the list, so a reviewer is
/// being asked to approve exactly what the elision would hide.
fn whole_change(field: impl Into<String>, live: String, desired: String) -> FieldChange {
    FieldChange {
        field: field.into(),
        live,
        desired,
    }
}

fn elide(s: String) -> String {
    match s.char_indices().nth(MAX_SHOWN_VALUE) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s,
    }
}

/// How a field value reads in a plan row. Strings are quoted so a trailing
/// space or an empty name is visible rather than invisible; an absent value
/// reads as `(unset)` rather than as an empty string it is not the same as.
trait Shown {
    fn shown(&self) -> String;
}

/// A value that is already display text (a joined list, say) and must not be
/// quoted again.
struct Raw(String);

impl Shown for Raw {
    fn shown(&self) -> String {
        self.0.clone()
    }
}

impl Shown for str {
    fn shown(&self) -> String {
        format!("{self:?}")
    }
}

impl Shown for String {
    fn shown(&self) -> String {
        self.as_str().shown()
    }
}

impl Shown for i64 {
    fn shown(&self) -> String {
        self.to_string()
    }
}

impl Shown for f64 {
    fn shown(&self) -> String {
        self.to_string()
    }
}

impl Shown for bool {
    fn shown(&self) -> String {
        self.to_string()
    }
}

impl<T: Shown + ?Sized> Shown for &T {
    fn shown(&self) -> String {
        (*self).shown()
    }
}

impl<T: Shown> Shown for Option<T> {
    fn shown(&self) -> String {
        match self {
            Some(v) => v.shown(),
            None => "(unset)".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResourceDiff {
    pub address: String,
    pub kind: &'static str,
    pub action: Action,
}

/// A `bidsmith:address` label to write (or reconcile) on a resource as part of
/// applying the diff. Carried alongside the per-resource actions because the
/// label is bidsmith's identity key for the labelable kinds (campaign,
/// ad_group, ad_group_ad).
#[derive(Debug, Clone)]
pub struct LabelPlanEntry {
    /// Address of the resource the label attaches to (key into the mutate
    /// builder's resource_name map).
    pub address: String,
    /// `campaign` | `ad_group` | `ad_group_ad`.
    pub kind: &'static str,
    /// The `bidsmith:address=<this>` value the resource should carry.
    pub label_address: String,
    /// An existing live label resource_name to reuse; None -> create one.
    pub existing_label_rn: Option<String>,
    /// A stale bidsmith label association to remove (relabel on a surviving
    /// resource). None when the resource is new or already correctly labeled.
    pub stale_assoc_rn: Option<String>,
}

/// A `bidsmith:owns=<category>` label association to add to (or remove from) a
/// campaign / ad group. The claim records that bidsmith manages that criterion
/// category on the parent, so orphaned members still plan as destroys after the
/// last declared member of the category is removed — criteria themselves can't
/// carry identity labels (the API forbids labels on negative criteria).
#[derive(Debug, Clone)]
pub struct ClaimPlanEntry {
    /// Declared address of the parent the claim attaches to.
    pub address: String,
    /// `campaign` | `ad_group`.
    pub kind: &'static str,
    /// Criterion category token (`keyword_negative`, `location`, ...).
    pub category: &'static str,
    /// An existing live `bidsmith:owns` label resource_name to reuse; None ->
    /// create one. Only meaningful for claim additions.
    pub existing_label_rn: Option<String>,
    /// A live claim association to remove — set when the category is no longer
    /// declared on this parent (claim release). None for claim additions.
    pub stale_assoc_rn: Option<String>,
}

#[derive(Default)]
pub struct DiffReport {
    pub diffs: Vec<ResourceDiff>,
    pub label_plans: Vec<LabelPlanEntry>,
    pub claim_plans: Vec<ClaimPlanEntry>,
    pub noop_count: usize,
    pub create_count: usize,
    pub update_count: usize,
    pub delete_count: usize,
    /// Auto-created asset links being switched off rather than removed. Counted
    /// apart from updates because the row is about something nobody declared:
    /// the file is not changing a setting it owns, it is silencing what Google
    /// attached on its own.
    pub pause_count: usize,
    /// Resources that match live with no field change but still need label
    /// work — a `bidsmith:address` write (first-run adoption) or a
    /// `bidsmith:owns` claim add / release. Counted so plan / apply treat
    /// label-only work as a pending change rather than a no-op.
    pub adopt_count: usize,
    /// Live drift bidsmith cannot reconcile (e.g. an undeclared device
    /// modifier — the API forbids removing device criteria). Printed by
    /// plan / apply; never turned into mutate ops.
    pub warnings: Vec<String>,
    /// Removals dropped before the batch because the API refuses them and
    /// nothing is lost by leaving the resource alone. Shown next to the
    /// destroy count so a skipped row is visible, not merely absent.
    pub skipped_removal_count: usize,
    /// Operations the file asks for that this account can never accept —
    /// checked locally, before anything is sent, because the batch is atomic
    /// and one doomed operation rejects every unrelated one with it
    /// (issue #116). A non-empty list stops the plan.
    pub blockers: Vec<String>,
}

/// Label-first match with content fallback over parallel declared / live
/// arrays. A declared resource matches the live one carrying its
/// `bidsmith:address` label; failing that, it falls back to a content key
/// (name / parent+name) to adopt an unlabeled live resource. Returns the
/// matched live index per declared item (None = create); each live index is
/// claimed at most once. The caller derives the claimed set (the `Some`
/// values) for whole-resource removal detection.
fn match_label_first(
    declared_addr: &[&str],
    declared_key: &[Option<String>],
    live_addr: &[Option<&str>],
    live_key: &[String],
) -> Vec<Option<usize>> {
    let mut claimed = vec![false; live_addr.len()];
    let mut result: Vec<Option<usize>> = vec![None; declared_addr.len()];

    let mut by_addr: HashMap<&str, usize> = HashMap::new();
    for (i, a) in live_addr.iter().enumerate() {
        if let Some(a) = a {
            by_addr.entry(a).or_insert(i);
        }
    }
    for (di, addr) in declared_addr.iter().enumerate() {
        if let Some(&li) = by_addr.get(addr) {
            if !claimed[li] {
                claimed[li] = true;
                result[di] = Some(li);
            }
        }
    }

    let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, k) in live_key.iter().enumerate() {
        if !claimed[i] {
            by_key.entry(k.as_str()).or_default().push(i);
        }
    }
    for (di, key) in declared_key.iter().enumerate() {
        if result[di].is_some() {
            continue;
        }
        if let Some(key) = key {
            if let Some(cands) = by_key.get(key.as_str()) {
                if let Some(&li) = cands.iter().find(|&&li| !claimed[li]) {
                    claimed[li] = true;
                    result[di] = Some(li);
                }
            }
        }
    }
    result
}

fn label_segment(kind: &str) -> &'static str {
    match kind {
        "campaign" => "campaignLabels",
        "ad_group" => "adGroupLabels",
        "ad_group_ad" => "adGroupAdLabels",
        "ad_group_criterion" => "adGroupCriterionLabels",
        _ => "labels",
    }
}

/// Decide what label work a labelable resource needs. `matched` is the live
/// resource it resolved to (id + its current `bidsmith:address` payload), or
/// None for a create. Returns None when the resource already carries exactly the
/// right label (a no-op). All comparisons run in label-payload space so long
/// addresses (whose labels are hash-encoded to fit the 80-char cap) still match.
fn make_label_plan(
    kind: &'static str,
    address: &str,
    matched: Option<(&str, Option<&str>)>,
    customer_id: &str,
    labels: &HashMap<String, String>,
) -> Option<LabelPlanEntry> {
    let payload = address_label_payload(address);
    if let Some((_, Some(current))) = matched {
        if current == payload {
            return None;
        }
    }
    let stale_assoc_rn = match matched {
        Some((live_id, Some(old))) if old != payload => labels.get(old).map(|label_rn| {
            let label_id = label_rn.rsplit('/').next().unwrap_or(label_rn);
            format!(
                "customers/{customer_id}/{}/{live_id}~{label_id}",
                label_segment(kind)
            )
        }),
        _ => None,
    };
    Some(LabelPlanEntry {
        address: address.to_string(),
        kind,
        existing_label_rn: labels.get(&payload).cloned(),
        label_address: payload,
        stale_assoc_rn,
    })
}

pub fn diff(declared: &ExportInput, live: &ExportInput) -> DiffReport {
    let mut diffs: Vec<ResourceDiff> = Vec::new();
    let mut label_plans: Vec<LabelPlanEntry> = Vec::new();
    let mut campaign_warnings: Vec<String> = Vec::new();
    let mut blockers: Vec<String> = Vec::new();
    let mut skipped_removal_count = 0usize;
    // Modules a partial run left unread, collected so the plan says which
    // files would have to be in the input for their resources to reconcile.
    let mut out_of_scope_modules: BTreeSet<&str> = BTreeSet::new();
    let mut out_of_scope_count = 0usize;
    // Live ids on the read-only VIDEO channel, so a removal the API would
    // refuse is dropped before it can poison the batch (issue #116).
    let live_video_campaigns: HashSet<&str> = live
        .campaigns
        .iter()
        .filter(|c| c.advertising_channel_type == "VIDEO")
        .map(|c| c.id.as_str())
        .collect();
    let live_video_ad_groups: HashSet<&str> = live
        .ad_groups
        .iter()
        .filter(|g| live_video_campaigns.contains(g.campaign.as_str()))
        .map(|g| g.id.as_str())
        .collect();
    let mut campaign_match: HashMap<String, String> = HashMap::new();
    let mut ad_group_match: HashMap<String, String> = HashMap::new();
    // declared asset address -> live asset id, across every asset type
    // (call / sitelink / callout / structured snippet / youtube video).
    // Consumed by the customer_asset / campaign_asset / ad_group_asset link
    // diffs and by the video ad body key.
    let mut asset_match: HashMap<String, String> = HashMap::new();
    let customer_id = declared.customer_id.as_str();

    // Video assets resolve ahead of the ad_group_ad pass: a video ad's body key
    // names its video by live asset id, so the mapping has to exist before ads
    // are matched. The diff entries themselves are pushed with the other assets.
    let live_youtube_video_assets: HashMap<&str, &JsonYoutubeVideoAsset> = live
        .youtube_video_assets
        .iter()
        .map(|a| (a.youtube_video_id.as_str(), a))
        .collect();
    for d in &declared.youtube_video_assets {
        if let Some(l) = live_youtube_video_assets.get(d.youtube_video_id.as_str()) {
            asset_match.insert(d.id.clone(), l.id.clone());
        }
    }

    // ---- campaign_budgets (match by name) --------------------------------
    let live_budgets: HashMap<&str, &JsonBudget> = live
        .campaign_budgets
        .iter()
        .map(|b| (b.name.as_str(), b))
        .collect();
    for d in &declared.campaign_budgets {
        let action = match live_budgets.get(d.name.as_str()) {
            Some(l) => {
                campaign_warnings.extend(budget_immutable_warnings(&d.id, d, l));
                action_for_match(l.id.clone(), diff_budget(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign_budget",
            action,
        });
    }

    // ---- campaigns (label-first, content-fallback by name) ----------------
    let live_campaigns: Vec<&JsonCampaign> = live.campaigns.iter().collect();
    {
        let decl_payloads: Vec<String> =
            declared.campaigns.iter().map(|c| address_label_payload(&c.id)).collect();
        let decl_addr: Vec<&str> = decl_payloads.iter().map(String::as_str).collect();
        let decl_key: Vec<Option<String>> =
            declared.campaigns.iter().map(|c| Some(c.name.clone())).collect();
        let live_a: Vec<Option<&str>> =
            live_campaigns.iter().map(|c| c.managed_address.as_deref()).collect();
        let live_k: Vec<String> = live_campaigns.iter().map(|c| c.name.clone()).collect();
        let matches = match_label_first(&decl_addr, &decl_key, &live_a, &live_k);
        let mut claimed = vec![false; live_campaigns.len()];
        for (di, m) in matches.iter().enumerate() {
            let d = &declared.campaigns[di];
            let (action, matched) = match m {
                Some(li) => {
                    let l = live_campaigns[*li];
                    claimed[*li] = true;
                    campaign_match.insert(d.id.clone(), l.id.clone());
                    campaign_warnings.extend(campaign_immutable_warnings(&d.id, d, l));
                    let caps_claimed = live
                        .campaign_claims
                        .get(&l.id)
                        .is_some_and(|cats| cats.iter().any(|c| c == FREQUENCY_CAPS_CATEGORY));
                    (
                        action_for_match(l.id.clone(), diff_campaign(d, l, caps_claimed)),
                        Some((l.id.as_str(), l.managed_address.as_deref())),
                    )
                }
                None => {
                    if let Some(w) = missing_bidding_strategy_warning(d) {
                        campaign_warnings.push(w);
                    }
                    (Action::Create, None)
                }
            };
            // A create the file already forbade gets the sharper "expected to
            // adopt, found nothing" blocker below; drift on one still needs
            // the channel warning.
            let adopt_only_create =
                matches!(action, Action::Create) && declared.adopt_only.contains(&d.id);
            if d.advertising_channel_type == "VIDEO" && !adopt_only_create {
                if let Some(b) = video_is_read_only_blocker(&d.id, &action) {
                    blockers.push(b);
                }
            }
            if let Some(plan) =
                make_label_plan("campaign", &d.id, matched, customer_id, &live.labels)
            {
                label_plans.push(plan);
            }
            diffs.push(ResourceDiff {
                address: d.id.clone(),
                kind: "campaign",
                action,
            });
        }
        for (li, l) in live_campaigns.iter().enumerate() {
            if !claimed[li] && !is_removed(l.status.as_deref()) {
                if let Some(addr) = &l.managed_address {
                    if let Some(module) = unread_module(declared, addr) {
                        out_of_scope_modules.insert(module);
                        out_of_scope_count += 1;
                        skipped_removal_count += 1;
                        continue;
                    }
                    if live_video_campaigns.contains(l.id.as_str()) {
                        campaign_warnings.push(video_removal_skipped_warning("a VIDEO campaign", addr));
                        skipped_removal_count += 1;
                        continue;
                    }
                    diffs.push(removal_diff("campaign", addr, &l.id));
                }
            }
        }
    }

    // ---- ad_groups (label-first, content-fallback by campaign + name) -----
    let live_ad_groups: Vec<&JsonAdGroup> = live.ad_groups.iter().collect();
    let video_campaigns: HashSet<&str> = declared
        .campaigns
        .iter()
        .filter(|c| c.advertising_channel_type == "VIDEO")
        .map(|c| c.id.as_str())
        .collect();
    {
        let decl_payloads: Vec<String> =
            declared.ad_groups.iter().map(|g| address_label_payload(&g.id)).collect();
        let decl_addr: Vec<&str> = decl_payloads.iter().map(String::as_str).collect();
        let decl_key: Vec<Option<String>> = declared
            .ad_groups
            .iter()
            .map(|g| campaign_match.get(&g.campaign).map(|cid| format!("{cid}\u{1f}{}", g.name)))
            .collect();
        let live_a: Vec<Option<&str>> =
            live_ad_groups.iter().map(|g| g.managed_address.as_deref()).collect();
        let live_k: Vec<String> = live_ad_groups
            .iter()
            .map(|g| format!("{}\u{1f}{}", g.campaign, g.name))
            .collect();
        let matches = match_label_first(&decl_addr, &decl_key, &live_a, &live_k);
        let mut claimed = vec![false; live_ad_groups.len()];
        for (di, m) in matches.iter().enumerate() {
            let d = &declared.ad_groups[di];
            let (action, matched) = match m {
                Some(li) => {
                    let l = live_ad_groups[*li];
                    claimed[*li] = true;
                    ad_group_match.insert(d.id.clone(), l.id.clone());
                    (
                        action_for_match(l.id.clone(), diff_ad_group(d, l)),
                        Some((l.id.as_str(), l.managed_address.as_deref())),
                    )
                }
                None => (Action::Create, None),
            };
            if video_campaigns.contains(d.campaign.as_str()) {
                if let Some(b) = video_is_read_only_blocker(&d.id, &action) {
                    blockers.push(b);
                }
            }
            if let Some(plan) =
                make_label_plan("ad_group", &d.id, matched, customer_id, &live.labels)
            {
                label_plans.push(plan);
            }
            diffs.push(ResourceDiff {
                address: d.id.clone(),
                kind: "ad_group",
                action,
            });
        }
        for (li, l) in live_ad_groups.iter().enumerate() {
            if !claimed[li] && !is_removed(l.status.as_deref()) {
                if let Some(addr) = &l.managed_address {
                    if let Some(module) = unread_module(declared, addr) {
                        out_of_scope_modules.insert(module);
                        out_of_scope_count += 1;
                        skipped_removal_count += 1;
                        continue;
                    }
                    if live_video_ad_groups.contains(l.id.as_str()) {
                        campaign_warnings.push(video_removal_skipped_warning("an ad group on a VIDEO campaign", addr));
                        skipped_removal_count += 1;
                        continue;
                    }
                    diffs.push(removal_diff("ad_group", addr, &l.id));
                }
            }
        }
    }

    // ---- ad_group_ads (label hit, else body 1:1; label authorizes destroy) -
    // The read-only channel reaches the creative too, and an ad carries no
    // channel of its own — so the check is on the parent, the same way the ad
    // group's is on its campaign. Adoption is what works here: the label write
    // that claims a live video ad is permitted, creating one never is.
    let video_ad_groups: HashSet<&str> = declared
        .ad_groups
        .iter()
        .filter(|g| video_campaigns.contains(g.campaign.as_str()))
        .map(|g| g.id.as_str())
        .collect();
    let ad_outcomes = match_ad_group_ads(
        &declared.ad_group_ads,
        &live.ad_group_ads,
        &ad_group_match,
        &asset_match,
    );
    let mut ad_claimed = vec![false; live.ad_group_ads.len()];
    for (di, (action, li)) in ad_outcomes.iter().enumerate() {
        let d = &declared.ad_group_ads[di];
        let matched = match li {
            Some(i) => {
                ad_claimed[*i] = true;
                let l = &live.ad_group_ads[*i];
                Some((l.id.as_str(), l.managed_address.as_deref()))
            }
            None => None,
        };
        // As on the campaign: a create the file already forbade gets the
        // sharper "expected to adopt, found nothing" blocker instead.
        let adopt_only_create =
            matches!(action, Action::Create) && declared.adopt_only.contains(&d.id);
        if video_ad_groups.contains(d.ad_group.as_str()) && !adopt_only_create {
            if let Some(b) = video_is_read_only_blocker(&d.id, action) {
                blockers.push(b);
            }
        }
        if let Some(plan) =
            make_label_plan("ad_group_ad", &d.id, matched, customer_id, &live.labels)
        {
            label_plans.push(plan);
        }
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "ad_group_ad",
            action: action.clone(),
        });
    }
    let live_ad_group_ids: std::collections::HashSet<&str> =
        live.ad_groups.iter().map(|g| g.id.as_str()).collect();
    for (i, l) in live.ad_group_ads.iter().enumerate() {
        if ad_claimed[i] {
            continue;
        }
        // Removing an ad group leaves its ads addressable but un-mutatable: the
        // ad keeps its `bidsmith:address` label yet the API rejects any op on it
        // ("Removed ads may not be modified"), sinking the whole atomic batch. An
        // ad already `REMOVED`, or orphaned under an ad group that is gone from
        // live state (i.e. removed), must not re-plan as a destroy.
        if is_removed(l.status.as_deref()) || !live_ad_group_ids.contains(l.ad_group.as_str()) {
            continue;
        }
        if let Some(addr) = &l.managed_address {
            if let Some(module) = unread_module(declared, addr) {
                out_of_scope_modules.insert(module);
                out_of_scope_count += 1;
                skipped_removal_count += 1;
                continue;
            }
            if live_video_ad_groups.contains(l.ad_group.as_str()) {
                campaign_warnings.push(video_removal_skipped_warning("an ad on a VIDEO campaign", addr));
                skipped_removal_count += 1;
                continue;
            }
            diffs.push(removal_diff("ad_group_ad", addr, &l.id));
        }
    }

    // Ahead of the criteria that reference them: an `audience` criterion is
    // keyed on the live custom audience id, which this match supplies.
    let mut custom_audience_match: HashMap<String, String> = HashMap::new();
    let live_custom_audiences: HashMap<&str, &JsonCustomAudience> = live
        .custom_audiences
        .iter()
        .map(|a| (a.name.as_str(), a))
        .collect();
    for d in &declared.custom_audiences {
        let action = match live_custom_audiences.get(d.name.as_str()) {
            Some(l) => {
                custom_audience_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), diff_custom_audience(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "custom_audience",
            action,
        });
    }

    // ---- ad_group_criteria (match by ad_group + criterion key) -----------
    let mut live_ag_criteria: HashMap<(String, String), &JsonAdGroupCriterion> = HashMap::new();
    for c in &live.ad_group_criteria {
        if let Some(key) = criterion_key(&c.target, c.negative.unwrap_or(false), &HashMap::new()) {
            live_ag_criteria.insert((c.ad_group.clone(), key), c);
        }
    }
    for d in &declared.ad_group_criteria {
        let action = match (
            ad_group_match.get(&d.ad_group),
            criterion_key(
                &d.target,
                d.negative.unwrap_or(false),
                &custom_audience_match,
            ),
        ) {
            (Some(parent_id), Some(key)) => match live_ag_criteria.get(&(parent_id.clone(), key)) {
                Some(l) => action_for_match(l.id.clone(), diff_ad_group_criterion(d, l)),
                None => Action::Create,
            },
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "ad_group_criterion",
            action,
        });
    }

    // ---- campaign_criteria (match by campaign + criterion key) -----------
    let mut live_c_criteria: HashMap<(String, String), &JsonCampaignCriterion> = HashMap::new();
    for c in &live.campaign_criteria {
        if let Some(key) = criterion_key(&c.target, c.negative.unwrap_or(false), &HashMap::new()) {
            live_c_criteria.insert((c.campaign.clone(), key), c);
        }
    }
    for d in &declared.campaign_criteria {
        let action = match (
            campaign_match.get(&d.campaign),
            criterion_key(
                &d.target,
                d.negative.unwrap_or(false),
                &custom_audience_match,
            ),
        ) {
            (Some(parent_id), Some(key)) => match live_c_criteria.get(&(parent_id.clone(), key)) {
                Some(l) => action_for_match(l.id.clone(), diff_campaign_criterion(d, l)),
                None => Action::Create,
            },
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign_criterion",
            action,
        });
    }

    let live_conversion_actions: HashMap<&str, &JsonConversionAction> = live
        .conversion_actions
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    for d in &declared.conversion_actions {
        let action = match live_conversion_actions.get(d.name.as_str()) {
            Some(l) => action_for_match(l.id.clone(), diff_conversion_action(d, l)),
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "conversion_action",
            action,
        });
    }

    let live_call_assets: HashMap<(String, String), &JsonCallAsset> = live
        .call_assets
        .iter()
        .map(|a| ((a.country_code.clone(), a.phone_number.clone()), a))
        .collect();
    for d in &declared.call_assets {
        let action = match live_call_assets
            .get(&(d.country_code.clone(), d.phone_number.clone()))
        {
            Some(l) => {
                asset_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), diff_call_asset(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "call_asset",
            action,
        });
    }

    let live_sitelink_assets = content_index(&live.sitelink_assets, sitelink_asset_key);
    for d in &declared.sitelink_assets {
        let action = match live_sitelink_assets.get(&sitelink_asset_key(d)) {
            Some(cands) => {
                let l = cands[0];
                asset_match.insert(d.id.clone(), l.id.clone());
                if let Some(w) = ambiguity_warning(&d.id, cands, |a| &a.id) {
                    campaign_warnings.push(w);
                }
                action_for_match(l.id.clone(), Vec::new())
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "sitelink_asset",
            action,
        });
    }

    let live_callout_assets = content_index(&live.callout_assets, |a| a.text.clone());
    for d in &declared.callout_assets {
        let action = match live_callout_assets.get(&d.text) {
            Some(cands) => {
                let l = cands[0];
                asset_match.insert(d.id.clone(), l.id.clone());
                if let Some(w) = ambiguity_warning(&d.id, cands, |a| &a.id) {
                    campaign_warnings.push(w);
                }
                action_for_match(l.id.clone(), Vec::new())
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "callout_asset",
            action,
        });
    }

    let live_structured_snippet_assets = content_index(
        &live.structured_snippet_assets,
        structured_snippet_asset_key,
    );
    for d in &declared.structured_snippet_assets {
        let action = match live_structured_snippet_assets.get(&structured_snippet_asset_key(d)) {
            Some(cands) => {
                let l = cands[0];
                asset_match.insert(d.id.clone(), l.id.clone());
                if let Some(w) = ambiguity_warning(&d.id, cands, |a| &a.id) {
                    campaign_warnings.push(w);
                }
                action_for_match(l.id.clone(), Vec::new())
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "structured_snippet_asset",
            action,
        });
    }

    for d in &declared.youtube_video_assets {
        let action = match live_youtube_video_assets.get(d.youtube_video_id.as_str()) {
            Some(l) => action_for_match(l.id.clone(), Vec::new()),
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "youtube_video_asset",
            action,
        });
    }

    let mut live_customer_assets: HashMap<(String, String), &JsonCustomerAsset> = HashMap::new();
    for a in &live.customer_assets {
        live_customer_assets.insert((a.asset.clone(), a.field_type.clone()), a);
    }
    for d in &declared.customer_assets {
        let action = match asset_match.get(&d.asset) {
            Some(asset_id) => match live_customer_assets
                .get(&(asset_id.clone(), d.field_type.clone()))
            {
                Some(l) => action_for_match(l.id.clone(), diff_customer_asset(d, l)),
                None => Action::Create,
            },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "customer_asset",
            action,
        });
    }

    let live_campaign_assets: HashMap<(String, String, String), &JsonCampaignAsset> = live
        .campaign_assets
        .iter()
        .map(|a| ((a.campaign.clone(), a.asset.clone(), a.field_type.clone()), a))
        .collect();
    for d in &declared.campaign_assets {
        let action = match (resolve_campaign_id(&campaign_match, &d.campaign), asset_match.get(&d.asset)) {
            (Some(campaign_id), Some(asset_id)) => match live_campaign_assets.get(&(
                campaign_id,
                asset_id.clone(),
                d.field_type.clone(),
            )) {
                Some(l) => action_for_match(l.id.clone(), diff_campaign_asset(d, l)),
                None => Action::Create,
            },
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign_asset",
            action,
        });
    }

    let live_ad_group_assets: HashMap<(String, String, String), &JsonAdGroupAsset> = live
        .ad_group_assets
        .iter()
        .map(|a| ((a.ad_group.clone(), a.asset.clone(), a.field_type.clone()), a))
        .collect();
    for d in &declared.ad_group_assets {
        let action = match (resolve_ad_group_id(&ad_group_match, &d.ad_group), asset_match.get(&d.asset)) {
            (Some(ad_group_id), Some(asset_id)) => match live_ad_group_assets.get(&(
                ad_group_id,
                asset_id.clone(),
                d.field_type.clone(),
            )) {
                Some(l) => action_for_match(l.id.clone(), diff_ad_group_asset(d, l)),
                None => Action::Create,
            },
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "ad_group_asset",
            action,
        });
    }


    let mut shared_set_match: HashMap<String, String> = HashMap::new();
    let live_shared_sets: HashMap<&str, &JsonSharedSet> = live
        .shared_sets
        .iter()
        .map(|s| (s.name.as_str(), s))
        .collect();
    for d in &declared.shared_sets {
        let action = match live_shared_sets.get(d.name.as_str()) {
            Some(l) => {
                shared_set_match.insert(d.id.clone(), l.id.clone());
                action_for_match(l.id.clone(), diff_shared_set(d, l))
            }
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "shared_set",
            action,
        });
    }

    let live_shared_criteria: HashMap<(String, String, String), &JsonSharedCriterion> = live
        .shared_criteria
        .iter()
        .map(|c| {
            (
                (
                    c.shared_set.clone(),
                    c.keyword.match_type.clone(),
                    c.keyword.text.clone(),
                ),
                c,
            )
        })
        .collect();
    for d in &declared.shared_criteria {
        let live_set_id = shared_set_match.get(&d.shared_set).cloned().or_else(|| {
            if d.shared_set.starts_with("customers/") {
                d.shared_set.rsplit('/').next().map(str::to_string)
            } else {
                None
            }
        });
        let action = match live_set_id {
            Some(set_id) => match live_shared_criteria.get(&(
                set_id,
                d.keyword.match_type.clone(),
                d.keyword.text.clone(),
            )) {
                Some(l) => action_for_match(l.id.clone(), Vec::new()),
                None => Action::Create,
            },
            None => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "shared_criterion",
            action,
        });
    }

    let live_campaign_shared_sets: HashMap<(String, String), &JsonCampaignSharedSet> = live
        .campaign_shared_sets
        .iter()
        .map(|s| ((s.campaign.clone(), s.shared_set.clone()), s))
        .collect();
    for d in &declared.campaign_shared_sets {
        let action = match (
            campaign_match.get(&d.campaign),
            shared_set_match.get(&d.shared_set),
        ) {
            (Some(c_id), Some(s_id)) => {
                match live_campaign_shared_sets.get(&(c_id.clone(), s_id.clone())) {
                    Some(l) => action_for_match(l.id.clone(), diff_campaign_shared_set(d, l)),
                    None => Action::Create,
                }
            }
            _ => Action::Create,
        };
        diffs.push(ResourceDiff {
            address: d.id.clone(),
            kind: "campaign_shared_set",
            action,
        });
    }

    let claim_plans = claim_plan_entries(declared, live, &campaign_match, &ad_group_match);

    let (link_deletes, link_warnings, link_skipped) = orphan_asset_link_deletes(
        declared,
        live,
        &diffs,
        &campaign_match,
        &ad_group_match,
        &live_video_campaigns,
        &live_video_ad_groups,
    );
    diffs.extend(link_deletes);
    skipped_removal_count += link_skipped;

    let (deletes, mut warnings, criteria_skipped) = orphan_criteria_deletes(
        declared,
        live,
        &diffs,
        &ad_group_match,
        &campaign_match,
        &shared_set_match,
    );
    diffs.extend(deletes);
    skipped_removal_count += criteria_skipped;
    warnings.extend(link_warnings);
    warnings.extend(campaign_warnings);
    warnings.extend(shared_budget_warnings(declared));
    if !out_of_scope_modules.is_empty() {
        warnings.push(out_of_scope_warning(
            out_of_scope_count,
            &out_of_scope_modules,
        ));
    }
    blockers.extend(adopt_only_blockers(declared, &diffs));

    let mut noop_count = 0;
    let mut create_count = 0;
    let mut update_count = 0;
    let mut delete_count = 0;
    let mut pause_count = 0;
    for d in &diffs {
        match &d.action {
            Action::NoOp { .. } => noop_count += 1,
            Action::Create => create_count += 1,
            Action::Update { .. } => update_count += 1,
            Action::Delete { .. } => delete_count += 1,
            Action::Pause { .. } => pause_count += 1,
        }
    }

    let noop_addrs: std::collections::HashSet<&str> = diffs
        .iter()
        .filter(|d| matches!(d.action, Action::NoOp { .. }))
        .map(|d| d.address.as_str())
        .collect();
    let adopt_addrs: std::collections::HashSet<&str> = label_plans
        .iter()
        .map(|p| p.address.as_str())
        .chain(claim_plans.iter().map(|p| p.address.as_str()))
        .filter(|a| noop_addrs.contains(a))
        .collect();
    let adopt_count = adopt_addrs.len();

    DiffReport {
        diffs,
        label_plans,
        claim_plans,
        noop_count,
        create_count,
        update_count,
        delete_count,
        pause_count,
        adopt_count,
        skipped_removal_count,
        warnings,
        blockers,
    }
}

/// The categories a `bidsmith:owns` claim can cover — criterion kinds, the
/// asset field types a campaign / ad group links under, and `frequency_caps`,
/// the one plain field whose "declared as empty" is indistinguishable from "not
/// managed here". Device is excluded (the API forbids removing device criteria,
/// so a claim would never drive a destroy).
fn canonical_category(cat: &str) -> Option<&'static str> {
    Some(match cat {
        FREQUENCY_CAPS_CATEGORY => FREQUENCY_CAPS_CATEGORY,
        "keyword_positive" => "keyword_positive",
        "keyword_negative" => "keyword_negative",
        "location" => "location",
        "language" => "language",
        "proximity" => "proximity",
        "youtube_channel" => "youtube_channel",
        "youtube_video" => "youtube_video",
        "topic" => "topic",
        "placement" => "placement",
        "user_interest" => "user_interest",
        "age_range" => "age_range",
        "gender" => "gender",
        "parental_status" => "parental_status",
        "income_range" => "income_range",
        "audience" => "audience",
        "asset_sitelink" => "asset_sitelink",
        "asset_callout" => "asset_callout",
        "asset_structured_snippet" => "asset_structured_snippet",
        "asset_call" => "asset_call",
        _ => return None,
    })
}

fn polarity_category(negative: bool) -> &'static str {
    if negative {
        "keyword_negative"
    } else {
        "keyword_positive"
    }
}

/// Reconcile desired category claims (derived from declared criteria, plus a
/// campaign's declared `frequency_caps`) against the live `bidsmith:owns`
/// associations: a desired claim missing live plans an association add; a live
/// claim on a still-declared parent whose category has no declared members
/// plans an association remove. Parents that are no longer declared need
/// nothing — their associations die with the resource.
fn claim_plan_entries(
    declared: &ExportInput,
    live: &ExportInput,
    campaign_match: &HashMap<String, String>,
    ad_group_match: &HashMap<String, String>,
) -> Vec<ClaimPlanEntry> {
    let customer_id = declared.customer_id.as_str();
    let mut out: Vec<ClaimPlanEntry> = Vec::new();

    let mut desired_ag: std::collections::BTreeSet<(&str, &'static str)> =
        std::collections::BTreeSet::new();
    let declared_ags: std::collections::HashSet<&str> =
        declared.ad_groups.iter().map(|g| g.id.as_str()).collect();
    for d in &declared.ad_group_criteria {
        if declared_ags.contains(d.ad_group.as_str()) {
            if let Some(cat) =
                canonical_category(criterion_category(&d.target, d.negative.unwrap_or(false)))
            {
                desired_ag.insert((&d.ad_group, cat));
            }
        }
    }
    for d in &declared.ad_group_assets {
        if declared_ags.contains(d.ad_group.as_str()) {
            if let Some(cat) = asset_link_category(&d.field_type) {
                desired_ag.insert((&d.ad_group, cat));
            }
        }
    }

    let mut desired_c: std::collections::BTreeSet<(&str, &'static str)> =
        std::collections::BTreeSet::new();
    let declared_cs: std::collections::HashSet<&str> =
        declared.campaigns.iter().map(|c| c.id.as_str()).collect();
    for d in &declared.campaign_criteria {
        if declared_cs.contains(d.campaign.as_str()) {
            if let Some(cat) =
                canonical_category(criterion_category(&d.target, d.negative.unwrap_or(false)))
            {
                desired_c.insert((&d.campaign, cat));
            }
        }
    }
    for d in &declared.campaign_assets {
        if declared_cs.contains(d.campaign.as_str()) {
            if let Some(cat) = asset_link_category(&d.field_type) {
                desired_c.insert((&d.campaign, cat));
            }
        }
    }
    for c in &declared.campaigns {
        if !c.frequency_caps.is_empty() {
            desired_c.insert((c.id.as_str(), FREQUENCY_CAPS_CATEGORY));
        }
    }

    let mut reconcile = |desired: &std::collections::BTreeSet<(&str, &'static str)>,
                         matches: &HashMap<String, String>,
                         claims: &HashMap<String, Vec<String>>,
                         kind: &'static str| {
        let assoc_segment = label_segment(kind);
        for (addr, cat) in desired {
            let live_has = matches
                .get(*addr)
                .and_then(|id| claims.get(id))
                .is_some_and(|cats| cats.iter().any(|c| c == cat));
            if !live_has {
                out.push(ClaimPlanEntry {
                    address: (*addr).to_string(),
                    kind,
                    category: cat,
                    existing_label_rn: live.claim_labels.get(*cat).cloned(),
                    stale_assoc_rn: None,
                });
            }
        }
        let mut stale: Vec<(&String, &String, &'static str)> = Vec::new();
        for (addr, live_id) in matches {
            let Some(cats) = claims.get(live_id) else {
                continue;
            };
            for cat in cats {
                let Some(tok) = canonical_category(cat) else {
                    continue;
                };
                if !desired.contains(&(addr.as_str(), tok)) {
                    stale.push((addr, live_id, tok));
                }
            }
        }
        stale.sort();
        for (addr, live_id, tok) in stale {
            let Some(label_rn) = live.claim_labels.get(tok) else {
                continue;
            };
            let label_id = label_rn.rsplit('/').next().unwrap_or(label_rn);
            out.push(ClaimPlanEntry {
                address: addr.clone(),
                kind,
                category: tok,
                existing_label_rn: None,
                stale_assoc_rn: Some(format!(
                    "customers/{customer_id}/{assoc_segment}/{live_id}~{label_id}"
                )),
            });
        }
    };

    reconcile(&desired_ag, ad_group_match, &live.ad_group_claims, "ad_group");
    reconcile(&desired_c, campaign_match, &live.campaign_claims, "campaign");
    out
}

/// A live criterion stops being declared in two ways: its whole parent
/// resource is dropped, or — the silent case — a `negative_keyword` /
/// `keyword` block is deleted from a resource that otherwise survives. The
/// first needs `bidsmith:address` labels to prune safely; the second doesn't,
/// because the parent is still declared and the declared members are the
/// authoritative set *within their category*. We prune only inside a
/// `(parent, category)` the file already claims (has ≥1 declared member of),
/// so declaring negatives never deletes positives a user manages elsewhere.
fn orphan_criteria_deletes(
    declared: &ExportInput,
    live: &ExportInput,
    diffs: &[ResourceDiff],
    ad_group_match: &HashMap<String, String>,
    campaign_match: &HashMap<String, String>,
    shared_set_match: &HashMap<String, String>,
) -> (Vec<ResourceDiff>, Vec<String>, usize) {
    let mut out: Vec<ResourceDiff> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut skipped = 0usize;

    let matched_live_ids = |kind: &str| -> std::collections::HashSet<&str> {
        diffs
            .iter()
            .filter(|d| d.kind == kind)
            .filter_map(|d| d.action.live_id())
            .collect()
    };
    let reverse = |m: &HashMap<String, String>| -> HashMap<String, String> {
        m.iter().map(|(addr, id)| (id.clone(), addr.clone())).collect()
    };

    // ---- ad_group_criteria: category = kw polarity / targeting axis ------
    {
        let matched = matched_live_ids("ad_group_criterion");
        let parent_addr = reverse(ad_group_match);
        let mut managed: std::collections::HashSet<(String, &'static str)> =
            std::collections::HashSet::new();
        for d in &declared.ad_group_criteria {
            if let Some(live_ag) = ad_group_match.get(&d.ad_group) {
                managed.insert((
                    live_ag.clone(),
                    criterion_category(&d.target, d.negative.unwrap_or(false)),
                ));
            }
        }
        for (live_id, cats) in &live.ad_group_claims {
            if !parent_addr.contains_key(live_id) {
                continue;
            }
            for cat in cats {
                if let Some(tok) = canonical_category(cat) {
                    managed.insert((live_id.clone(), tok));
                }
            }
        }
        for l in &live.ad_group_criteria {
            if matched.contains(l.id.as_str()) {
                continue;
            }
            let negative = l.negative.unwrap_or(false);
            if !managed.contains(&(l.ad_group.clone(), criterion_category(&l.target, negative))) {
                continue;
            }
            let Some(descriptor) = criterion_descriptor(&l.target, negative) else {
                continue;
            };
            out.push(delete_diff(
                "ad_group_criterion",
                parent_addr.get(&l.ad_group),
                &l.ad_group,
                "adGroups",
                &descriptor,
                &l.id,
            ));
        }
    }

    // ---- campaign_criteria: category = kw polarity / location / language / proximity ----
    {
        let matched = matched_live_ids("campaign_criterion");
        let parent_addr = reverse(campaign_match);
        let mut managed: std::collections::HashSet<(String, &'static str)> =
            std::collections::HashSet::new();
        for d in &declared.campaign_criteria {
            if let Some(live_c) = campaign_match.get(&d.campaign) {
                managed.insert((
                    live_c.clone(),
                    criterion_category(&d.target, d.negative.unwrap_or(false)),
                ));
            }
        }
        for (live_id, cats) in &live.campaign_claims {
            if !parent_addr.contains_key(live_id) {
                continue;
            }
            for cat in cats {
                if let Some(tok) = canonical_category(cat) {
                    managed.insert((live_id.clone(), tok));
                }
            }
        }
        for l in &live.campaign_criteria {
            if matched.contains(l.id.as_str()) {
                continue;
            }
            let negative = l.negative.unwrap_or(false);
            let category = criterion_category(&l.target, negative);
            if !managed.contains(&(l.campaign.clone(), category)) {
                continue;
            }
            let Some(descriptor) = criterion_descriptor(&l.target, negative) else {
                continue;
            };
            // Device criteria can never be removed via the API, and Google
            // auto-materializes every device type once one exists — a remove
            // op is guaranteed to reject and sinks the whole atomic batch.
            if category == "device" {
                if device_criterion_has_adjustment(l) {
                    let anchor = parent_addr
                        .get(&l.campaign)
                        .cloned()
                        .unwrap_or_else(|| format!("campaigns/{}", l.campaign));
                    let detail = match l.bid_modifier {
                        Some(m) => format!("bid_modifier {m}"),
                        None => "negative".to_string(),
                    };
                    warnings.push(format!(
                        "{anchor}: live {descriptor} ({detail}) is not declared, and Google Ads \
                         forbids removing device criteria — leaving it untouched. Declare a \
                         matching device block to manage it (omit bid_modifier to reset the \
                         adjustment)."
                    ));
                    skipped += 1;
                }
                continue;
            }
            out.push(delete_diff(
                "campaign_criterion",
                parent_addr.get(&l.campaign),
                &l.campaign,
                "campaigns",
                &descriptor,
                &l.id,
            ));
        }
    }

    // ---- shared_criteria: one category (set membership) -----------------
    {
        let matched = matched_live_ids("shared_criterion");
        let parent_addr = reverse(shared_set_match);
        let resolve_set = |declared_ref: &str| -> Option<String> {
            shared_set_match.get(declared_ref).cloned().or_else(|| {
                declared_ref
                    .starts_with("customers/")
                    .then(|| declared_ref.rsplit('/').next().map(str::to_string))
                    .flatten()
            })
        };
        // Still gated on ≥1 declared member: sets match by bare name, so an
        // empty declared set adopting a UI-curated one must not empty it.
        let mut managed: std::collections::HashSet<String> = std::collections::HashSet::new();
        for d in &declared.shared_criteria {
            if let Some(set_id) = resolve_set(&d.shared_set) {
                managed.insert(set_id);
            }
        }
        for l in &live.shared_criteria {
            if matched.contains(l.id.as_str()) {
                continue;
            }
            if !managed.contains(&l.shared_set) {
                continue;
            }
            let descriptor =
                format!("negative_keyword \"{}\" {}", l.keyword.text, l.keyword.match_type);
            out.push(delete_diff(
                "shared_criterion",
                parent_addr.get(&l.shared_set),
                &l.shared_set,
                "sharedSets",
                &descriptor,
                &l.id,
            ));
        }
    }

    (out, warnings, skipped)
}

/// A live asset link stops being declared the same two ways a criterion does,
/// and is pruned under the same rule: only inside a `(parent, field type)` the
/// file owns, so declaring a campaign's sitelinks never detaches its callouts.
/// A campaign / ad group proves ownership the way it does for criteria — ≥1
/// declared link, or a `bidsmith:owns` claim label from a previous apply. The
/// account has nowhere to carry a claim (the API has no customer label bidsmith
/// can write), so account-wide ownership is claimed in the file instead, by the
/// `provider` block's `owns` list.
///
/// What Google's automation attached is in scope on the same terms, but ends
/// `PAUSED` rather than destroyed, and needs `owns = ["automatically_created_assets"]`
/// wherever no declaration could have proved the claim.
fn orphan_asset_link_deletes(
    declared: &ExportInput,
    live: &ExportInput,
    diffs: &[ResourceDiff],
    campaign_match: &HashMap<String, String>,
    ad_group_match: &HashMap<String, String>,
    live_video_campaigns: &HashSet<&str>,
    live_video_ad_groups: &HashSet<&str>,
) -> (Vec<ResourceDiff>, Vec<String>, usize) {
    let mut out: Vec<ResourceDiff> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut skipped = 0usize;
    let mut automatic: BTreeMap<&str, usize> = BTreeMap::new();

    // Live campaign ids whose file claims what Google invented for them. An ad
    // group inherits the claim: an automation asset lands on whichever level
    // Google chose, and no ad-group block could have named it either.
    let campaigns_owning_automatic: HashSet<&str> = declared
        .campaigns
        .iter()
        .filter(|c| c.owns_automatic_assets)
        .filter_map(|c| campaign_match.get(&c.id).map(String::as_str))
        .collect();
    let ad_groups_owning_automatic: HashSet<&str> = live
        .ad_groups
        .iter()
        .filter(|g| campaigns_owning_automatic.contains(g.campaign.as_str()))
        .map(|g| g.id.as_str())
        .collect();

    let matched_live_ids = |kind: &str| -> HashSet<&str> {
        diffs
            .iter()
            .filter(|d| d.kind == kind)
            .filter_map(|d| d.action.live_id())
            .collect()
    };
    let reverse = |m: &HashMap<String, String>| -> HashMap<String, String> {
        m.iter().map(|(addr, id)| (id.clone(), addr.clone())).collect()
    };
    let describe = |asset_id: &str, field_type: &str| -> String {
        asset_descriptor(live, asset_id, field_type)
    };

    // ---- campaign_asset ---------------------------------------------------
    {
        let matched = matched_live_ids("campaign_asset");
        let parent_addr = reverse(campaign_match);
        let mut managed: HashSet<(String, &'static str)> = HashSet::new();
        // Keyed off the declared campaigns only, never a literal resource name:
        // attaching an asset to a campaign the file does not otherwise manage
        // says nothing about what else may hang off it.
        for d in &declared.campaign_assets {
            if let Some(live_c) = campaign_match.get(&d.campaign) {
                if let Some(cat) = asset_link_category(&d.field_type) {
                    managed.insert((live_c.clone(), cat));
                }
            }
        }
        for (live_id, cats) in &live.campaign_claims {
            if !parent_addr.contains_key(live_id) {
                continue;
            }
            for cat in cats {
                if let Some(tok) = canonical_category(cat).filter(|t| is_asset_category(t)) {
                    managed.insert((live_id.clone(), tok));
                }
            }
        }
        for l in &live.campaign_assets {
            if matched.contains(l.id.as_str()) || is_removed(l.status.as_deref()) {
                continue;
            }
            let automatic_here = is_automatic(l.source.as_deref());
            let owned = asset_link_category(&l.field_type)
                .is_some_and(|cat| managed.contains(&(l.campaign.clone(), cat)))
                || (automatic_here && campaigns_owning_automatic.contains(l.campaign.as_str()));
            if !owned {
                if parent_addr.contains_key(&l.campaign) {
                    note_automatic(&mut automatic, l.source.as_deref(), &l.field_type);
                }
                continue;
            }
            let descriptor = describe(&l.asset, &l.field_type);
            let anchor = parent_addr
                .get(&l.campaign)
                .cloned()
                .unwrap_or_else(|| format!("campaigns/{}", l.campaign));
            if let Some(w) = unmutable_link_warning(
                &anchor,
                &descriptor,
                live_video_campaigns
                    .contains(l.campaign.as_str())
                    .then_some("a campaign on the VIDEO channel"),
            ) {
                warnings.push(w);
                skipped += 1;
                continue;
            }
            out.extend(orphan_link_diff(
                "campaign_asset",
                &anchor,
                &descriptor,
                l.id.as_str(),
                l.status.as_deref(),
                automatic_here,
            ));
        }
    }

    // ---- ad_group_asset ---------------------------------------------------
    {
        let matched = matched_live_ids("ad_group_asset");
        let parent_addr = reverse(ad_group_match);
        let mut managed: HashSet<(String, &'static str)> = HashSet::new();
        for d in &declared.ad_group_assets {
            if let Some(live_ag) = ad_group_match.get(&d.ad_group) {
                if let Some(cat) = asset_link_category(&d.field_type) {
                    managed.insert((live_ag.clone(), cat));
                }
            }
        }
        for (live_id, cats) in &live.ad_group_claims {
            if !parent_addr.contains_key(live_id) {
                continue;
            }
            for cat in cats {
                if let Some(tok) = canonical_category(cat).filter(|t| is_asset_category(t)) {
                    managed.insert((live_id.clone(), tok));
                }
            }
        }
        for l in &live.ad_group_assets {
            if matched.contains(l.id.as_str()) || is_removed(l.status.as_deref()) {
                continue;
            }
            let automatic_here = is_automatic(l.source.as_deref());
            let owned = asset_link_category(&l.field_type)
                .is_some_and(|cat| managed.contains(&(l.ad_group.clone(), cat)))
                || (automatic_here && ad_groups_owning_automatic.contains(l.ad_group.as_str()));
            if !owned {
                if parent_addr.contains_key(&l.ad_group) {
                    note_automatic(&mut automatic, l.source.as_deref(), &l.field_type);
                }
                continue;
            }
            let descriptor = describe(&l.asset, &l.field_type);
            let anchor = parent_addr
                .get(&l.ad_group)
                .cloned()
                .unwrap_or_else(|| format!("adGroups/{}", l.ad_group));
            if let Some(w) = unmutable_link_warning(
                &anchor,
                &descriptor,
                live_video_ad_groups
                    .contains(l.ad_group.as_str())
                    .then_some("an ad group on the VIDEO channel"),
            ) {
                warnings.push(w);
                skipped += 1;
                continue;
            }
            out.extend(orphan_link_diff(
                "ad_group_asset",
                &anchor,
                &descriptor,
                l.id.as_str(),
                l.status.as_deref(),
                automatic_here,
            ));
        }
    }

    // ---- customer_asset: claimed by the provider block, never implicitly --
    {
        let matched = matched_live_ids("customer_asset");
        for l in &live.customer_assets {
            if matched.contains(l.id.as_str()) || is_removed(l.status.as_deref()) {
                continue;
            }
            let automatic_here = is_automatic(l.source.as_deref());
            let owned = declared.owned_account_assets.contains(&l.field_type)
                || (automatic_here && declared.owns_account_automatic_assets);
            if !owned {
                note_automatic(&mut automatic, l.source.as_deref(), &l.field_type);
                continue;
            }
            let descriptor = format!("account-level {}", describe(&l.asset, &l.field_type));
            out.extend(orphan_link_diff(
                "customer_asset",
                "account",
                &descriptor,
                l.id.as_str(),
                l.status.as_deref(),
                automatic_here,
            ));
        }
    }

    warnings.extend(automatic_asset_warning(&automatic));
    (out, warnings, skipped)
}

/// Count one live link Google attached and nothing declared, where no ownership
/// rule reaches it — so it would otherwise leave no trace in the plan at all.
fn note_automatic<'a>(
    counts: &mut BTreeMap<&'a str, usize>,
    source: Option<&str>,
    field_type: &'a str,
) {
    if source == Some(AUTOMATICALLY_CREATED) {
        *counts.entry(field_type).or_default() += 1;
    }
}

/// What Google's automation is serving on the resources bidsmith manages. The
/// account-level switch behind these has no field in the Google Ads API — not
/// on `customer`, not anywhere — so reporting it on every plan is the whole of
/// what bidsmith can do about it, and is what catches someone flipping it back
/// on in the UI (issue #152).
fn automatic_asset_warning(counts: &BTreeMap<&str, usize>) -> Option<String> {
    let total: usize = counts.values().sum();
    if total == 0 {
        return None;
    }
    let breakdown = counts
        .iter()
        .map(|(field_type, n)| format!("{n} {field_type}"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "account: Google's asset automation is serving {total} asset(s) nothing declares \
         ({breakdown}). Add `owns = [\"automatically_created_assets\"]` to the campaign — or to \
         the `provider` block for the account-level ones — and bidsmith will pause them so they \
         stop serving. The switch that makes them is not in the Google Ads API, so pausing is \
         what bidsmith can do about it; turning it off outright is a Google Ads UI setting."
    ))
}

/// Copy the live automations the file has no attribute for onto the campaign
/// that is about to be written, so the whole-list write puts them back exactly
/// as the account held them. Without this, declaring one automation would
/// silently return every automation this build cannot name to Google's default
/// — a loss the plan row could not even show, since there is no attribute to
/// show it under. Runs after the diff because it needs the campaign match.
pub fn carry_unmodelled_automation(
    declared: &mut ExportInput,
    live: &ExportInput,
    report: &DiffReport,
) {
    let live_by_id: HashMap<&str, &JsonCampaign> =
        live.campaigns.iter().map(|c| (c.id.as_str(), c)).collect();
    let matched: HashMap<&str, &str> = report
        .diffs
        .iter()
        .filter(|d| d.kind == "campaign")
        .filter_map(|d| d.action.live_id().map(|id| (d.address.as_str(), id)))
        .collect();
    for c in &mut declared.campaigns {
        let carried = matched
            .get(c.id.as_str())
            .and_then(|id| live_by_id.get(id))
            .and_then(|l| l.asset_automation_settings.as_ref())
            .map(|ls| ls.unmodelled.clone());
        let (Some(carried), Some(settings)) = (carried, c.asset_automation_settings.as_mut())
        else {
            continue;
        };
        settings.unmodelled = carried;
    }
}

/// The link status that stops an asset serving without detaching it.
const PAUSED: &str = "PAUSED";

fn is_automatic(source: Option<&str>) -> bool {
    source == Some(AUTOMATICALLY_CREATED)
}

/// Why a link the file no longer declares is left attached anyway: the account
/// would reject the operation, and the batch is atomic — one doomed operation
/// takes every unrelated one down with it (issue #116).
fn unmutable_link_warning(
    anchor: &str,
    descriptor: &str,
    read_only_parent: Option<&str>,
) -> Option<String> {
    read_only_parent.map(|what| {
        format!(
            "{anchor}: live {descriptor} is not declared, but it hangs off {what} and the \
             Google Ads API cannot mutate those \
             (see developers.google.com/google-ads/api/docs/video/overview) — leaving it alone \
             so the rest of the batch can go through."
        )
    })
}

/// What to do with a live link inside a scope the file owns but does not
/// declare. An advertiser put it there, so it goes; Google's automation put it
/// there and keeps putting it there, so it is switched off instead and the
/// account converges. One already switched off is where the file wants it.
fn orphan_link_diff(
    kind: &'static str,
    anchor: &str,
    descriptor: &str,
    live_id: &str,
    live_status: Option<&str>,
    automatic: bool,
) -> Option<ResourceDiff> {
    if !automatic {
        return Some(ResourceDiff {
            address: format!("{anchor} (removed {descriptor})"),
            kind,
            action: Action::Delete {
                live_id: live_id.to_string(),
            },
        });
    }
    if live_status == Some(PAUSED) {
        return None;
    }
    Some(ResourceDiff {
        address: format!("{anchor} (paused {descriptor})"),
        kind,
        action: Action::Pause {
            live_id: live_id.to_string(),
        },
    })
}

/// How a live asset link reads in a destroy row: what it puts on the page, not
/// the numeric id nobody recognises.
fn asset_descriptor(live: &ExportInput, asset_id: &str, field_type: &str) -> String {
    let found = match field_type {
        "SITELINK" => live
            .sitelink_assets
            .iter()
            .find(|a| a.id == asset_id)
            .map(|a| format!("sitelink {:?}", a.link_text)),
        "CALLOUT" => live
            .callout_assets
            .iter()
            .find(|a| a.id == asset_id)
            .map(|a| format!("callout {:?}", a.text)),
        "STRUCTURED_SNIPPET" => live
            .structured_snippet_assets
            .iter()
            .find(|a| a.id == asset_id)
            .map(|a| format!("structured_snippet {:?}", a.header)),
        "CALL" => live
            .call_assets
            .iter()
            .find(|a| a.id == asset_id)
            .map(|a| format!("call {:?}", a.phone_number)),
        _ => None,
    };
    found.unwrap_or_else(|| format!("{} asset {asset_id}", field_type.to_lowercase()))
}

/// The `bidsmith:owns` category an asset link falls in — its field type, so
/// ownership of a campaign's sitelinks says nothing about its callouts.
fn asset_link_category(field_type: &str) -> Option<&'static str> {
    Some(match field_type {
        "SITELINK" => "asset_sitelink",
        "CALLOUT" => "asset_callout",
        "STRUCTURED_SNIPPET" => "asset_structured_snippet",
        "CALL" => "asset_call",
        _ => return None,
    })
}

fn is_asset_category(category: &str) -> bool {
    category.starts_with("asset_")
}

/// True when a live device criterion deviates from its default state — the
/// state Google's auto-materialized criteria are born in (no modifier, not
/// negative). Default-state criteria are implicitly desired, not drift.
fn device_criterion_has_adjustment(cr: &JsonCampaignCriterion) -> bool {
    cr.negative.unwrap_or(false) || cr.bid_modifier.is_some_and(|m| (m - 1.0).abs() > 1e-6)
}

/// A `REMOVED` live resource can no longer be mutated: the Google Ads API
/// rejects every op against it, and one such op sinks the whole atomic batch.
/// Removed resources keep their `bidsmith:address` label, so without this guard
/// a removal that succeeded would re-plan as a doomed destroy on every
/// subsequent plan.
fn is_removed(status: Option<&str>) -> bool {
    status == Some("REMOVED")
}

/// The module `address` belongs to when this run never read that module's
/// file, or `None` when the resource is in scope and its absence from the
/// declaration therefore means what it says.
///
/// A label payload is the address, truncated on the *tail* when it is too long
/// for Google's 80-char cap, so the module segment at the head survives — a
/// module name that did not survive matches nothing and the resource is left
/// alone, which is the safe direction (issue #160).
fn unread_module<'a>(declared: &ExportInput, address: &'a str) -> Option<&'a str> {
    let read = declared.partial_modules.as_ref()?;
    let module = address.split('.').next().unwrap_or(address);
    (!read.contains(module)).then_some(module)
}

/// Said once per plan rather than per resource: the count is in the summary,
/// and what a reader needs here is which files to add to the input.
fn out_of_scope_warning(count: usize, modules: &BTreeSet<&str>) -> String {
    let names: Vec<&str> = modules.iter().copied().collect();
    format!(
        "{count} labeled resource(s) belong to module(s) this run did not read ({}) and were \
         left alone. Only an input that covers the whole project can tell \"deleted from the \
         files\" apart from \"not in this file\" — point plan / apply at the project root to \
         reconcile them.",
        names.join(", "),
    )
}

/// A whole labeled resource that is no longer declared — destroyed because its
/// `bidsmith:address` label proves bidsmith owns it. Unlabeled live resources
/// (UI-created, unmanaged) are never produced here.
fn removal_diff(kind: &'static str, address: &str, live_id: &str) -> ResourceDiff {
    ResourceDiff {
        address: format!("{address} (managed, no longer declared)"),
        kind,
        action: Action::Delete {
            live_id: live_id.to_string(),
        },
    }
}

fn delete_diff(
    kind: &'static str,
    parent_addr: Option<&String>,
    parent_live_id: &str,
    parent_segment: &str,
    descriptor: &str,
    live_id: &str,
) -> ResourceDiff {
    let anchor = parent_addr
        .cloned()
        .unwrap_or_else(|| format!("{parent_segment}/{parent_live_id}"));
    ResourceDiff {
        address: format!("{anchor} (removed {descriptor})"),
        kind,
        action: Action::Delete {
            live_id: live_id.to_string(),
        },
    }
}

/// The `bidsmith:owns` category a criterion falls in — the partition orphan
/// pruning runs inside, so declaring one axis never deletes another.
fn criterion_category(cr: &JsonCriterion, negative: bool) -> &'static str {
    if cr.keyword.is_some() {
        polarity_category(negative)
    } else if cr.location.is_some() {
        "location"
    } else if cr.language.is_some() {
        "language"
    } else if cr.proximity.is_some() {
        "proximity"
    } else if cr.device.is_some() {
        "device"
    } else if cr.youtube_channel.is_some() {
        "youtube_channel"
    } else if cr.youtube_video.is_some() {
        "youtube_video"
    } else if cr.topic.is_some() {
        "topic"
    } else if cr.placement.is_some() {
        "placement"
    } else if cr.user_interest.is_some() {
        "user_interest"
    } else if cr.age_range.is_some() {
        "age_range"
    } else if cr.gender.is_some() {
        "gender"
    } else if cr.parental_status.is_some() {
        "parental_status"
    } else if cr.income_range.is_some() {
        "income_range"
    } else if cr.audience.is_some() {
        "audience"
    } else {
        "other"
    }
}

fn criterion_descriptor(cr: &JsonCriterion, negative: bool) -> Option<String> {
    if let Some(kw) = &cr.keyword {
        let word = if negative { "negative_keyword" } else { "keyword" };
        Some(format!("{word} \"{}\" {}", kw.text, kw.match_type))
    } else if let Some(loc) = &cr.location {
        Some(format!("location {}", loc.geo_target_constant))
    } else if let Some(lang) = &cr.language {
        Some(format!("language {}", lang.language_constant))
    } else if let Some(dev) = &cr.device {
        Some(format!("device {}", dev.ty))
    } else if let Some(c) = &cr.youtube_channel {
        Some(format!("youtube_channel {}", c.channel_id))
    } else if let Some(v) = &cr.youtube_video {
        Some(format!("youtube_video {}", v.video_id))
    } else if let Some(t) = &cr.topic {
        Some(format!("topic {}", t.topic_constant))
    } else if let Some(p) = &cr.placement {
        Some(format!("placement {}", p.url))
    } else if let Some(u) = &cr.user_interest {
        Some(format!("user_interest {}", u.user_interest_category))
    } else if let Some(a) = &cr.age_range {
        Some(format!("age_range {}", a.ty))
    } else if let Some(g) = &cr.gender {
        Some(format!("gender {}", g.ty))
    } else if let Some(p) = &cr.parental_status {
        Some(format!("parental_status {}", p.ty))
    } else if let Some(i) = &cr.income_range {
        Some(format!("income_range {}", i.ty))
    } else if let Some((field, value)) = cr.audience.as_ref().and_then(JsonAudience::source) {
        Some(format!("{field} {value}"))
    } else {
        cr.proximity.as_ref().map(|_| "proximity".to_string())
    }
}

fn action_for_match(live_id: String, changed: Vec<FieldChange>) -> Action {
    if changed.is_empty() {
        Action::NoOp { live_id }
    } else {
        Action::Update {
            live_id,
            changed_fields: changed,
        }
    }
}

/// Google Ads accepts a budget on a second campaign only when it is
/// `explicitly_shared`. The trap is that `explicitly_shared` defaults to
/// `false` and bidsmith fills that default in, so the file that earns the
/// rejection never mentions the field — and the rejection lands on the second
/// campaign, while the first reports a misleading "Resource was not found".
/// Grouped by resolved address: five modules each declaring their own local
/// `budget` are five budgets, not one shared five ways.
fn shared_budget_warnings(declared: &ExportInput) -> Vec<String> {
    let mut users: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in &declared.campaigns {
        users
            .entry(c.campaign_budget.as_str())
            .or_default()
            .push(c.id.as_str());
    }
    let mut out = Vec::new();
    for b in &declared.campaign_budgets {
        if b.explicitly_shared == Some(true) {
            continue;
        }
        let Some(campaigns) = users.get(b.id.as_str()) else {
            continue;
        };
        if campaigns.len() < 2 {
            continue;
        }
        out.push(format!(
            "{} backs {} campaigns ({}) but is not explicitly shared — Google Ads rejects the \
             second one; set 'explicitly_shared = true' on the budget, or give each campaign \
             its own",
            b.id,
            campaigns.len(),
            campaigns.join(", "),
        ));
    }
    out
}

/// A budget's `period` and `type` are fixed at creation, so the diff skips
/// them — which would let a file describing a daily budget adopt a lifetime one
/// and still report a clean plan, with the declared `amount_micros` silently
/// ignored by Google Ads (issue #131).
fn budget_immutable_warnings(address: &str, d: &JsonBudget, l: &JsonBudget) -> Vec<String> {
    let mut out = Vec::new();
    if let (Some(dp), Some(lp)) = (d.period.as_deref(), l.period.as_deref()) {
        if dp != lp {
            out.push(format!(
                "{address} declares period = {dp:?} but the live budget it matched is {lp:?}. A \
                 budget's period is fixed when it is created, so bidsmith can never reconcile \
                 that — a {} budget spends 'total_amount_micros' over its lifetime and ignores \
                 'amount_micros'",
                CUSTOM_PERIOD,
            ));
        }
    }
    if let (Some(dt), Some(lt)) = (d.ty.as_deref(), l.ty.as_deref()) {
        if dt != lt {
            out.push(format!(
                "{address} declares type = {dt:?} but the live budget it matched is {lt:?}. A \
                 budget's type is fixed when it is created, so bidsmith can never reconcile \
                 that — check the file is pointing at the budget you think it is"
            ));
        }
    }
    out
}

/// A campaign's channel and its sub-type are both fixed at creation, so the
/// diff skips them — which means a file naming one and matching a live campaign
/// on another reports as a clean adoption. The plan says so out loud instead:
/// the file is describing a campaign it is not pointing at (issues #112, #133).
fn campaign_immutable_warnings(address: &str, d: &JsonCampaign, l: &JsonCampaign) -> Vec<String> {
    let mut out = Vec::new();
    if !d.advertising_channel_type.is_empty()
        && !l.advertising_channel_type.is_empty()
        && d.advertising_channel_type != l.advertising_channel_type
    {
        out.push(format!(
            "{address} declares advertising_channel_type = {:?} but the live campaign it matched \
             is {:?}. A campaign's channel is fixed when it is created, so bidsmith can never \
             reconcile that — check the file is pointing at the campaign you think it is",
            d.advertising_channel_type, l.advertising_channel_type,
        ));
    }
    // Only when the file says something. An omitted sub-type is unmanaged, like
    // every other omitted field, not an assertion that the campaign has none.
    if let Some(ds) = d.advertising_channel_sub_type.as_deref() {
        if Some(ds) != l.advertising_channel_sub_type.as_deref() {
            let live = match l.advertising_channel_sub_type.as_deref() {
                Some(ls) => format!("{ls:?}"),
                None => "not set".to_string(),
            };
            out.push(format!(
                "{address} declares advertising_channel_sub_type = {ds:?} but the live campaign \
                 it matched is {live}. A campaign's sub-type is fixed when it is created, so \
                 bidsmith can never reconcile that — the two campaigns are different formats"
            ));
        }
    }
    out
}

/// Google Ads requires a bidding strategy to *create* a campaign, and rejects
/// the create with a bare "The required field was not present." that names no
/// field. Only creates: an adopted campaign keeps whatever the account bids
/// with, which is why this can't live in the offline lint (issue #104).
fn missing_bidding_strategy_warning(d: &JsonCampaign) -> Option<String> {
    // A VIDEO campaign can't be created at all; the stronger warning covers it.
    if d.bidding_strategy().is_some() || d.advertising_channel_type == "VIDEO" {
        return None;
    }
    Some(format!(
        "{} is a new {} campaign with no bidding strategy — Google Ads will reject the create; \
         add a bidding block (e.g. 'manual_cpc')",
        d.id, d.advertising_channel_type
    ))
}

/// Every create or update against the VIDEO channel is rejected, and because
/// the batch is atomic it takes every unrelated operation with it — so the
/// plan stops here rather than letting Google decide (issues #104, #116). The
/// restriction covers the channel, not just the campaign resource: a bid or
/// status update on a video campaign's ad group is refused the same way
/// (issue #109). Label writes are allowed, so `~ adopt` stays quiet.
///
/// A create or update is blocked rather than skipped because the file is
/// asserting a state the account can never reach; quietly dropping it would
/// leave the repo and the account permanently disagreeing under a green plan.
/// A *removal* is skipped instead — see `diff` — since a file that stopped
/// mentioning a resource is not asserting anything about it.
fn video_is_read_only_blocker(address: &str, action: &Action) -> Option<String> {
    let what = match action {
        Action::Create => "would be created".to_string(),
        Action::Update { changed_fields, .. } => {
            format!(
                "has drift on {}",
                changed_fields
                    .iter()
                    .map(FieldChange::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        _ => return None,
    };
    Some(format!(
        "{address} {what}. Nothing in the batch can be sent while it is there, because {}",
        crate::schema::VIDEO_IS_READ_ONLY
    ))
}

/// Resources the file declared adopt-only (`lifecycle { create = false }`)
/// that matched nothing live. Without this the plan silently degrades into a
/// create — which for a VIDEO campaign Google then rejects, taking every
/// unrelated operation in the atomic batch with it. Saying "expected to adopt,
/// found nothing" names the real problem, and names the key the match is on so
/// the fix is obvious (issue #115).
fn adopt_only_blockers(declared: &ExportInput, diffs: &[ResourceDiff]) -> Vec<String> {
    if declared.adopt_only.is_empty() {
        return Vec::new();
    }
    diffs
        .iter()
        .filter(|d| matches!(d.action, Action::Create))
        .filter(|d| declared.adopt_only.contains(&d.address))
        .map(|d| {
            let by = match adopt_match_key(declared, d.kind, &d.address) {
                Some(name) => format!("by name {name:?}"),
                None => "by content".to_string(),
            };
            format!(
                "{} is declared adopt-only (lifecycle {{ create = false }}) but no live {} \
                 matched it, so there is nothing to adopt. bidsmith looks for its \
                 bidsmith:address label first, then {by}. Create it in the Google Ads UI to \
                 match, or drop the lifecycle block to let bidsmith create it",
                d.address,
                d.kind.replace('_', " "),
            )
        })
        .collect()
}

/// The content key a declared resource falls back to when no `bidsmith:address`
/// label matches — the thing a reviewer has to keep in sync by hand, and so the
/// thing the adopt-only error has to name. `None` for kinds matched on their
/// whole body rather than a name.
fn adopt_match_key<'a>(
    declared: &'a ExportInput,
    kind: &str,
    address: &str,
) -> Option<&'a str> {
    let find = |name: Option<&'a String>| name.map(String::as_str);
    match kind {
        "campaign_budget" => find(
            declared.campaign_budgets.iter().find(|b| b.id == address).map(|b| &b.name),
        ),
        "campaign" => find(declared.campaigns.iter().find(|c| c.id == address).map(|c| &c.name)),
        "ad_group" => find(declared.ad_groups.iter().find(|g| g.id == address).map(|g| &g.name)),
        "conversion_action" => find(
            declared.conversion_actions.iter().find(|c| c.id == address).map(|c| &c.name),
        ),
        "shared_set" => {
            find(declared.shared_sets.iter().find(|s| s.id == address).map(|s| &s.name))
        }
        "custom_audience" => {
            find(declared.custom_audiences.iter().find(|a| a.id == address).map(|a| &a.name))
        }
        _ => None,
    }
}

/// A labeled live resource the file no longer declares, on a channel whose
/// removals the API refuses. Nothing is lost by leaving it alone, and skipping
/// it lets every unrelated operation through (issue #116).
fn video_removal_skipped_warning(what: &str, address: &str) -> String {
    format!(
        "{address} is labeled by bidsmith but no longer declared, and the Google Ads API \
         cannot remove {what} (see developers.google.com/google-ads/api/docs/video/overview) — \
         skipping the removal so the rest of the batch can go through. Delete it in the Google \
         Ads UI, or restore the declaration, to make this quiet."
    )
}

fn diff_budget(d: &JsonBudget, l: &JsonBudget) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.name != l.name {
        c.push(change("name", &l.name, &d.name));
    }
    // Only the amount the declared period spends. The other one is whatever the
    // account happens to carry, and writing it is an API error.
    if d.is_custom_period() {
        if d.total_amount_micros != l.total_amount_micros {
            c.push(change(
                "total_amount_micros",
                l.total_amount_micros,
                d.total_amount_micros,
            ));
        }
    } else if d.amount_micros != l.amount_micros {
        c.push(change("amount_micros", l.amount_micros, d.amount_micros));
    }
    if d.delivery_method != l.delivery_method {
        c.push(change("delivery_method", &l.delivery_method, &d.delivery_method));
    }
    if d.explicitly_shared != l.explicitly_shared {
        c.push(change("explicitly_shared", l.explicitly_shared, d.explicitly_shared));
    }
    c
}

/// `caps_claimed` is the live `bidsmith:owns=frequency_caps` association: caps
/// are only bidsmith's to reconcile once the file declared some. Without that
/// gate, a campaign that never mentions `frequency_caps` would read as "desired
/// = no caps" and plan a clear of whatever the Google Ads UI set (issue #102).
/// The tracking-template pair, compared the same way at every level. Omitted
/// means unmanaged, as everywhere else — a file that says nothing about the
/// suffix is not asking to clear the one the account is appending.
fn diff_tracking(
    c: &mut Vec<FieldChange>,
    desired: (&Option<String>, &Option<Vec<JsonCustomParameter>>),
    live: (&Option<String>, &Option<Vec<JsonCustomParameter>>),
) {
    if desired.0.is_some() && desired.0 != live.0 {
        c.push(change("final_url_suffix", live.0, desired.0));
    }
    if let Some(want) = desired.1 {
        let have = live.1.as_deref().unwrap_or(&[]);
        if want.as_slice() != have {
            c.push(change(
                "custom_parameters",
                render_custom_parameters(have),
                render_custom_parameters(want),
            ));
        }
    }
}

fn render_custom_parameters(params: &[JsonCustomParameter]) -> String {
    if params.is_empty() {
        return "{}".to_string();
    }
    format!(
        "{{{}}}",
        params
            .iter()
            .map(|p| format!("{}={}", p.key, p.value))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn diff_campaign(d: &JsonCampaign, l: &JsonCampaign, caps_claimed: bool) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.name != l.name {
        c.push(change("name", &l.name, &d.name));
    }
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    if d.contains_eu_political_advertising != l.contains_eu_political_advertising
        && d.contains_eu_political_advertising.is_some()
    {
        c.push(change(
            "contains_eu_political_advertising",
            &l.contains_eu_political_advertising,
            &d.contains_eu_political_advertising,
        ));
    }
    // Omitted means unmanaged, as everywhere else: a file that names no flight
    // window is not asking to clear the one the account has.
    for (field, desired, live) in [
        ("start_date", &d.start_date, &l.start_date),
        ("end_date", &d.end_date, &l.end_date),
    ] {
        if desired.is_some() && desired != live {
            c.push(change(field, live, desired));
        }
    }
    diff_tracking(
        &mut c,
        (&d.final_url_suffix, &d.custom_parameters),
        (&l.final_url_suffix, &l.custom_parameters),
    );
    // advertising_channel_type is creation-only; skip.
    // The bidding strategy is a `oneof`, so a file that declares none leaves it
    // unmanaged rather than asking to clear whatever the account is bidding on.
    match (d.bidding_strategy(), l.bidding_strategy()) {
        (Some(desired), live) if Some(desired) != live => {
            c.push(change(desired, live, Some(desired)))
        }
        (Some("manual_cpc"), _) => {
            let dm = d.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled);
            let lm = l.manual_cpc.as_ref().and_then(|m| m.enhanced_cpc_enabled);
            if dm != lm {
                c.push(change("manual_cpc.enhanced_cpc_enabled", lm, dm));
            }
        }
        (Some("target_impression_share"), _) => {
            let dm = d.target_impression_share.as_ref();
            let lm = l.target_impression_share.as_ref();
            let (dl, ll) = (
                dm.and_then(|t| t.location.as_deref()),
                lm.and_then(|t| t.location.as_deref()),
            );
            if dl != ll {
                c.push(change("target_impression_share.location", ll, dl));
            }
            for (field, desired, live) in [
                (
                    "target_impression_share.location_fraction_micros",
                    dm.and_then(|t| t.location_fraction_micros),
                    lm.and_then(|t| t.location_fraction_micros),
                ),
                (
                    "target_impression_share.cpc_bid_ceiling_micros",
                    dm.and_then(|t| t.cpc_bid_ceiling_micros),
                    lm.and_then(|t| t.cpc_bid_ceiling_micros),
                ),
            ] {
                if desired != live {
                    c.push(change(field, live, desired));
                }
            }
        }
        (Some("target_spend"), _) => {
            let dm = d.target_spend.as_ref().and_then(|t| t.cpc_bid_ceiling_micros);
            let lm = l.target_spend.as_ref().and_then(|t| t.cpc_bid_ceiling_micros);
            if dm != lm {
                c.push(change("target_spend.cpc_bid_ceiling_micros", lm, dm));
            }
        }
        _ => {}
    }
    // Omitted means unmanaged, one network at a time: a file that says nothing
    // about YouTube is not asking to switch it off, and an update mask naming a
    // field the body leaves out is exactly how Google Ads reads a clear.
    for (field, _) in crate::schema::NETWORK_SETTINGS_FIELDS {
        let dv = d.network_settings.as_ref().and_then(|n| n.get(field));
        let lv = l.network_settings.as_ref().and_then(|n| n.get(field));
        if dv.is_some() && dv != lv {
            c.push(change(format!("network_settings.{field}"), lv, dv));
        }
    }
    // Omitted means unmanaged: a campaign that says nothing about how its geo
    // targets are read leaves that to the account, and the live side may report
    // a value this schema doesn't model (`UNKNOWN`) that no file could match.
    for (field, _) in crate::schema::GEO_TARGET_TYPE_FIELDS {
        let desired = d.geo_target_type_setting.as_ref().and_then(|g| g.get(field));
        let live = l.geo_target_type_setting.as_ref().and_then(|g| g.get(field));
        if desired.is_some() && desired != live {
            c.push(change(
                format!("geo_target_type_setting.{field}"),
                live,
                desired,
            ));
        }
    }
    // Omitted means unmanaged, one inventory at a time. The point of comparing
    // these at all is a format experiment: a campaign declared in-stream-only
    // stays a valid test only while nothing switches Shorts back on.
    for (field, _) in crate::schema::VIDEO_AD_INVENTORY_FIELDS {
        let dv = d.video_ad_inventory(field);
        let lv = l.video_ad_inventory(field);
        if dv.is_some() && dv != lv {
            c.push(change(
                format!("video_campaign_settings.video_ad_inventory_control.{field}"),
                lv,
                dv,
            ));
        }
    }
    // Omitted means unmanaged, one automation at a time — an automation live
    // carries that the file never names cannot be drift, or a block naming one
    // of five would propose the same write on every plan and never converge.
    // The write is still the whole list, since that is all the API takes, so
    // both sides render whole: the row says what the campaign ends up with,
    // including the automations the write drops (issue #152).
    if let Some(a) = d.asset_automation_settings.as_ref().filter(|a| !a.is_empty()) {
        let drifted = crate::schema::ASSET_AUTOMATION_FIELDS.iter().any(|(field, _)| {
            let desired = a.get(field);
            desired.is_some()
                && desired != l.asset_automation_settings.as_ref().and_then(|s| s.get(field))
        });
        if drifted {
            let carried = l
                .asset_automation_settings
                .as_ref()
                .map(|s| &s.unmodelled)
                .filter(|u| !u.is_empty());
            c.push(whole_change(
                "asset_automation_settings",
                shown_asset_automation(l.asset_automation_settings.as_ref(), carried),
                shown_asset_automation(Some(a), carried),
            ));
        }
    }
    // Omitted means unmanaged, as everywhere else — and here that is the state
    // the issue is about: Google decides what an undeclared campaign does, and
    // may decide differently tomorrow. A campaign that declares the switch has
    // pinned it, either way (issue #158).
    let desired_ai_max = d.ai_max_setting.as_ref().and_then(|a| a.enable_ai_max);
    let live_ai_max = l.ai_max_setting.as_ref().and_then(|a| a.enable_ai_max);
    if desired_ai_max.is_some() && desired_ai_max != live_ai_max {
        c.push(change(
            "ai_max_setting.enable_ai_max",
            live_ai_max,
            desired_ai_max,
        ));
    }
    c.extend(diff_targeting_setting(
        d.targeting_setting.as_ref(),
        l.targeting_setting.as_ref(),
    ));
    if (!d.frequency_caps.is_empty() || caps_claimed)
        && sorted_frequency_caps(d) != sorted_frequency_caps(l)
    {
        c.push(change(
            "frequency_caps",
            Raw(shown_frequency_caps(l)),
            Raw(shown_frequency_caps(d)),
        ));
    }
    c
}

/// Omitted means unmanaged, as everywhere else — but a declared block manages
/// the *whole* list, because that is the only thing the API offers: it replaces
/// `target_restrictions` wholesale and removes whatever the body leaves out.
/// Both sides drop the entries that say what an absent one would, so Google's
/// own defaults never read as drift (issue #135).
fn diff_targeting_setting(
    d: Option<&JsonTargetingSetting>,
    l: Option<&JsonTargetingSetting>,
) -> Option<FieldChange> {
    let desired = d?.effective();
    let live = l.map(JsonTargetingSetting::effective).unwrap_or_default();
    (desired != live).then(|| {
        change(
            "targeting_setting.target_restrictions",
            Raw(shown_target_restrictions(&live)),
            Raw(shown_target_restrictions(&desired)),
        )
    })
}

/// The restrictions as a reviewer reads them, so a plan row says which
/// dimensions stop narrowing the audience rather than merely that a list moved.
fn shown_target_restrictions(effective: &[(&str, bool)]) -> String {
    if effective.is_empty() {
        return "all targeting".to_string();
    }
    effective
        .iter()
        .map(|(dimension, bid_only)| {
            crate::commands::export::shown_target_restriction(dimension, *bid_only)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The automation list as a reviewer reads it — in the attribute names the
/// file uses, so a plan row says which automations the campaign ends up with
/// rather than merely that the setting moves.
fn shown_asset_automation(
    settings: Option<&JsonAssetAutomationSettings>,
    carried: Option<&BTreeMap<String, String>>,
) -> String {
    let named = crate::schema::ASSET_AUTOMATION_FIELDS
        .iter()
        .filter_map(|(field, _)| Some(format!("{field}={}", settings?.get(field)?)));
    // Under their API name: an automation this build models no attribute for
    // has no other name to render.
    let extra = carried
        .into_iter()
        .flatten()
        .map(|(api, status)| format!("{api}={status}"));
    let shown: Vec<String> = named.chain(extra).collect();
    if shown.is_empty() {
        return "Google's defaults".to_string();
    }
    shown.join(", ")
}

/// The cap list as a reviewer reads it — `3 IMPRESSION / 1 DAY (CAMPAIGN)` per
/// cap — so a plan row says what the cap becomes, not merely that it moves.
fn shown_frequency_caps(c: &JsonCampaign) -> String {
    if c.frequency_caps.is_empty() {
        return "none".to_string();
    }
    sorted_frequency_caps(c)
        .iter()
        .map(|(level, event, unit, length, cap)| {
            format!("{cap} {event} / {length} {unit} ({level})")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The whole cap list is one API field, so it diffs as a set — reordering the
/// blocks in a `.bid` is not a change.
fn sorted_frequency_caps(c: &JsonCampaign) -> Vec<(&str, &str, &str, i64, i64)> {
    let mut caps: Vec<_> = c.frequency_caps.iter().map(|f| f.sort_key()).collect();
    caps.sort_unstable();
    caps
}

fn diff_ad_group(d: &JsonAdGroup, l: &JsonAdGroup) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.name != l.name {
        c.push(change("name", &l.name, &d.name));
    }
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    if d.ty != l.ty {
        c.push(change("type", &l.ty, &d.ty));
    }
    // A bid field the file leaves out is unmanaged, not a request to clear
    // whatever the account is bidding — the same rule the campaign's bidding
    // `oneof` follows. Which field is live depends on the strategy, so an ad
    // group names the one it manages and stays quiet about the rest.
    for (field, _) in crate::schema::AD_GROUP_BID_FIELDS {
        let desired = d.bid(field);
        if desired.is_some() && desired != l.bid(field) {
            c.push(change(*field, l.bid(field), desired));
        }
    }
    diff_tracking(
        &mut c,
        (&d.final_url_suffix, &d.custom_parameters),
        (&l.final_url_suffix, &l.custom_parameters),
    );
    c.extend(diff_targeting_setting(
        d.targeting_setting.as_ref(),
        l.targeting_setting.as_ref(),
    ));
    let desired_matching = d
        .ai_max_ad_group_setting
        .as_ref()
        .and_then(|a| a.disable_search_term_matching);
    let live_matching = l
        .ai_max_ad_group_setting
        .as_ref()
        .and_then(|a| a.disable_search_term_matching);
    if desired_matching.is_some() && desired_matching != live_matching {
        c.push(change(
            "ai_max_ad_group_setting.disable_search_term_matching",
            live_matching,
            desired_matching,
        ));
    }
    c
}

fn diff_ad_group_ad(d: &JsonAdGroupAd, l: &JsonAdGroupAd) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    // The creative is creation-only — a new ad is how you "edit" copy — but the
    // tracking pair is not part of it. The API updates both in place, and
    // recreating an ad to change a UTM slug would throw away its performance
    // history for a string the visitor never sees.
    let mut tracking = Vec::new();
    diff_tracking(
        &mut tracking,
        (&d.ad.final_url_suffix, &d.ad.custom_parameters),
        (&l.ad.final_url_suffix, &l.ad.custom_parameters),
    );
    c.extend(tracking.into_iter().map(|f| FieldChange {
        field: format!("ad.{}", f.field),
        live: f.live,
        desired: f.desired,
    }));
    c
}

/// Match declared ads to live ads 1:1 within each ad group, keyed on the ad
/// *body* (final URLs + RSA content), not on the ad group alone. Accounts often
/// hold several ads with byte-identical bodies that differ only by status
/// (`adblock_rsa_7/8/9`); keying on the ad group and taking the first live ad
/// collapsed them all onto one live id, which surfaced as spurious status
/// updates, "Cannot mutate the same resource twice" rejections, and bogus
/// creates for the unmatched ads. Each live ad is consumed at most once: within
/// a body bucket we first claim the live ads that already match (no diff), then
/// assign the rest as status updates; a declared ad with no live body left is a
/// create.
fn match_ad_group_ads(
    declared: &[JsonAdGroupAd],
    live: &[JsonAdGroupAd],
    ad_group_match: &HashMap<String, String>,
    asset_match: &HashMap<String, String>,
) -> Vec<(Action, Option<usize>)> {
    let mut consumed = vec![false; live.len()];
    let mut out: Vec<(Action, Option<usize>)> = vec![(Action::Create, None); declared.len()];

    // Pass 0: label hits — a declared ad claims the live ad carrying its
    // address, regardless of body. Body is creation-only, so a body edit on a
    // labeled ad falls through to a create + a destroy of the old ad below.
    let mut live_by_addr: HashMap<&str, usize> = HashMap::new();
    for (i, l) in live.iter().enumerate() {
        if let Some(a) = l.managed_address.as_deref() {
            live_by_addr.entry(a).or_insert(i);
        }
    }
    for (di, d) in declared.iter().enumerate() {
        if let Some(&li) = live_by_addr.get(address_label_payload(&d.id).as_str()) {
            if !consumed[li] && ad_bodies_match(d, &live[li], asset_match) {
                consumed[li] = true;
                out[di] = (
                    action_for_match(live[li].id.clone(), diff_ad_group_ad(d, &live[li])),
                    Some(li),
                );
            }
        }
    }

    // Body 1:1 buckets for the rest, keyed on (live ad_group, body). Buckets are
    // disjoint in both declared and live indices (a distinct body key never
    // shares live ads), so the result is independent of bucket order.
    let mut live_buckets: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, l) in live.iter().enumerate() {
        if consumed[i] {
            continue;
        }
        live_buckets
            .entry((l.ad_group.clone(), ad_body_key(l, asset_match)))
            .or_default()
            .push(i);
    }
    let mut declared_buckets: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, d) in declared.iter().enumerate() {
        if out[i].1.is_some() {
            continue;
        }
        if let Some(parent_id) = ad_group_match.get(&d.ad_group) {
            declared_buckets
                .entry((parent_id.clone(), ad_body_key(d, asset_match)))
                .or_default()
                .push(i);
        }
    }

    for (key, decl_indices) in &declared_buckets {
        let live_indices = live_buckets.get(key);
        let mut pending: Vec<usize> = Vec::new();
        for &di in decl_indices {
            let claimed = live_indices.and_then(|lis| {
                lis.iter()
                    .copied()
                    .find(|&li| !consumed[li] && diff_ad_group_ad(&declared[di], &live[li]).is_empty())
            });
            match claimed {
                Some(li) => {
                    consumed[li] = true;
                    out[di] = (
                        Action::NoOp {
                            live_id: live[li].id.clone(),
                        },
                        Some(li),
                    );
                }
                None => pending.push(di),
            }
        }
        for di in pending {
            let claimed = live_indices.and_then(|lis| lis.iter().copied().find(|&li| !consumed[li]));
            if let Some(li) = claimed {
                consumed[li] = true;
                out[di] = (
                    action_for_match(live[li].id.clone(), diff_ad_group_ad(&declared[di], &live[li])),
                    Some(li),
                );
            }
        }
    }

    // Last pass: an `ad {}` with no creative block declares the URLs and leaves
    // the creative unmanaged — what a refresh of a UI-built ad renders. It
    // matches any live ad in the group with the same URLs, creative and all,
    // so adopting one never plans as a destroy plus an ad-less create.
    for (di, d) in declared.iter().enumerate() {
        if out[di].1.is_some() || declares_creative(d) {
            continue;
        }
        let Some(parent_id) = ad_group_match.get(&d.ad_group) else {
            continue;
        };
        let claimed = live
            .iter()
            .enumerate()
            .filter(|(li, l)| {
                !consumed[*li] && &l.ad_group == parent_id && ad_urls_match(d, l)
            })
            .map(|(li, _)| li)
            .min_by_key(|&li| usize::from(!diff_ad_group_ad(d, &live[li]).is_empty()));
        if let Some(li) = claimed {
            consumed[li] = true;
            out[di] = (
                action_for_match(live[li].id.clone(), diff_ad_group_ad(d, &live[li])),
                Some(li),
            );
        }
    }

    out
}

fn declares_creative(a: &JsonAdGroupAd) -> bool {
    a.ad.responsive_search_ad.is_some()
        || a.ad.video_responsive_ad.is_some()
        || a.ad.video_ad.is_some()
        || a.ad.demand_gen_video_responsive_ad.is_some()
}

/// Body match for the label pass: a declared ad that leaves the creative
/// unmanaged compares on URLs alone, everything else on the full body.
fn ad_bodies_match(
    d: &JsonAdGroupAd,
    l: &JsonAdGroupAd,
    asset_match: &HashMap<String, String>,
) -> bool {
    if declares_creative(d) {
        ad_body_key(d, asset_match) == ad_body_key(l, asset_match)
    } else {
        ad_urls_match(d, l)
    }
}

/// The URL match for a creative-less `ad {}` — the "the URLs are mine, the
/// creative is not" shape. `final_urls` always has to agree; the two optional
/// URL fields only when the file names them, because an ad that declares no
/// creative is not asserting anything about the ones it leaves out, and
/// tightening this would re-plan every already-adopted UI-built ad as a create.
fn ad_urls_match(d: &JsonAdGroupAd, l: &JsonAdGroupAd) -> bool {
    d.ad.final_urls == l.ad.final_urls
        && (d.ad.final_mobile_urls.is_empty() || d.ad.final_mobile_urls == l.ad.final_mobile_urls)
        && (d.ad.display_url.is_none() || d.ad.display_url == l.ad.display_url)
}

fn ad_urls_key(a: &JsonAdGroupAd) -> String {
    a.ad.final_urls.join("\u{1f}")
}

/// A stable key for an ad's content (everything `diff_ad_group_ad` treats as
/// creation-only). Status is deliberately excluded so identical-bodied ads in
/// different states share a bucket and get assigned 1:1. Video refs run through
/// `asset_match` so a declared asset address and the live asset id it matched
/// produce the same key; a live id is its own key, so one function serves both
/// sides.
fn ad_body_key(a: &JsonAdGroupAd, asset_match: &HashMap<String, String>) -> String {
    use std::fmt::Write;
    let video_id = |r: &String| asset_match.get(r).unwrap_or(r).clone();
    let mut k = String::new();
    let _ = write!(
        k,
        "urls:{}\u{1e}murls:{}\u{1e}durl:{}",
        ad_urls_key(a),
        a.ad.final_mobile_urls.join("\u{1f}"),
        a.ad.display_url.as_deref().unwrap_or(""),
    );
    if let Some(rsa) = &a.ad.responsive_search_ad {
        k.push_str("\u{1e}h:");
        for h in &rsa.headlines {
            let _ = write!(k, "{}\u{1f}{}\u{1d}", h.text, h.pin.as_deref().unwrap_or(""));
        }
        k.push_str("\u{1e}d:");
        for d in &rsa.descriptions {
            let _ = write!(k, "{}\u{1f}{}\u{1d}", d.text, d.pin.as_deref().unwrap_or(""));
        }
        let _ = write!(
            k,
            "\u{1e}p1:{}\u{1e}p2:{}",
            rsa.path1.as_deref().unwrap_or(""),
            rsa.path2.as_deref().unwrap_or(""),
        );
    }
    if let Some(v) = &a.ad.video_responsive_ad {
        let _ = write!(
            k,
            "\u{1e}video:{}\u{1e}vh:{}\u{1e}vlh:{}\u{1e}vd:{}\u{1e}vcta:{}\u{1e}vb1:{}\u{1e}vb2:{}",
            video_id(&v.video),
            v.headlines.join("\u{1f}"),
            v.long_headlines.join("\u{1f}"),
            v.descriptions.join("\u{1f}"),
            v.call_to_actions.join("\u{1f}"),
            v.breadcrumb1.as_deref().unwrap_or(""),
            v.breadcrumb2.as_deref().unwrap_or(""),
        );
    }
    if let Some(v) = &a.ad.video_ad {
        let _ = write!(k, "\u{1e}va:{}", video_id(&v.video));
    }
    if let Some(dg) = &a.ad.demand_gen_video_responsive_ad {
        // call_to_actions are left out: live Demand Gen CTAs are asset refs, so
        // reading them back never reproduces the declared text and every ad
        // would look like a body change.
        let videos: Vec<String> = dg.videos.iter().map(video_id).collect();
        let _ = write!(
            k,
            "\u{1e}dgvideos:{}\u{1e}dgh:{}\u{1e}dglh:{}\u{1e}dgd:{}\u{1e}dgb1:{}\u{1e}dgb2:{}\u{1e}dgbn:{}",
            videos.join("\u{1f}"),
            dg.headlines.join("\u{1f}"),
            dg.long_headlines.join("\u{1f}"),
            dg.descriptions.join("\u{1f}"),
            dg.breadcrumb1.as_deref().unwrap_or(""),
            dg.breadcrumb2.as_deref().unwrap_or(""),
            dg.business_name.as_deref().unwrap_or(""),
        );
    }
    k
}

fn diff_ad_group_criterion(d: &JsonAdGroupCriterion, l: &JsonAdGroupCriterion) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    if d.negative != l.negative {
        c.push(change("negative", negative_or_default(l.negative), negative_or_default(d.negative)));
    }
    if d.cpc_bid_micros != l.cpc_bid_micros {
        c.push(change("cpc_bid_micros", l.cpc_bid_micros, d.cpc_bid_micros));
    }
    if bid_modifier_changed(d.bid_modifier, l.bid_modifier) {
        c.push(change("bid_modifier", l.bid_modifier, d.bid_modifier));
    }
    // What the criterion targets is creation-only; it is the match key.
    c
}

fn diff_campaign_criterion(
    d: &JsonCampaignCriterion,
    l: &JsonCampaignCriterion,
) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    if d.negative != l.negative {
        c.push(change("negative", negative_or_default(l.negative), negative_or_default(d.negative)));
    }
    if bid_modifier_changed(d.bid_modifier, l.bid_modifier) {
        c.push(change("bid_modifier", l.bid_modifier, d.bid_modifier));
    }
    c
}

/// An omitted `negative` is `false` everywhere else in the diff, so a plan row
/// says `false -> true` rather than the `(unset) -> true` the raw option would
/// print.
fn negative_or_default(v: Option<bool>) -> bool {
    v.unwrap_or(false)
}

fn bid_modifier_changed(d: Option<f64>, l: Option<f64>) -> bool {
    match (d, l) {
        (Some(a), Some(b)) => (a - b).abs() > 1e-6,
        (None, None) => false,
        _ => true,
    }
}

fn diff_conversion_action(d: &JsonConversionAction, l: &JsonConversionAction) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    if d.counting_type != l.counting_type {
        c.push(change("counting_type", &l.counting_type, &d.counting_type));
    }
    if d.click_through_lookback_window_days != l.click_through_lookback_window_days {
        c.push(change(
            "click_through_lookback_window_days",
            l.click_through_lookback_window_days,
            d.click_through_lookback_window_days,
        ));
    }
    if d.view_through_lookback_window_days != l.view_through_lookback_window_days {
        c.push(change(
            "view_through_lookback_window_days",
            l.view_through_lookback_window_days,
            d.view_through_lookback_window_days,
        ));
    }
    let dv = d.value_settings.as_ref().and_then(|v| v.default_value);
    let lv = l.value_settings.as_ref().and_then(|v| v.default_value);
    if dv != lv {
        c.push(change("value_settings.default_value", lv, dv));
    }
    let dc = d
        .value_settings
        .as_ref()
        .and_then(|v| v.default_currency_code.clone());
    let lc = l
        .value_settings
        .as_ref()
        .and_then(|v| v.default_currency_code.clone());
    if dc != lc {
        c.push(change("value_settings.default_currency_code", &lc, &dc));
    }
    let da = d
        .value_settings
        .as_ref()
        .and_then(|v| v.always_use_default_value);
    let la = l
        .value_settings
        .as_ref()
        .and_then(|v| v.always_use_default_value);
    if da != la {
        c.push(change("value_settings.always_use_default_value", la, da));
    }
    c
}

fn diff_call_asset(_d: &JsonCallAsset, _l: &JsonCallAsset) -> Vec<FieldChange> {
    Vec::new()
}

/// Live resources grouped by the content key adoption matches them on. Assets
/// carry no `bidsmith:address` label — the API refuses to label them — so
/// content is the only identity they have, and it is not guaranteed unique.
fn content_index<T>(items: &[T], key: impl Fn(&T) -> String) -> HashMap<String, Vec<&T>> {
    let mut out: HashMap<String, Vec<&T>> = HashMap::new();
    for item in items {
        out.entry(key(item)).or_default().push(item);
    }
    out
}

/// A declaration that several live resources answer to equally. The `.bid` says
/// nothing that could tell them apart, so adoption takes the first and the rest
/// stay unmanaged — worth saying out loud, because the account keeps serving a
/// duplicate the file cannot see.
fn ambiguity_warning<T>(
    address: &str,
    candidates: &[&T],
    id: impl Fn(&T) -> &String,
) -> Option<String> {
    if candidates.len() < 2 {
        return None;
    }
    let ids: Vec<&str> = candidates.iter().map(|c| id(c).as_str()).collect();
    Some(format!(
        "{address}: {} live resources are identical to it ({}) — adopted {}, the rest \
         stay unmanaged. Remove the duplicates in the account, or declare them too so \
         bidsmith owns each one.",
        ids.len(),
        ids.join(", "),
        ids[0],
    ))
}

// Assets are content-immutable in the Google Ads API: editing a sitelink's text
// mints a new asset. So identity is the full content tuple, and there are no
// in-place field updates — a content change plans as create (of a new asset).
fn sitelink_asset_key(a: &JsonSitelinkAsset) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        a.link_text,
        a.description1.as_deref().unwrap_or(""),
        a.description2.as_deref().unwrap_or(""),
        a.final_urls.join(",")
    )
}

fn structured_snippet_asset_key(a: &JsonStructuredSnippetAsset) -> String {
    format!("{}\u{1f}{}", a.header, a.values.join(","))
}

fn resolve_campaign_id(campaign_match: &HashMap<String, String>, campaign: &str) -> Option<String> {
    campaign_match.get(campaign).cloned().or_else(|| {
        campaign
            .starts_with("customers/")
            .then(|| campaign.rsplit('/').next().map(str::to_string))
            .flatten()
    })
}

fn resolve_ad_group_id(ad_group_match: &HashMap<String, String>, ad_group: &str) -> Option<String> {
    ad_group_match.get(ad_group).cloned().or_else(|| {
        ad_group
            .starts_with("customers/")
            .then(|| ad_group.rsplit('/').next().map(str::to_string))
            .flatten()
    })
}

fn diff_customer_asset(d: &JsonCustomerAsset, l: &JsonCustomerAsset) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    c
}

fn diff_campaign_asset(d: &JsonCampaignAsset, l: &JsonCampaignAsset) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    c
}

fn diff_ad_group_asset(d: &JsonAdGroupAsset, l: &JsonAdGroupAsset) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    c
}

fn diff_shared_set(d: &JsonSharedSet, l: &JsonSharedSet) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    if d.ty != l.ty && d.ty.is_some() {
        c.push(change("type", &l.ty, &d.ty));
    }
    c
}

fn diff_campaign_shared_set(
    d: &JsonCampaignSharedSet,
    l: &JsonCampaignSharedSet,
) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    c
}

fn diff_custom_audience(d: &JsonCustomAudience, l: &JsonCustomAudience) -> Vec<FieldChange> {
    let mut c = Vec::new();
    if d.description != l.description && d.description.is_some() {
        c.push(change("description", &l.description, &d.description));
    }
    if d.status != l.status {
        c.push(change("status", &l.status, &d.status));
    }
    // `type` is creation-only: the API rejects changing what a segment is built from.
    if sorted_members(d) != sorted_members(l) {
        c.push(change(
            "members",
            Raw(shown_members(l)),
            Raw(shown_members(d)),
        ));
    }
    c
}

/// The member list as `keyword:running shoes` pairs, so a plan row says which
/// signals the segment gains or loses rather than only that it moved.
fn shown_members(a: &JsonCustomAudience) -> String {
    if sorted_members(a).is_empty() {
        return "none".to_string();
    }
    sorted_members(a)
        .iter()
        .map(|(ty, value)| format!("{ty}:{value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The member list is one API field, so it diffs as a set — reordering the
/// blocks in a `.bid` is not a change.
fn sorted_members(a: &JsonCustomAudience) -> Vec<(&'static str, &str)> {
    let mut members: Vec<_> = a
        .members
        .iter()
        .filter_map(|m| m.payload().map(|(_, ty, v)| (ty, v)))
        .collect();
    members.sort_unstable();
    members
}

/// The live-comparable token for an audience reference. A declared
/// `google_ads_custom_audience.<name>` resolves through the match to the live
/// id; anything else falls back to the resource name's last segment, so a raw
/// `customers/X/customAudiences/999` and the live row agree on `999`.
fn canonical_audience(value: &str, custom_audience_match: &HashMap<String, String>) -> String {
    match custom_audience_match.get(value) {
        Some(live_id) => live_id.clone(),
        None => value.rsplit('/').next().unwrap_or(value).to_string(),
    }
}

/// The identity of a criterion within its parent — what a declared criterion
/// and a live one have to agree on to be the same targeting. Shared by both
/// criterion resources.
fn criterion_key(
    cr: &JsonCriterion,
    negative: bool,
    custom_audience_match: &HashMap<String, String>,
) -> Option<String> {
    if let Some(kw) = &cr.keyword {
        let polarity = if negative { "neg" } else { "pos" };
        return Some(format!("kw:{polarity}:{}|{}", kw.match_type, kw.text));
    }
    if let Some(loc) = &cr.location {
        return Some(format!("loc:{}", loc.geo_target_constant));
    }
    if let Some(lang) = &cr.language {
        return Some(format!("lang:{}", lang.language_constant));
    }
    if let Some(p) = &cr.proximity {
        return Some(format!(
            "prox:{}:{}:{:.6}:{}",
            (p.latitude * 1_000_000.0).round() as i64,
            (p.longitude * 1_000_000.0).round() as i64,
            p.radius,
            p.radius_units,
        ));
    }
    if let Some(d) = &cr.device {
        return Some(format!("dev:{}", d.ty));
    }
    if let Some(c) = &cr.youtube_channel {
        return Some(format!("ytchan:{}", c.channel_id));
    }
    if let Some(v) = &cr.youtube_video {
        return Some(format!("ytvid:{}", v.video_id));
    }
    if let Some(t) = &cr.topic {
        return Some(format!("topic:{}", t.topic_constant));
    }
    if let Some(p) = &cr.placement {
        return Some(format!("placement:{}", p.url));
    }
    if let Some(u) = &cr.user_interest {
        return Some(format!("interest:{}", u.user_interest_category));
    }
    if let Some(a) = &cr.age_range {
        return Some(format!("age:{}", a.ty));
    }
    if let Some(g) = &cr.gender {
        return Some(format!("gender:{}", g.ty));
    }
    if let Some(p) = &cr.parental_status {
        return Some(format!("parental:{}", p.ty));
    }
    if let Some(i) = &cr.income_range {
        return Some(format!("income:{}", i.ty));
    }
    if let Some((field, value)) = cr.audience.as_ref().and_then(JsonAudience::source) {
        return Some(format!(
            "{field}:{}",
            canonical_audience(value, custom_audience_match)
        ));
    }
    None
}

#[cfg(test)]
mod ad_match_tests {
    use super::*;
    use crate::commands::export::{JsonAd, JsonResponsiveSearchAd, JsonRsaAsset};

    fn ad(id: &str, ad_group: &str, status: &str, headline: &str) -> JsonAdGroupAd {
        JsonAdGroupAd {
            id: id.to_string(),
            ad_group: ad_group.to_string(),
            status: Some(status.to_string()),
            ad: JsonAd {
                name: None,
                final_urls: vec!["https://example.com".to_string()],
                final_mobile_urls: Vec::new(),
                display_url: None,
                final_url_suffix: None,
                custom_parameters: None,
                video_ad: None,
                responsive_search_ad: Some(JsonResponsiveSearchAd {
                    headlines: vec![JsonRsaAsset {
                        text: headline.to_string(),
                        pin: None,
                    }],
                    descriptions: vec![JsonRsaAsset {
                        text: "Block ads everywhere.".to_string(),
                        pin: None,
                    }],
                    path1: None,
                    path2: None,
                }),
                video_responsive_ad: None,
                demand_gen_video_responsive_ad: None,
            },
            managed_address: None,
        }
    }

    fn identity_match() -> HashMap<String, String> {
        // declared resources reference the same live ad-group id directly.
        let mut m = HashMap::new();
        m.insert("ag".to_string(), "ag".to_string());
        m
    }

    #[test]
    fn identical_bodies_match_one_to_one_after_refresh() {
        // Three same-bodied ads differing only by status — the refresh scenario.
        // Each declared ad must claim a distinct live id with no diff.
        let declared = vec![
            ad("rsa_7", "ag", "ENABLED", "Block ads now"),
            ad("rsa_8", "ag", "PAUSED", "Block ads now"),
            ad("rsa_9", "ag", "ENABLED", "Block ads now"),
        ];
        let live = vec![
            ad("100", "ag", "ENABLED", "Block ads now"),
            ad("101", "ag", "PAUSED", "Block ads now"),
            ad("102", "ag", "ENABLED", "Block ads now"),
        ];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        assert!(
            actions.iter().all(|a| matches!(a, Action::NoOp { .. })),
            "expected all no-ops, got {actions:?}"
        );
        let mut ids: Vec<&str> = actions.iter().filter_map(|a| a.live_id()).collect();
        ids.sort();
        assert_eq!(ids, vec!["100", "101", "102"], "each live ad claimed once");
    }

    #[test]
    fn exact_status_preferred_over_status_update() {
        // declared [ENABLED, PAUSED] vs live [PAUSED, ENABLED]: a naive
        // first-come match would update both; correct is two no-ops.
        let declared = vec![
            ad("a", "ag", "ENABLED", "Same body"),
            ad("b", "ag", "PAUSED", "Same body"),
        ];
        let live = vec![
            ad("200", "ag", "PAUSED", "Same body"),
            ad("201", "ag", "ENABLED", "Same body"),
        ];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        assert!(matches!(&actions[0], Action::NoOp { live_id } if live_id == "201"));
        assert!(matches!(&actions[1], Action::NoOp { live_id } if live_id == "200"));
    }

    #[test]
    fn leftover_declared_status_becomes_update() {
        // declared [ENABLED, ENABLED] vs live [ENABLED, PAUSED]: one no-op, one
        // genuine status update — never two updates on the same id.
        let declared = vec![
            ad("a", "ag", "ENABLED", "Same body"),
            ad("b", "ag", "ENABLED", "Same body"),
        ];
        let live = vec![
            ad("300", "ag", "ENABLED", "Same body"),
            ad("301", "ag", "PAUSED", "Same body"),
        ];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        let noops = actions.iter().filter(|a| matches!(a, Action::NoOp { .. })).count();
        let updates = actions
            .iter()
            .filter(|a| matches!(a, Action::Update { .. }))
            .count();
        assert_eq!((noops, updates), (1, 1));
        let mut ids: Vec<&str> = actions.iter().filter_map(|a| a.live_id()).collect();
        ids.sort();
        assert_eq!(ids, vec!["300", "301"], "no live id reused");
    }

    #[test]
    fn distinct_bodies_do_not_cross_match() {
        // A unique-bodied declared ad with no live counterpart is a create, not a
        // false match against an unrelated live ad in the same group.
        let declared = vec![ad("new", "ag", "ENABLED", "Brand new copy")];
        let live = vec![ad("400", "ag", "ENABLED", "Old copy")];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        assert!(matches!(&actions[0], Action::Create));
    }

    #[test]
    fn unmapped_ad_group_is_create() {
        let declared = vec![ad("x", "missing_ag", "ENABLED", "Copy")];
        let live = vec![ad("500", "ag", "ENABLED", "Copy")];

        let actions: Vec<Action> =
            match_ad_group_ads(&declared, &live, &identity_match(), &HashMap::new())
                .into_iter()
                .map(|(a, _)| a)
                .collect();

        assert!(matches!(&actions[0], Action::Create));
    }
}

#[cfg(test)]
mod criterion_match_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    #[test]
    fn positive_and_negative_same_keyword_do_not_collide() {
        // A live ad group can hold the same keyword as both a positive and a
        // negative criterion. Declaring only the positive must match the live
        // positive and leave the negative untouched — never delete the positive.
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"b"}],
            "ad_groups": [{"id":"g","name":"G","campaign":"c"}],
            "ad_group_criteria": [
                {"id":"k","ad_group":"g","negative":false,"keyword":{"text":"shoes","match_type":"BROAD"}}
            ]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "ad_groups": [{"id":"300","name":"G","campaign":"100"}],
            "ad_group_criteria": [
                {"id":"400","ad_group":"300","negative":false,"keyword":{"text":"shoes","match_type":"BROAD"}},
                {"id":"401","ad_group":"300","negative":true,"keyword":{"text":"shoes","match_type":"BROAD"}}
            ]
        }"#,
        );

        let report = diff(&declared, &live);

        assert_eq!(
            report.delete_count, 0,
            "no criterion should be deleted: {:?}",
            report
                .diffs
                .iter()
                .map(|d| (&d.address, &d.action))
                .collect::<Vec<_>>()
        );
        let crit = report
            .diffs
            .iter()
            .find(|d| d.kind == "ad_group_criterion" && d.address == "k")
            .expect("declared criterion present");
        assert!(
            matches!(&crit.action, Action::NoOp { live_id } if live_id == "400"),
            "declared positive keyword should match the live positive one, got {:?}",
            crit.action
        );
    }

    fn device_criteria(declared_extra: &str, live_extra: &str) -> DiffReport {
        let declared = input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"b"}}],
            "campaign_criteria": [{declared_extra}]
        }}"#,
        ));
        let live = input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}}],
            "campaign_criteria": [{live_extra}]
        }}"#,
        ));
        diff(&declared, &live)
    }

    fn crit_action(report: &DiffReport, address: &str) -> Action {
        report
            .diffs
            .iter()
            .find(|d| d.kind == "campaign_criterion" && d.address == address)
            .unwrap_or_else(|| panic!("criterion {address} present"))
            .action
            .clone()
    }

    #[test]
    fn device_criterion_matches_by_type_and_diffs_bid_modifier() {
        // Same device type, same modifier → no-op; changed modifier → in-place
        // update (device type is the match key, bid_modifier a scalar field).
        let same = device_criteria(
            r#"{"id":"d","campaign":"c","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
            r#"{"id":"500","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
        );
        assert!(
            matches!(crit_action(&same, "d"), Action::NoOp { live_id } if live_id == "500"),
            "identical device modifier should be a no-op, got {:?}",
            crit_action(&same, "d")
        );

        let changed = device_criteria(
            r#"{"id":"d","campaign":"c","bid_modifier":0.7,"device":{"type":"MOBILE"}}"#,
            r#"{"id":"500","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
        );
        assert!(
            matches!(
                crit_action(&changed, "d"),
                Action::Update { ref changed_fields, .. } if field_names(changed_fields) == ["bid_modifier"]
            ),
            "changed device modifier should update bid_modifier, got {:?}",
            crit_action(&changed, "d")
        );
    }

    #[test]
    fn device_criterion_of_new_type_is_created_not_matched() {
        // A declared DESKTOP modifier must not match a live MOBILE one.
        let report = device_criteria(
            r#"{"id":"d","campaign":"c","bid_modifier":1.2,"device":{"type":"DESKTOP"}}"#,
            r#"{"id":"500","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
        );
        assert!(
            matches!(crit_action(&report, "d"), Action::Create),
            "a different device type should create, got {:?}",
            crit_action(&report, "d")
        );
    }

    #[test]
    fn auto_materialized_device_criteria_are_not_destroyed() {
        // Issue #82: a desktop-only campaign declares MOBILE/TABLET at 0 and no
        // DESKTOP; Google auto-materializes DESKTOP (id 30000) in default state.
        // It must read as implicitly desired — no doomed destroy, no warning.
        let report = device_criteria(
            r#"{"id":"m","campaign":"c","bid_modifier":0.0,"device":{"type":"MOBILE"}},
               {"id":"t","campaign":"c","bid_modifier":0.0,"device":{"type":"TABLET"}}"#,
            r#"{"id":"30000","campaign":"100","device":{"type":"DESKTOP"}},
               {"id":"30001","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}},
               {"id":"30002","campaign":"100","bid_modifier":0.0,"device":{"type":"TABLET"}},
               {"id":"30004","campaign":"100","bid_modifier":1.0,"device":{"type":"CONNECTED_TV"}}"#,
        );
        assert_eq!(
            report.delete_count, 0,
            "device criteria must never be destroyed: {:?}",
            report
                .diffs
                .iter()
                .map(|d| (&d.address, &d.action))
                .collect::<Vec<_>>()
        );
        assert!(report.warnings.is_empty(), "default-state device criteria are not drift: {:?}", report.warnings);
        assert!(matches!(crit_action(&report, "m"), Action::NoOp { .. }));
        assert!(matches!(crit_action(&report, "t"), Action::NoOp { .. }));
    }

    #[test]
    fn undeclared_device_adjustment_warns_instead_of_destroying() {
        let report = device_criteria(
            r#"{"id":"m","campaign":"c","bid_modifier":0.0,"device":{"type":"MOBILE"}}"#,
            r#"{"id":"30001","campaign":"100","bid_modifier":0.0,"device":{"type":"MOBILE"}},
               {"id":"30002","campaign":"100","bid_modifier":0.7,"device":{"type":"TABLET"}}"#,
        );
        assert_eq!(report.delete_count, 0, "even a drifted device criterion must not be destroyed");
        assert_eq!(report.warnings.len(), 1, "drift should surface as a warning: {:?}", report.warnings);
        assert!(
            report.warnings[0].contains("device TABLET") && report.warnings[0].contains("0.7"),
            "warning should name the criterion and modifier: {}",
            report.warnings[0]
        );
    }
}

#[cfg(test)]
mod label_match_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    const DECLARED_SUMMER: &str = r#"{
        "customer_id": "100",
        "campaign_budgets": [{"id":"m.b","name":"B","amount_micros":1000}],
        "campaigns": [{"id":"m.google_ads_campaign.summer","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
    }"#;

    fn campaign_diff(report: &DiffReport) -> &ResourceDiff {
        report
            .diffs
            .iter()
            .find(|d| d.kind == "campaign")
            .expect("a campaign diff")
    }

    #[test]
    fn adoption_labels_an_unlabeled_match() {
        // Live campaign matches by name but carries no bidsmith label: adopt it
        // (no field change) and schedule a fresh label write.
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"999"}]
        }"#,
        );
        let report = diff(&input(DECLARED_SUMMER), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { live_id } if live_id == "555"));
        assert_eq!(report.adopt_count, 1);
        let plan = report
            .label_plans
            .iter()
            .find(|p| p.kind == "campaign")
            .expect("a label plan");
        assert_eq!(plan.label_address, "m.google_ads_campaign.summer");
        assert!(plan.existing_label_rn.is_none(), "fresh label is created");
        assert!(plan.stale_assoc_rn.is_none());
    }

    #[test]
    fn adopting_a_campaign_on_another_channel_warns_instead_of_reading_as_a_match() {
        // The channel is creation-only, so the diff skips it — without the
        // warning this adoption reports as a clean match while the file
        // describes a campaign it is not pointing at (issue #112).
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"VIDEO","campaign_budget":"999"}]
        }"#,
        );
        let report = diff(&input(DECLARED_SUMMER), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { .. }));
        let warning = report
            .warnings
            .iter()
            .find(|w| w.contains("advertising_channel_type"))
            .expect("a channel mismatch warning");
        assert!(warning.contains("\"SEARCH\"") && warning.contains("\"VIDEO\""), "{warning}");
    }

    /// The sub-type is creation-only too, and it is what separates two video
    /// campaigns of different formats — so a mismatch has to be said out loud
    /// rather than reported as a clean adoption (issue #133).
    #[test]
    fn adopting_a_campaign_of_another_sub_type_warns() {
        let declared = DECLARED_SUMMER.replace(
            r#""advertising_channel_type":"SEARCH""#,
            r#""advertising_channel_type":"SEARCH","advertising_channel_sub_type":"VIDEO_NON_SKIPPABLE""#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"SEARCH","advertising_channel_sub_type":"VIDEO_REACH_TARGET_FREQUENCY","campaign_budget":"999"}]
        }"#,
        );
        let report = diff(&input(&declared), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { .. }));
        let warning = report
            .warnings
            .iter()
            .find(|w| w.contains("advertising_channel_sub_type"))
            .expect("a sub-type mismatch warning");
        assert!(
            warning.contains("\"VIDEO_NON_SKIPPABLE\"")
                && warning.contains("\"VIDEO_REACH_TARGET_FREQUENCY\""),
            "{warning}"
        );
    }

    /// A file that says nothing about the sub-type is not asserting the
    /// campaign has none, so adopting one that carries a sub-type is quiet.
    #[test]
    fn an_undeclared_sub_type_is_not_a_mismatch() {
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"SEARCH","advertising_channel_sub_type":"VIDEO_NON_SKIPPABLE","campaign_budget":"999"}]
        }"#,
        );
        let report = diff(&input(DECLARED_SUMMER), &live);

        assert!(
            !report.warnings.iter().any(|w| w.contains("advertising_channel_sub_type")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn correct_label_is_a_clean_noop() {
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"m.google_ads_campaign.summer"}],
            "labels": {"m.google_ads_campaign.summer":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&input(DECLARED_SUMMER), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { .. }));
        assert_eq!(report.adopt_count, 0);
        assert!(report.label_plans.is_empty(), "label already correct");
    }

    #[test]
    fn renaming_the_name_on_a_labeled_campaign_is_an_update() {
        // The live campaign is identified by its label, so changing its display
        // name is an in-place update — not a create + orphan.
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Old Name","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"m.google_ads_campaign.summer"}],
            "labels": {"m.google_ads_campaign.summer":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&input(DECLARED_SUMMER), &live);

        assert!(
            matches!(&campaign_diff(&report).action, Action::Update { changed_fields, .. } if changed_fields.iter().any(|f| f.field == "name")),
            "expected a name update, got {:?}",
            campaign_diff(&report).action
        );
        assert_eq!(report.create_count, 0);
        assert!(report.label_plans.is_empty(), "label already correct");
    }

    #[test]
    fn labeled_orphan_is_destroyed_unlabeled_is_left_alone() {
        // Two undeclared live campaigns: the bidsmith-labeled one is destroyed,
        // the UI-created (unlabeled) one is untouched.
        let declared = input(r#"{"customer_id":"100","campaign_budgets":[{"id":"m.b","name":"B","amount_micros":1000}]}"#);
        let live = input(
            r#"{
            "customer_id": "100",
            "campaigns": [
                {"id":"555","name":"Managed","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"m.google_ads_campaign.gone"},
                {"id":"556","name":"Hand Made","advertising_channel_type":"SEARCH","campaign_budget":"999"}
            ],
            "labels": {"m.google_ads_campaign.gone":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&declared, &live);

        let destroys: Vec<&ResourceDiff> = report
            .diffs
            .iter()
            .filter(|d| d.kind == "campaign" && matches!(d.action, Action::Delete { .. }))
            .collect();
        assert_eq!(report.delete_count, 1);
        assert_eq!(destroys.len(), 1);
        assert!(matches!(&destroys[0].action, Action::Delete { live_id } if live_id == "555"));
    }

    /// Applying one file of a per-campaign tree destroyed every other file's
    /// campaign: "not declared in this file" was read as "not declared at all"
    /// (issue #160). A partial run's removals stay inside the modules it read.
    #[test]
    fn a_partial_run_does_not_destroy_another_modules_campaign() {
        let mut declared = input(
            r#"{"customer_id":"100","campaign_budgets":[{"id":"campaign_b.b","name":"B","amount_micros":1000}]}"#,
        );
        declared.partial_modules = Some(["campaign_b".to_string()].into_iter().collect());
        let live = input(
            r#"{
            "customer_id": "100",
            "campaigns": [
                {"id":"555","name":"A","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"campaign_a.google_ads_campaign.a"},
                {"id":"556","name":"B gone","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"campaign_b.google_ads_campaign.old"}
            ],
            "labels": {
                "campaign_a.google_ads_campaign.a":"customers/100/labels/777",
                "campaign_b.google_ads_campaign.old":"customers/100/labels/778"
            }
        }"#,
        );
        let report = diff(&declared, &live);

        // The campaign in the module this run *did* read is still reconciled —
        // scoping is not a mute button on removal.
        assert_eq!(report.delete_count, 1, "{:?}", report.diffs);
        let destroy = report
            .diffs
            .iter()
            .find(|d| matches!(d.action, Action::Delete { .. }))
            .unwrap();
        assert!(matches!(&destroy.action, Action::Delete { live_id } if live_id == "556"));
        assert_eq!(report.skipped_removal_count, 1);
        assert!(
            report.warnings.iter().any(|w| w.contains("campaign_a")),
            "the skip names the module whose file to add: {:?}",
            report.warnings
        );
    }

    /// A run that read the whole project still prunes what a deleted file used
    /// to declare — otherwise removing a campaign would have no gesture at all.
    #[test]
    fn a_whole_project_run_still_destroys_a_deleted_files_campaign() {
        let declared = input(
            r#"{"customer_id":"100","campaign_budgets":[{"id":"campaign_b.b","name":"B","amount_micros":1000}]}"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "campaigns": [{"id":"555","name":"A","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"campaign_a.google_ads_campaign.a"}],
            "labels": {"campaign_a.google_ads_campaign.a":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(report.delete_count, 1, "{:?}", report.diffs);
        assert_eq!(report.skipped_removal_count, 0);
    }

    #[test]
    fn address_rename_relabels_via_content_fallback() {
        // `bidsmith mv` changed the address in source (summer -> autumn) but the
        // live campaign still carries the old label and the same name. Content
        // fallback re-adopts it under the new address and reconciles the label —
        // no destroy, no recreate.
        let declared = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"m.b","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"m.google_ads_campaign.autumn","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Summer","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"m.google_ads_campaign.summer"}],
            "labels": {"m.google_ads_campaign.summer":"customers/100/labels/777"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { live_id } if live_id == "555"));
        assert_eq!(report.delete_count, 0, "mv must never destroy");
        let plan = report
            .label_plans
            .iter()
            .find(|p| p.kind == "campaign")
            .expect("a relabel plan");
        assert_eq!(plan.label_address, "m.google_ads_campaign.autumn");
        assert_eq!(
            plan.stale_assoc_rn.as_deref(),
            Some("customers/100/campaignLabels/555~777"),
            "the old label association is removed"
        );
    }

    const LONG_ADDR: &str =
        "instream.google_ads_campaign.instream_preroll_a_rather_long_campaign_family_identifier_2026";

    fn long_declared() -> String {
        format!(
            r#"{{
            "customer_id": "100",
            "campaign_budgets": [{{"id":"m.b","name":"B","amount_micros":1000}}],
            "campaigns": [{{"id":"{LONG_ADDR}","name":"Preroll","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}}]
        }}"#
        )
    }

    #[test]
    fn adopting_a_long_address_writes_a_label_that_fits_eighty_chars() {
        // The reported bug: an unlabeled live campaign whose address overflows the
        // 80-char label cap. Adoption must encode the label so the API accepts it.
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"999","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"555","name":"Preroll","advertising_channel_type":"SEARCH","campaign_budget":"999"}]
        }"#,
        );
        let report = diff(&input(&long_declared()), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { live_id } if live_id == "555"));
        assert_eq!(report.adopt_count, 1);
        let plan = report
            .label_plans
            .iter()
            .find(|p| p.kind == "campaign")
            .expect("a label plan");
        let label_name = format!(
            "{}{}",
            crate::commands::export::ADDRESS_LABEL_PREFIX,
            plan.label_address
        );
        assert!(
            label_name.len() <= crate::commands::export::MAX_LABEL_NAME_LEN,
            "label {label_name:?} is {} chars",
            label_name.len()
        );
        assert_ne!(plan.label_address, LONG_ADDR, "the long address is encoded");
    }

    #[test]
    fn a_long_address_already_labeled_is_a_clean_noop() {
        // Second run after adoption: the live campaign carries the encoded payload.
        // It must read as already-labeled, not re-adopt forever.
        let payload = address_label_payload(LONG_ADDR);
        let live = input(&format!(
            r#"{{
            "customer_id": "100",
            "campaign_budgets": [{{"id":"999","name":"B","amount_micros":1000}}],
            "campaigns": [{{"id":"555","name":"Preroll","advertising_channel_type":"SEARCH","campaign_budget":"999","managed_address":"{payload}"}}],
            "labels": {{"{payload}":"customers/100/labels/777"}}
        }}"#
        ));
        let report = diff(&input(&long_declared()), &live);

        assert!(matches!(&campaign_diff(&report).action, Action::NoOp { .. }));
        assert_eq!(report.adopt_count, 0, "must not re-adopt an already-labeled resource");
        assert!(report.label_plans.is_empty(), "label already correct");
    }
}

#[cfg(test)]
mod removed_resource_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    fn ad_destroys(report: &DiffReport) -> Vec<&str> {
        report
            .diffs
            .iter()
            .filter(|d| d.kind == "ad_group_ad" && matches!(d.action, Action::Delete { .. }))
            .filter_map(|d| d.action.live_id())
            .collect()
    }

    #[test]
    fn a_video_ad_matching_live_plans_as_a_no_op() {
        // Second plan after `apply` created the asset and the creative: the
        // declared video ref is an address, live reports an asset id, and the
        // two have to reconcile or every plan would re-create the ad.
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "youtube_video_assets": [{"id":"m.brand","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"m.brand","headlines":["Block ads"],"call_to_actions":["Install"]
                }}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"c1","managed_address":"m.ag"}],
            "youtube_video_assets": [{"id":"42","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"55~9","ad_group":"55","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"42","headlines":["Block ads"],"call_to_actions":["Install"]
                }},
                "managed_address":"m.preroll"
            }],
            "labels": {"m.ag":"customers/100/labels/1","m.preroll":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        let ad = report
            .diffs
            .iter()
            .find(|d| d.address == "m.preroll")
            .expect("the video ad is diffed");
        assert!(
            matches!(ad.action, Action::NoOp { .. }),
            "an unchanged video ad must not re-create: {:?}",
            ad.action
        );
        let asset = report
            .diffs
            .iter()
            .find(|d| d.kind == "youtube_video_asset")
            .expect("the video asset is diffed");
        assert_eq!(asset.action.live_id(), Some("42"));
        assert!(ad_destroys(&report).is_empty());
    }

    #[test]
    fn a_scaffold_only_ad_still_adopts_a_ui_built_creative() {
        // The shape `refresh` produced for a UI-built video ad before the
        // creative was readable: URLs, no creative block. It has to keep
        // adopting the live ad rather than destroying it and creating an
        // ad with no creative at all.
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"]}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"c1","managed_address":"m.ag"}],
            "ad_group_ads": [{
                "id":"55~9","ad_group":"55","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"42","headlines":["Built in the UI"]
                }},
                "managed_address":"m.preroll"
            }],
            "labels": {"m.ag":"customers/100/labels/1","m.preroll":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        let ad = report
            .diffs
            .iter()
            .find(|d| d.address == "m.preroll")
            .expect("the ad is diffed");
        assert_eq!(ad.action.live_id(), Some("55~9"), "{:?}", ad.action);
        assert!(ad_destroys(&report).is_empty());
    }

    #[test]
    fn an_unlabeled_ui_built_creative_adopts_onto_a_scaffold_only_ad() {
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"]}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"c1","managed_address":"m.ag"}],
            "ad_group_ads": [{
                "id":"55~9","ad_group":"55","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"42","headlines":["Built in the UI"]
                }}
            }],
            "labels": {"m.ag":"customers/100/labels/1"}
        }"#,
        );
        let report = diff(&declared, &live);

        let ad = report
            .diffs
            .iter()
            .find(|d| d.address == "m.preroll")
            .expect("the ad is diffed");
        assert_eq!(ad.action.live_id(), Some("55~9"), "{:?}", ad.action);
    }

    #[test]
    fn a_changed_video_creative_replaces_the_ad() {
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "youtube_video_assets": [{"id":"m.brand","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"m.brand","headlines":["Block trackers too"]
                }}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"c1","managed_address":"m.ag"}],
            "youtube_video_assets": [{"id":"42","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"55~9","ad_group":"55","status":"PAUSED",
                "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                    "video":"42","headlines":["Block ads"]
                }},
                "managed_address":"m.preroll"
            }],
            "labels": {"m.ag":"customers/100/labels/1","m.preroll":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        let ad = report
            .diffs
            .iter()
            .find(|d| d.address == "m.preroll")
            .expect("the video ad is diffed");
        assert!(
            matches!(ad.action, Action::Create),
            "an edited creative is a new ad: {:?}",
            ad.action
        );
        assert_eq!(ad_destroys(&report), vec!["55~9"]);
    }

    #[test]
    fn ads_orphaned_under_a_removed_ad_group_are_not_re_destroyed() {
        // Second plan after a whole ad group was removed: the group is gone from
        // live state but its ads survive ENABLED, still carrying their labels.
        // The orphan must be skipped (its destroy would reject with "Removed ads
        // may not be modified" and sink the batch); a normal undeclared ad under
        // a still-live group is still destroyed.
        let declared = input(r#"{"customer_id":"100"}"#);
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [
                {"id":"live_ag","name":"Live","campaign":"c1"}
            ],
            "ad_group_ads": [
                {"id":"live_ag~ad1","ad_group":"live_ag","status":"ENABLED","ad":{"final_urls":["https://e.com"]},"managed_address":"m.google_ads_ad_group_ad.under_live"},
                {"id":"gone_ag~ad2","ad_group":"gone_ag","status":"ENABLED","ad":{"final_urls":["https://e.com"]},"managed_address":"m.google_ads_ad_group_ad.orphan"}
            ],
            "labels": {
                "m.google_ads_ad_group_ad.under_live":"customers/100/labels/2",
                "m.google_ads_ad_group_ad.orphan":"customers/100/labels/3"
            }
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(
            ad_destroys(&report),
            vec!["live_ag~ad1"],
            "orphan under a removed ad group must be skipped; ad under a live group still destroyed"
        );
    }

    #[test]
    fn a_removed_status_ad_is_not_re_destroyed() {
        // Defense in depth: a REMOVED ad leaking into live state (its label
        // survives removal) must not re-plan as a destroy, even under a live group.
        let declared = input(r#"{"customer_id":"100"}"#);
        let live = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [
                {"id":"live_ag","name":"Live","campaign":"c1"}
            ],
            "ad_group_ads": [
                {"id":"live_ag~ad1","ad_group":"live_ag","status":"REMOVED","ad":{"final_urls":["https://e.com"]},"managed_address":"m.google_ads_ad_group_ad.dead"}
            ],
            "labels": {"m.google_ads_ad_group_ad.dead":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert!(
            ad_destroys(&report).is_empty(),
            "a REMOVED ad must not be re-flagged for destroy"
        );
    }
}

#[cfg(test)]
mod video_creative_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    fn ad_action(report: &DiffReport) -> &Action {
        &report.diffs.iter().find(|d| d.kind == "ad_group_ad").expect("an ad row").action
    }

    /// A live in-stream ad as `pull` renders it: a `video_ad` creative, the UTM
    /// slug on `final_urls`, and the display URL beside it.
    fn live_instream(managed_address: &str) -> ExportInput {
        input(&format!(
            r#"{{
            "customer_id": "100",
            "campaigns": [{{"id":"5","name":"Preroll","advertising_channel_type":"VIDEO","campaign_budget":"9"}}],
            "ad_groups": [{{"id":"55","name":"In-stream","campaign":"5","managed_address":"m.ag"}}],
            "youtube_video_assets": [{{"id":"42","youtube_video_id":"dQw4w9WgXcQ"}}],
            "ad_group_ads": [{{
                "id":"55~9","ad_group":"55","status":"ENABLED",
                "ad":{{
                    "final_urls":["https://ghostery.com/?utm_campaign=GH_YouTubeUS_v1"],
                    "display_url":"www.ghostery.com",
                    "video_ad":{{"video":"42"}}
                }},
                "managed_address":"{managed_address}"
            }}],
            "labels": {{"m.ag":"customers/100/labels/1","{managed_address}":"customers/100/labels/2"}}
        }}"#
        ))
    }

    #[test]
    fn a_declared_video_ad_adopts_the_live_creative_it_describes() {
        // The whole point of modelling these fields: the UTM slug the test is
        // measured on becomes a reviewable line in the repo, and the ad it
        // belongs to still plans as a no-op.
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "youtube_video_assets": [{"id":"m.brand","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"ENABLED",
                "ad":{
                    "final_urls":["https://ghostery.com/?utm_campaign=GH_YouTubeUS_v1"],
                    "display_url":"www.ghostery.com",
                    "video_ad":{"video":"m.brand"}
                }
            }]
        }"#,
        );
        let report = diff(&declared, &live_instream("m.preroll"));

        assert!(matches!(ad_action(&report), Action::NoOp { .. }), "{:?}", ad_action(&report));
    }

    #[test]
    fn a_creative_less_ad_still_adopts_one_carrying_a_display_url() {
        // The pre-#136 shape: a file that names only the URLs. Modelling
        // `display_url` must not turn every already-adopted UI-built ad into a
        // create of an ad the VIDEO channel would refuse anyway.
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"ENABLED",
                "ad":{"final_urls":["https://ghostery.com/?utm_campaign=GH_YouTubeUS_v1"]}
            }]
        }"#,
        );
        let report = diff(&declared, &live_instream("m.preroll"));

        assert!(matches!(ad_action(&report), Action::NoOp { .. }), "{:?}", ad_action(&report));
    }

    #[test]
    fn a_declared_display_url_that_disagrees_is_not_the_same_ad() {
        let declared = input(
            r#"{
            "customer_id": "100",
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"ENABLED",
                "ad":{
                    "final_urls":["https://ghostery.com/?utm_campaign=GH_YouTubeUS_v1"],
                    "display_url":"ghostery.example"
                }
            }]
        }"#,
        );
        let report = diff(&declared, &live_instream("m.preroll"));

        assert!(matches!(ad_action(&report), Action::Create), "{:?}", ad_action(&report));
    }

    #[test]
    fn two_ads_differing_only_in_the_tracking_url_are_two_bodies() {
        let a = input(
            r#"{"customer_id":"1","ad_group_ads":[{"id":"a","ad_group":"g",
            "ad":{"final_urls":["https://e.com"],"display_url":"e.com",
                  "final_mobile_urls":["https://m.e.com"],"video_ad":{"video":"v"}}}]}"#,
        );
        let b = input(
            r#"{"customer_id":"1","ad_group_ads":[{"id":"b","ad_group":"g",
            "ad":{"final_urls":["https://e.com"],"display_url":"e.com",
                  "final_mobile_urls":["https://m2.e.com"],"video_ad":{"video":"v"}}}]}"#,
        );
        let empty = HashMap::new();
        assert_ne!(
            ad_body_key(&a.ad_group_ads[0], &empty),
            ad_body_key(&b.ad_group_ads[0], &empty),
        );
    }

    #[test]
    fn video_responsive_breadcrumbs_are_part_of_the_body() {
        let with = input(
            r#"{"customer_id":"1","ad_group_ads":[{"id":"a","ad_group":"g",
            "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{
                "video":"v","breadcrumb1":"AdBlocker","breadcrumb2":"Browser"}}}]}"#,
        );
        let without = input(
            r#"{"customer_id":"1","ad_group_ads":[{"id":"a","ad_group":"g",
            "ad":{"final_urls":["https://e.com"],"video_responsive_ad":{"video":"v"}}}]}"#,
        );
        let empty = HashMap::new();
        assert_ne!(
            ad_body_key(&with.ad_group_ads[0], &empty),
            ad_body_key(&without.ad_group_ads[0], &empty),
        );
    }

    #[test]
    fn a_new_ad_on_a_video_campaign_blocks_the_batch() {
        // The restriction is the channel, and an ad carries none of its own —
        // so without the parent check this create reaches Google and takes
        // every unrelated operation in the atomic batch down with it.
        let declared = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"m.b","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"m.c","name":"Preroll","advertising_channel_type":"VIDEO","campaign_budget":"m.b"}],
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "youtube_video_assets": [{"id":"m.brand","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"ENABLED",
                "ad":{"final_urls":["https://ghostery.com/get"],
                      "video_responsive_ad":{"video":"m.brand","headlines":["Block ads"]}}
            }]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"9","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"5","name":"Preroll","advertising_channel_type":"VIDEO","campaign_budget":"9","managed_address":"m.c"}],
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"5","managed_address":"m.ag"}],
            "labels": {"m.c":"customers/100/labels/1","m.ag":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert!(
            report.blockers.iter().any(|b| b.contains("m.preroll")),
            "the ad itself has to be named: {:?}",
            report.blockers,
        );
    }

    #[test]
    fn an_adopt_only_ad_that_matched_nothing_gets_the_sharper_blocker() {
        // Two true statements, one useful one. "You meant to adopt and found
        // nothing" tells the reader what to fix; "the channel is read-only"
        // just restates why it matters.
        let declared = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"m.b","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"m.c","name":"Preroll","advertising_channel_type":"VIDEO","campaign_budget":"m.b"}],
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "youtube_video_assets": [{"id":"m.brand","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"ENABLED",
                "ad":{"final_urls":["https://ghostery.com/get"],"video_ad":{"video":"m.brand"}}
            }],
            "adopt_only": ["m.preroll"]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"9","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"5","name":"Preroll","advertising_channel_type":"VIDEO","campaign_budget":"9","managed_address":"m.c"}],
            "ad_groups": [{"id":"55","name":"In-stream","campaign":"5","managed_address":"m.ag"}],
            "labels": {"m.c":"customers/100/labels/1","m.ag":"customers/100/labels/2"}
        }"#,
        );
        let report = diff(&declared, &live);

        let about_the_ad: Vec<&String> =
            report.blockers.iter().filter(|b| b.contains("m.preroll")).collect();
        assert_eq!(about_the_ad.len(), 1, "{about_the_ad:?}");
        assert!(about_the_ad[0].contains("nothing to adopt"), "{about_the_ad:?}");
    }

    #[test]
    fn an_adopted_video_ad_draws_no_blocker() {
        let declared = input(
            r#"{
            "customer_id": "100",
            "campaign_budgets": [{"id":"m.b","name":"B","amount_micros":1000}],
            "campaigns": [{"id":"m.c","name":"Preroll","advertising_channel_type":"VIDEO","campaign_budget":"m.b"}],
            "ad_groups": [{"id":"m.ag","name":"In-stream","campaign":"m.c"}],
            "youtube_video_assets": [{"id":"m.brand","youtube_video_id":"dQw4w9WgXcQ"}],
            "ad_group_ads": [{
                "id":"m.preroll","ad_group":"m.ag","status":"ENABLED",
                "ad":{
                    "final_urls":["https://ghostery.com/?utm_campaign=GH_YouTubeUS_v1"],
                    "display_url":"www.ghostery.com",
                    "video_ad":{"video":"m.brand"}
                }
            }]
        }"#,
        );
        let mut live = live_instream("m.preroll");
        live.campaign_budgets =
            input(r#"{"customer_id":"100","campaign_budgets":[{"id":"9","name":"B","amount_micros":1000}]}"#)
                .campaign_budgets;
        live.campaigns[0].managed_address = Some("m.c".to_string());
        live.labels.insert("m.c".to_string(), "customers/100/labels/3".to_string());
        let report = diff(&declared, &live);

        assert!(
            report.blockers.is_empty(),
            "adoption is the supported path on this channel: {:?}",
            report.blockers,
        );
    }
}

#[cfg(test)]
mod claim_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    // A campaign + ad group declaring only positive keywords — the state after
    // removing the last negative-keyword resource (issue #88).
    const DECLARED_POSITIVES_ONLY: &str = r#"{
        "customer_id": "1",
        "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}],
        "ad_groups": [{"id":"m.g","name":"G","campaign":"m.c"}],
        "ad_group_criteria": [
            {"id":"m.k","ad_group":"m.g","negative":false,"keyword":{"text":"shoes","match_type":"EXACT"}}
        ]
    }"#;

    fn live_with_orphaned_negatives(claims_and_labels: &str) -> ExportInput {
        input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}}],
            "ad_groups": [{{"id":"300","name":"G","campaign":"100"}}],
            "ad_group_criteria": [
                {{"id":"400","ad_group":"300","negative":false,"keyword":{{"text":"shoes","match_type":"EXACT"}}}},
                {{"id":"401","ad_group":"300","negative":true,"keyword":{{"text":"vpn","match_type":"EXACT"}}}},
                {{"id":"402","ad_group":"300","negative":true,"keyword":{{"text":"proxy","match_type":"PHRASE"}}}}
            ]{claims_and_labels}
        }}"#
        ))
    }

    #[test]
    fn live_claim_keeps_destroying_after_last_declared_member_is_removed() {
        // Issue #88: the negatives resource is gone from desired state, but the
        // ad group's live `bidsmith:owns=keyword_negative` claim proves bidsmith
        // managed that category — the orphans must plan as destroys and the
        // claim must be released in the same batch.
        let live = live_with_orphaned_negatives(
            r#",
            "ad_group_claims": {"300": ["keyword_negative", "keyword_positive"]},
            "claim_labels": {
                "keyword_negative": "customers/1/labels/777",
                "keyword_positive": "customers/1/labels/778"
            }"#,
        );
        let report = diff(&input(DECLARED_POSITIVES_ONLY), &live);

        let destroyed: Vec<&ResourceDiff> = report
            .diffs
            .iter()
            .filter(|d| matches!(d.action, Action::Delete { .. }))
            .collect();
        assert_eq!(
            destroyed.len(),
            2,
            "both orphaned negatives should be destroyed: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        assert!(destroyed
            .iter()
            .all(|d| d.kind == "ad_group_criterion" && d.address.contains("m.g (removed negative_keyword")));

        let releases: Vec<&ClaimPlanEntry> = report
            .claim_plans
            .iter()
            .filter(|p| p.stale_assoc_rn.is_some())
            .collect();
        assert_eq!(releases.len(), 1, "claims: {:?}", report.claim_plans);
        assert_eq!(releases[0].category, "keyword_negative");
        assert_eq!(
            releases[0].stale_assoc_rn.as_deref(),
            Some("customers/1/adGroupLabels/300~777")
        );
        assert!(
            !report
                .claim_plans
                .iter()
                .any(|p| p.category == "keyword_positive"),
            "the still-declared positive claim already exists live: {:?}",
            report.claim_plans
        );
    }

    #[test]
    fn unclaimed_live_negatives_are_never_destroyed() {
        // Adoption safety: no declared negatives, no live claim — the negatives
        // could be UI-managed, so they stay. The declared positive category
        // plans a fresh claim instead.
        let live = live_with_orphaned_negatives("");
        let report = diff(&input(DECLARED_POSITIVES_ONLY), &live);

        assert_eq!(
            report.delete_count, 0,
            "unclaimed live negatives must be left alone: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        let adds: Vec<&ClaimPlanEntry> = report
            .claim_plans
            .iter()
            .filter(|p| p.stale_assoc_rn.is_none())
            .collect();
        assert_eq!(adds.len(), 1, "claims: {:?}", report.claim_plans);
        assert_eq!(adds[0].kind, "ad_group");
        assert_eq!(adds[0].address, "m.g");
        assert_eq!(adds[0].category, "keyword_positive");
        assert!(adds[0].existing_label_rn.is_none(), "no live owns label yet");
    }

    #[test]
    fn claim_add_reuses_an_existing_owns_label() {
        let live = live_with_orphaned_negatives(
            r#",
            "claim_labels": {"keyword_positive": "customers/1/labels/778"}"#,
        );
        let report = diff(&input(DECLARED_POSITIVES_ONLY), &live);

        let add = report
            .claim_plans
            .iter()
            .find(|p| p.stale_assoc_rn.is_none())
            .expect("a claim add");
        assert_eq!(
            add.existing_label_rn.as_deref(),
            Some("customers/1/labels/778")
        );
    }

    #[test]
    fn stale_claim_without_live_members_plans_only_a_release() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}],
            "ad_groups": [{"id":"m.g","name":"G","campaign":"m.c"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "ad_groups": [{"id":"300","name":"G","campaign":"100"}],
            "ad_group_claims": {"300": ["keyword_negative"]},
            "claim_labels": {"keyword_negative": "customers/1/labels/777"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(report.delete_count, 0);
        assert_eq!(report.claim_plans.len(), 1, "claims: {:?}", report.claim_plans);
        assert_eq!(
            report.claim_plans[0].stale_assoc_rn.as_deref(),
            Some("customers/1/adGroupLabels/300~777")
        );
        assert!(report.adopt_count > 0, "a claim release is pending work");
    }

    #[test]
    fn campaign_level_claim_gates_destroys_and_releases() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "campaign_criteria": [
                {"id":"500","campaign":"100","location":{"geo_target_constant":"geoTargetConstants/2616"}}
            ],
            "campaign_claims": {"100": ["location"]},
            "claim_labels": {"location": "customers/1/labels/779"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(
            report.delete_count, 1,
            "the claimed orphaned location should be destroyed: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        let del = report
            .diffs
            .iter()
            .find(|d| matches!(d.action, Action::Delete { .. }))
            .expect("a destroy");
        assert!(del.address.contains("m.c (removed location"));
        assert_eq!(report.claim_plans.len(), 1);
        assert_eq!(
            report.claim_plans[0].stale_assoc_rn.as_deref(),
            Some("customers/1/campaignLabels/100~779")
        );
    }

    #[test]
    fn an_ad_group_audience_matches_live_and_never_prunes_its_keywords() {
        // Issue #110: ad-group criteria partition by axis the way campaign ones
        // do — declaring a cohort adopts the live cohort and leaves the ad
        // group's keywords (a category the file says nothing about) alone.
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"VIDEO","campaign_budget":"m.b"}],
            "ad_groups": [{"id":"m.g","name":"G","campaign":"m.c"}],
            "ad_group_criteria": [
                {"id":"m.aud","ad_group":"m.g","audience":{"user_list":"customers/1/userLists/987"}}
            ]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"VIDEO","campaign_budget":"200"}],
            "ad_groups": [{"id":"300","name":"G","campaign":"100"}],
            "ad_group_criteria": [
                {"id":"400","ad_group":"300","keyword":{"text":"shoes","match_type":"EXACT"}},
                {"id":"401","ad_group":"300","audience":{"user_list":"customers/1/userLists/987"}}
            ]
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(
            report.delete_count, 0,
            "the live keyword is in a category this file never claims: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        let aud = report
            .diffs
            .iter()
            .find(|d| d.address == "m.aud")
            .expect("the audience criterion");
        assert!(
            matches!(&aud.action, Action::NoOp { live_id } if live_id == "401"),
            "the declared cohort should adopt the live one: {:?}",
            aud.action
        );
    }

    #[test]
    fn an_ad_group_claim_destroys_the_axis_it_owns() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"VIDEO","campaign_budget":"m.b"}],
            "ad_groups": [{"id":"m.g","name":"G","campaign":"m.c"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"VIDEO","campaign_budget":"200"}],
            "ad_groups": [{"id":"300","name":"G","campaign":"100"}],
            "ad_group_criteria": [
                {"id":"400","ad_group":"300","youtube_channel":{"channel_id":"UCabc"}}
            ],
            "ad_group_claims": {"300": ["youtube_channel"]},
            "claim_labels": {"youtube_channel": "customers/1/labels/781"}
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(report.delete_count, 1, "{:?}", report.diffs);
        let del = report
            .diffs
            .iter()
            .find(|d| matches!(d.action, Action::Delete { .. }))
            .expect("a destroy");
        assert!(del.address.contains("m.g (removed youtube_channel UCabc)"));
        assert_eq!(
            report.claim_plans[0].stale_assoc_rn.as_deref(),
            Some("customers/1/adGroupLabels/300~781")
        );
    }

    #[test]
    fn empty_declared_shared_set_leaves_live_members_alone() {
        // Sets match by bare name, so a declared set with no negative_keywords
        // blocks may be adopting a UI-curated list — never empty it.
        let declared = input(
            r#"{
            "customer_id": "1",
            "shared_sets": [{"id":"m.s","name":"S","type":"NEGATIVE_KEYWORDS"}]
        }"#,
        );
        let live = input(
            r#"{
            "customer_id": "1",
            "shared_sets": [{"id":"600","name":"S","type":"NEGATIVE_KEYWORDS"}],
            "shared_criteria": [
                {"id":"600~1","shared_set":"600","keyword":{"text":"vpn","match_type":"EXACT"}}
            ]
        }"#,
        );
        let report = diff(&declared, &live);

        assert_eq!(
            report.delete_count, 0,
            "live members of an adopted set must not be pruned: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
    }
}

#[cfg(test)]
mod field_change_tests {
    use super::*;

    fn campaign(extra: &str) -> JsonCampaign {
        serde_json::from_str(&format!(
            r#"{{"id":"c","name":"C","advertising_channel_type":"SEARCH",
                 "campaign_budget":"b"{extra}}}"#
        ))
        .expect("valid test campaign")
    }

    #[test]
    fn a_scalar_change_carries_both_sides() {
        let mut declared = campaign("");
        declared.name = "Winter Sale".to_string();
        let mut live = campaign("");
        live.name = "Winter".to_string();
        live.status = Some("PAUSED".to_string());
        declared.status = Some("ENABLED".to_string());

        let changed = diff_campaign(&declared, &live, false);
        assert_eq!(
            changed.iter().map(FieldChange::render).collect::<Vec<_>>(),
            vec![
                "name: \"Winter\" -> \"Winter Sale\"".to_string(),
                "status: \"PAUSED\" -> \"ENABLED\"".to_string(),
            ]
        );
    }

    #[test]
    fn an_absent_live_value_reads_as_unset_not_as_empty() {
        let declared = campaign(r#","end_date":"2026-12-31""#);
        let live = campaign("");
        let changed = diff_campaign(&declared, &live, false);
        assert_eq!(
            changed.iter().map(FieldChange::render).collect::<Vec<_>>(),
            vec!["end_date: (unset) -> \"2026-12-31\"".to_string()]
        );
    }

    #[test]
    fn frequency_caps_render_as_the_caps_themselves() {
        let declared = campaign(
            r#","frequency_caps":[{"event_type":"IMPRESSION","time_unit":"DAY","time_length":1,"cap":3}]"#,
        );
        let live = campaign("");
        let changed = diff_campaign(&declared, &live, false);
        assert_eq!(
            changed.iter().map(FieldChange::render).collect::<Vec<_>>(),
            vec!["frequency_caps: none -> 3 IMPRESSION / 1 DAY (CAMPAIGN)".to_string()]
        );
    }

    #[test]
    fn a_long_value_is_elided_rather_than_burying_the_row() {
        let mut declared = campaign("");
        declared.name = "x".repeat(200);
        let live = campaign("");
        let changed = diff_campaign(&declared, &live, false);
        assert_eq!(changed.len(), 1);
        assert!(changed[0].desired.ends_with('…'), "{:?}", changed[0]);
        assert!(changed[0].desired.chars().count() <= MAX_SHOWN_VALUE + 1);
    }
}

#[cfg(test)]
mod campaign_bidding_tests {
    use super::*;

    fn campaign(bidding: &str) -> JsonCampaign {
        serde_json::from_str(&format!(
            r#"{{"id":"c","name":"C","advertising_channel_type":"VIDEO",
                 "campaign_budget":"b"{bidding}}}"#
        ))
        .expect("valid test campaign")
    }

    #[test]
    fn switching_strategy_reports_the_desired_member() {
        let changed = diff_campaign(
            &campaign(r#","target_cpv":{}"#),
            &campaign(r#","manual_cpv":{}"#),
            false,
        );
        assert_eq!(field_names(&changed), vec!["target_cpv".to_string()]);
    }

    #[test]
    fn the_same_strategy_is_a_noop() {
        let changed = diff_campaign(
            &campaign(r#","target_cpm":{}"#),
            &campaign(r#","target_cpm":{}"#),
            false,
        );
        assert!(changed.is_empty(), "{changed:?}");
    }

    #[test]
    fn enhanced_cpc_still_diffs_within_manual_cpc() {
        let changed = diff_campaign(
            &campaign(r#","manual_cpc":{"enhanced_cpc_enabled":true}"#),
            &campaign(r#","manual_cpc":{"enhanced_cpc_enabled":false}"#),
            false,
        );
        assert_eq!(field_names(&changed), vec!["manual_cpc.enhanced_cpc_enabled".to_string()]);
    }

    #[test]
    fn a_file_that_declares_no_strategy_leaves_bidding_alone() {
        let changed = diff_campaign(
            &campaign(""),
            &campaign(r#","manual_cpc":{"enhanced_cpc_enabled":false}"#),
            false,
        );
        assert!(changed.is_empty(), "{changed:?}");
    }

    const TIS: &str = r#","target_impression_share":{"location":"ANYWHERE_ON_PAGE",
        "location_fraction_micros":800000,"cpc_bid_ceiling_micros":500000}"#;

    #[test]
    fn a_ui_tune_of_the_impression_share_target_is_drift_per_leaf() {
        let live = r#","target_impression_share":{"location":"TOP_OF_PAGE",
            "location_fraction_micros":800000,"cpc_bid_ceiling_micros":650000}"#;
        let changed = diff_campaign(&campaign(TIS), &campaign(live), false);
        assert_eq!(
            field_names(&changed),
            vec![
                "target_impression_share.location".to_string(),
                "target_impression_share.cpc_bid_ceiling_micros".to_string(),
            ]
        );
    }

    #[test]
    fn the_same_impression_share_target_is_a_noop() {
        let changed = diff_campaign(&campaign(TIS), &campaign(TIS), false);
        assert!(changed.is_empty(), "{changed:?}");
    }

    #[test]
    fn switching_onto_target_impression_share_reports_the_member() {
        let changed = diff_campaign(
            &campaign(TIS),
            &campaign(r#","manual_cpc":{"enhanced_cpc_enabled":false}"#),
            false,
        );
        assert_eq!(field_names(&changed), vec!["target_impression_share".to_string()]);
    }

    #[test]
    fn a_target_spend_ceiling_set_in_the_ui_is_drift() {
        let changed = diff_campaign(
            &campaign(r#","target_spend":{}"#),
            &campaign(r#","target_spend":{"cpc_bid_ceiling_micros":1100000}"#),
            false,
        );
        assert_eq!(
            field_names(&changed),
            vec!["target_spend.cpc_bid_ceiling_micros".to_string()]
        );
    }
}

#[cfg(test)]
mod geo_target_type_tests {
    use super::*;

    fn campaign(geo: &str) -> JsonCampaign {
        serde_json::from_str(&format!(
            r#"{{"id":"c","name":"C","advertising_channel_type":"SEARCH",
                 "campaign_budget":"b"{geo}}}"#
        ))
        .expect("valid test campaign")
    }

    const PRESENCE: &str = r#","geo_target_type_setting":{"positive_geo_target_type":"PRESENCE"}"#;
    const INTEREST: &str =
        r#","geo_target_type_setting":{"positive_geo_target_type":"PRESENCE_OR_INTEREST"}"#;

    #[test]
    fn a_ui_flip_to_presence_or_interest_is_drift() {
        let changed = diff_campaign(&campaign(PRESENCE), &campaign(INTEREST), false);
        assert_eq!(
            field_names(&changed),
            vec!["geo_target_type_setting.positive_geo_target_type".to_string()]
        );
    }

    #[test]
    fn the_declared_interpretation_is_a_noop() {
        let changed = diff_campaign(&campaign(PRESENCE), &campaign(PRESENCE), false);
        assert!(changed.is_empty(), "{changed:?}");
    }

    #[test]
    fn a_file_that_says_nothing_leaves_the_interpretation_alone() {
        let changed = diff_campaign(&campaign(""), &campaign(INTEREST), false);
        assert!(changed.is_empty(), "{changed:?}");
    }

    #[test]
    fn each_side_is_managed_on_its_own() {
        let changed = diff_campaign(
            &campaign(
                r#","geo_target_type_setting":{"negative_geo_target_type":"PRESENCE"}"#,
            ),
            &campaign(INTEREST),
            false,
        );
        assert_eq!(
            field_names(&changed),
            vec!["geo_target_type_setting.negative_geo_target_type".to_string()]
        );
    }
}

#[cfg(test)]
mod targeting_setting_tests {
    use super::*;

    fn campaign(setting: &str) -> JsonCampaign {
        serde_json::from_str(&format!(
            r#"{{"id":"c","name":"C","advertising_channel_type":"SEARCH",
                 "campaign_budget":"b"{setting}}}"#
        ))
        .expect("valid test campaign")
    }

    fn ad_group(setting: &str) -> JsonAdGroup {
        serde_json::from_str(&format!(
            r#"{{"id":"g","name":"G","campaign":"c"{setting}}}"#
        ))
        .expect("valid test ad group")
    }

    fn restrictions(entries: &str) -> String {
        format!(r#","targeting_setting":{{"target_restrictions":[{entries}]}}"#)
    }

    const AUDIENCE_OBSERVED: &str = r#"{"targeting_dimension":"AUDIENCE","bid_only":true}"#;
    const AUDIENCE_TARGETED: &str = r#"{"targeting_dimension":"AUDIENCE","bid_only":false}"#;
    const AGE_OBSERVED: &str = r#"{"targeting_dimension":"AGE_RANGE","bid_only":true}"#;

    #[test]
    fn observation_versus_targeting_reads_as_drift_in_both_words() {
        let changed = diff_campaign(
            &campaign(&restrictions(AUDIENCE_TARGETED)),
            &campaign(&restrictions(AUDIENCE_OBSERVED)),
            false,
        );
        assert_eq!(
            changed.iter().map(FieldChange::render).collect::<Vec<_>>(),
            vec![
                "targeting_setting.target_restrictions: AUDIENCE observation -> all targeting"
                    .to_string()
            ]
        );
    }

    #[test]
    fn a_campaign_that_declares_nothing_leaves_the_restrictions_alone() {
        let changed = diff_campaign(
            &campaign(""),
            &campaign(&restrictions(AUDIENCE_OBSERVED)),
            false,
        );
        assert!(changed.is_empty(), "{changed:?}");
    }

    /// The API reads a dimension nobody named as targeting, so an entry that
    /// says exactly that is not a difference — otherwise every plan would
    /// propose a write that changes nothing, forever.
    #[test]
    fn an_explicitly_default_entry_matches_an_absent_one() {
        let changed = diff_campaign(
            &campaign(&restrictions(AUDIENCE_TARGETED)),
            &campaign(&restrictions("")),
            false,
        );
        assert!(changed.is_empty(), "{changed:?}");
    }

    #[test]
    fn declaration_order_is_not_a_setting() {
        let changed = diff_campaign(
            &campaign(&restrictions(&format!("{AGE_OBSERVED},{AUDIENCE_OBSERVED}"))),
            &campaign(&restrictions(&format!("{AUDIENCE_OBSERVED},{AGE_OBSERVED}"))),
            false,
        );
        assert!(changed.is_empty(), "{changed:?}");
    }

    /// A declared block owns the whole list, because that is all the API offers:
    /// it removes whatever the body leaves out. So dropping a dimension from the
    /// file is a change, not silence about it.
    #[test]
    fn a_dimension_the_declared_block_omits_is_planned_away() {
        let changed = diff_campaign(
            &campaign(&restrictions(AUDIENCE_OBSERVED)),
            &campaign(&restrictions(&format!("{AGE_OBSERVED},{AUDIENCE_OBSERVED}"))),
            false,
        );
        assert_eq!(
            changed.iter().map(FieldChange::render).collect::<Vec<_>>(),
            vec![
                "targeting_setting.target_restrictions: AGE_RANGE observation, \
                 AUDIENCE observation -> AUDIENCE observation"
                    .to_string()
            ]
        );
    }

    #[test]
    fn an_ad_group_carries_the_same_setting() {
        let changed = diff_ad_group(
            &ad_group(&restrictions(AGE_OBSERVED)),
            &ad_group(&restrictions("")),
        );
        assert_eq!(
            field_names(&changed),
            vec!["targeting_setting.target_restrictions".to_string()]
        );
    }
}

#[cfg(test)]
mod bidding_warning_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    const LIVE: &str = r#"{
        "customer_id": "1",
        "campaign_budgets": [{"id":"7","name":"B","amount_micros":1000000}],
        "campaigns": [{"id":"9","name":"Preroll","advertising_channel_type":"VIDEO",
                       "campaign_budget":"7"}]
    }"#;

    /// A labeled video campaign dropped from the file plans a destroy the API
    /// refuses, and one refusal sinks every unrelated operation in the atomic
    /// batch. Nothing is lost by leaving it alone, so the row is dropped and
    /// the rest of the plan survives (issue #116).
    #[test]
    fn an_undeclared_video_campaign_is_skipped_rather_than_destroyed() {
        const LIVE_LABELED: &str = r#"{
            "customer_id": "1",
            "campaign_budgets": [{"id":"7","name":"B","amount_micros":1000000}],
            "campaigns": [{"id":"9","name":"Preroll","advertising_channel_type":"VIDEO",
                           "campaign_budget":"7","managed_address":"m.google_ads_campaign.preroll"}]
        }"#;
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [{"id":"b","name":"B","amount_micros":1000000}]
        }"#,
        );
        let report = diff(&declared, &input(LIVE_LABELED));
        assert_eq!(report.delete_count, 0, "diffs: {:?}", report.diffs);
        assert!(
            !report
                .diffs
                .iter()
                .any(|d| matches!(d.action, Action::Delete { .. })),
            "a doomed destroy reached the batch: {:?}",
            report.diffs
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("preroll") && w.contains("skipping the removal")),
            "{:?}",
            report.warnings
        );
        assert!(report.blockers.is_empty(), "{:?}", report.blockers);
    }

    /// The same live campaign without a bidsmith label is not bidsmith's to
    /// remove in the first place, so nothing is said about it.
    #[test]
    fn an_unlabeled_video_campaign_is_not_mentioned() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [{"id":"b","name":"B","amount_micros":1000000}]
        }"#,
        );
        let report = diff(&declared, &input(LIVE));
        assert_eq!(report.delete_count, 0, "diffs: {:?}", report.diffs);
        assert!(
            !report.warnings.iter().any(|w| w.contains("skipping the removal")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_new_search_campaign_without_bidding_warns() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [{"id":"b","name":"B","amount_micros":1000000}],
            "campaigns": [{"id":"c","name":"Brand new","advertising_channel_type":"SEARCH",
                           "campaign_budget":"b"}]
        }"#,
        );
        let report = diff(&declared, &input(LIVE));
        assert!(
            report.warnings.iter().any(|w| w.contains("no bidding strategy")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_new_video_campaign_blocks_the_plan_because_the_api_cannot_create_it() {
        // No bidding block makes a video campaign appliable — the channel is
        // read-only through the API, whatever it bids with.
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [{"id":"b","name":"B","amount_micros":1000000}],
            "campaigns": [{"id":"c","name":"Brand new","advertising_channel_type":"VIDEO",
                           "campaign_budget":"b","manual_cpv":{}}]
        }"#,
        );
        let report = diff(&declared, &input(LIVE));
        assert!(
            report.blockers.iter().any(|b| b.contains("cannot create or update VIDEO")),
            "{:?}",
            report.blockers
        );
    }

    #[test]
    fn drift_on_a_live_video_campaign_blocks_before_the_batch_goes_out() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [{"id":"b","name":"B","amount_micros":1000000}],
            "campaigns": [{"id":"c","name":"Preroll","advertising_channel_type":"VIDEO",
                           "campaign_budget":"b","status":"ENABLED"}]
        }"#,
        );
        let report = diff(&declared, &input(LIVE));
        assert!(
            report.blockers.iter().any(|b| b.contains("cannot create or update VIDEO")
                && b.contains("status")),
            "{:?}",
            report.blockers
        );
    }

    #[test]
    fn an_adopted_video_campaign_without_bidding_is_quiet() {
        // Matched by name, so bidding stays whatever the account already bids
        // with — the 88-warning case a purely offline check produced.
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [{"id":"b","name":"B","amount_micros":1000000}],
            "campaigns": [{"id":"c","name":"Preroll","advertising_channel_type":"VIDEO",
                           "campaign_budget":"b"}]
        }"#,
        );
        let report = diff(&declared, &input(LIVE));
        assert!(
            !report.warnings.iter().any(|w| w.contains("no bidding strategy")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_new_search_campaign_with_a_strategy_is_quiet() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [{"id":"b","name":"B","amount_micros":1000000}],
            "campaigns": [{"id":"c","name":"Brand new","advertising_channel_type":"SEARCH",
                           "campaign_budget":"b","manual_cpc":{}}]
        }"#,
        );
        let report = diff(&declared, &input(LIVE));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }
}

#[cfg(test)]
mod shared_budget_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    fn two_campaigns_on(budget: &str) -> ExportInput {
        input(&format!(
            r#"{{
            "customer_id": "1",
            "campaign_budgets": [{budget}],
            "campaigns": [
                {{"id":"a","name":"A","advertising_channel_type":"SEARCH",
                  "campaign_budget":"b","manual_cpc":{{}}}},
                {{"id":"z","name":"Z","advertising_channel_type":"SEARCH",
                  "campaign_budget":"b","manual_cpc":{{}}}}
            ]
        }}"#
        ))
    }

    const EMPTY_LIVE: &str = r#"{"customer_id": "1"}"#;

    #[test]
    fn two_campaigns_on_an_implicitly_shared_budget_warn() {
        let declared = two_campaigns_on(r#"{"id":"b","name":"B","amount_micros":1000000}"#);
        let report = diff(&declared, &input(EMPTY_LIVE));
        let w = report
            .warnings
            .iter()
            .find(|w| w.contains("not explicitly shared"))
            .unwrap_or_else(|| panic!("{:?}", report.warnings));
        assert!(w.contains("backs 2 campaigns") && w.contains("a, z"), "{w}");
    }

    #[test]
    fn an_explicitly_shared_budget_is_fine() {
        let declared = two_campaigns_on(
            r#"{"id":"b","name":"B","amount_micros":1000000,"explicitly_shared":true}"#,
        );
        let report = diff(&declared, &input(EMPTY_LIVE));
        assert!(
            !report.warnings.iter().any(|w| w.contains("not explicitly shared")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn one_campaign_per_budget_is_fine() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [
                {"id":"b1","name":"B1","amount_micros":1000000},
                {"id":"b2","name":"B2","amount_micros":1000000}
            ],
            "campaigns": [
                {"id":"a","name":"A","advertising_channel_type":"SEARCH",
                 "campaign_budget":"b1","manual_cpc":{}},
                {"id":"z","name":"Z","advertising_channel_type":"SEARCH",
                 "campaign_budget":"b2","manual_cpc":{}}
            ]
        }"#,
        );
        let report = diff(&declared, &input(EMPTY_LIVE));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }

    #[test]
    fn same_local_name_in_two_modules_is_two_budgets() {
        // The false positive a source-level check produces: `m1.budget` and
        // `m2.budget` are distinct budgets, each backing one campaign.
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaign_budgets": [
                {"id":"m1.budget","name":"B","amount_micros":1000000},
                {"id":"m2.budget","name":"B","amount_micros":1000000}
            ],
            "campaigns": [
                {"id":"m1.c","name":"A","advertising_channel_type":"SEARCH",
                 "campaign_budget":"m1.budget","manual_cpc":{}},
                {"id":"m2.c","name":"Z","advertising_channel_type":"SEARCH",
                 "campaign_budget":"m2.budget","manual_cpc":{}}
            ]
        }"#,
        );
        let report = diff(&declared, &input(EMPTY_LIVE));
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }
}

#[cfg(test)]
mod budget_period_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        let mut v: ExportInput = serde_json::from_str(json).expect("valid test input");
        v.apply_schema_defaults();
        v
    }

    fn declared(budget: &str) -> ExportInput {
        input(&format!(
            r#"{{"customer_id":"1","campaign_budgets":[{budget}]}}"#
        ))
    }

    #[test]
    fn adopting_a_lifetime_budget_with_a_daily_declaration_warns() {
        let report = diff(
            &declared(r#"{"id":"m.b","name":"Flight","amount_micros":10000000}"#),
            &input(
                r#"{"customer_id":"1","campaign_budgets":[{"id":"900","name":"Flight",
                  "total_amount_micros":91000000,"period":"CUSTOM_PERIOD"}]}"#,
            ),
        );
        let w = report
            .warnings
            .iter()
            .find(|w| w.contains("period"))
            .unwrap_or_else(|| panic!("{:?}", report.warnings));
        assert!(w.contains("\"DAILY\"") && w.contains("\"CUSTOM_PERIOD\""), "{w}");
    }

    #[test]
    fn a_matching_lifetime_budget_diffs_the_total_not_the_daily_amount() {
        let report = diff(
            &declared(
                r#"{"id":"m.b","name":"Flight","total_amount_micros":150000000,
                  "period":"CUSTOM_PERIOD"}"#,
            ),
            &input(
                r#"{"customer_id":"1","campaign_budgets":[{"id":"900","name":"Flight",
                  "amount_micros":10000000,"total_amount_micros":91000000,
                  "period":"CUSTOM_PERIOD"}]}"#,
            ),
        );
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let fields: Vec<&str> = match &report.diffs[0].action {
            Action::Update { changed_fields, .. } => {
                changed_fields.iter().map(|c| c.field.as_str()).collect()
            }
            other => panic!("expected an update, got {other:?}"),
        };
        assert_eq!(fields, ["total_amount_micros"]);
    }

    #[test]
    fn a_budget_type_the_file_names_wrong_warns() {
        let report = diff(
            &declared(
                r#"{"id":"m.b","name":"B","amount_micros":10000000,"type":"STANDARD"}"#,
            ),
            &input(
                r#"{"customer_id":"1","campaign_budgets":[{"id":"900","name":"B",
                  "amount_micros":10000000,"type":"SMART_CAMPAIGN"}]}"#,
            ),
        );
        assert!(
            report.warnings.iter().any(|w| w.contains("type")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn a_type_the_file_leaves_open_is_not_drift() {
        let report = diff(
            &declared(r#"{"id":"m.b","name":"B","amount_micros":10000000}"#),
            &input(
                r#"{"customer_id":"1","campaign_budgets":[{"id":"900","name":"B",
                  "amount_micros":10000000,"type":"SMART_CAMPAIGN"}]}"#,
            ),
        );
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }
}

#[cfg(test)]
mod adopt_only_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        let mut v: ExportInput = serde_json::from_str(json).expect("valid test input");
        v.apply_schema_defaults();
        v
    }

    const NAME: &str = "GH_YouTube_FR Instream";

    fn declared_video(adopt_only: bool) -> ExportInput {
        let lifecycle = if adopt_only { r#""adopt_only": ["m.c"],"# } else { "" };
        input(&format!(
            r#"{{
            "customer_id": "1",
            {lifecycle}
            "campaign_budgets": [{{"id":"m.b","name":"B","amount_micros":1000000}}],
            "campaigns": [{{"id":"m.c","name":"{NAME}","advertising_channel_type":"VIDEO",
              "campaign_budget":"m.b"}}]
        }}"#
        ))
    }

    fn live_video(name: &str, status: &str) -> ExportInput {
        input(&format!(
            r#"{{
            "customer_id": "1",
            "campaign_budgets": [{{"id":"900","name":"B","amount_micros":1000000}}],
            "campaigns": [{{"id":"500","name":"{name}","status":"{status}",
              "advertising_channel_type":"VIDEO","campaign_budget":"900"}}]
        }}"#
        ))
    }

    const EMPTY_LIVE: &str = r#"{"customer_id": "1"}"#;

    #[test]
    fn an_unmatched_adopt_only_resource_blocks_the_plan() {
        let report = diff(&declared_video(true), &input(EMPTY_LIVE));
        let b = report
            .blockers
            .iter()
            .find(|b| b.contains("adopt-only"))
            .unwrap_or_else(|| panic!("{:?}", report.blockers));
        assert!(b.contains("m.c is declared adopt-only"), "{b}");
        assert!(b.contains(&format!("by name {NAME:?}")), "{b}");
    }

    #[test]
    fn an_unmatched_adopt_only_resource_raises_one_blocker_not_two() {
        // Without the suppression a VIDEO campaign draws the channel blocker
        // too, and the vaguer message wins the reader's attention.
        let report = diff(&declared_video(true), &input(EMPTY_LIVE));
        assert_eq!(report.blockers.len(), 1, "{:?}", report.blockers);
    }

    #[test]
    fn without_the_lifecycle_block_the_same_file_only_reports_the_channel() {
        let report = diff(&declared_video(false), &input(EMPTY_LIVE));
        assert_eq!(report.blockers.len(), 1, "{:?}", report.blockers);
        assert!(report.blockers[0].contains("would be created"), "{:?}", report.blockers);
    }

    #[test]
    fn an_adopt_only_resource_that_matches_live_is_quiet() {
        let report = diff(&declared_video(true), &live_video(NAME, "ENABLED"));
        assert!(report.blockers.is_empty(), "{:?}", report.blockers);
        assert_eq!(report.create_count, 0);
    }

    #[test]
    fn drift_on_a_matched_adopt_only_video_campaign_still_blocks() {
        // create = false says nothing about updates, and the API still refuses
        // them on this channel.
        let report = diff(&declared_video(true), &live_video(NAME, "PAUSED"));
        let b = report
            .blockers
            .iter()
            .find(|b| b.contains("has drift on status"))
            .unwrap_or_else(|| panic!("{:?}", report.blockers));
        assert!(b.contains("m.c"), "{b}");
    }

    #[test]
    fn a_renamed_live_campaign_is_reported_as_a_failed_adoption() {
        let report = diff(&declared_video(true), &live_video("Renamed In The UI", "ENABLED"));
        assert!(
            report.blockers.iter().any(|b| b.contains("adopt-only")),
            "{:?}",
            report.blockers
        );
    }

    #[test]
    fn adopt_only_works_on_kinds_other_than_campaigns() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "adopt_only": ["m.b"],
            "campaign_budgets": [{"id":"m.b","name":"Always On","amount_micros":1000000}]
        }"#,
        );
        let report = diff(&declared, &input(EMPTY_LIVE));
        let b = report
            .blockers
            .first()
            .unwrap_or_else(|| panic!("{:?}", report.blockers));
        assert!(b.contains("no live campaign budget matched it"), "{b}");
        assert!(b.contains(r#"by name "Always On""#), "{b}");
    }

    #[test]
    fn an_undeclared_address_in_adopt_only_changes_nothing() {
        let declared = input(
            r#"{
            "customer_id": "1",
            "adopt_only": ["m.gone"],
            "campaign_budgets": [{"id":"m.b","name":"B","amount_micros":1000000}]
        }"#,
        );
        let report = diff(&declared, &input(EMPTY_LIVE));
        assert!(report.blockers.is_empty(), "{:?}", report.blockers);
    }
}

#[cfg(test)]
mod tracking_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    fn changed_fields(declared: &ExportInput, live: &ExportInput, address: &str) -> Vec<String> {
        diff(declared, live)
            .diffs
            .iter()
            .find(|d| d.address == address)
            .map(|d| match &d.action {
                Action::Update { changed_fields, .. } => {
                    changed_fields.iter().map(FieldChange::render).collect()
                }
                _ => Vec::new(),
            })
            .unwrap_or_default()
    }

    const LIVE: &str = r#"{
        "customer_id": "1",
        "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200","managed_address":"m.google_ads_campaign.c","final_url_suffix":"utm_source=google","custom_parameters":[{"key":"region","value":"us"}]}],
        "campaign_budgets": [{"id":"200","name":"B","amount_micros":1000000,"managed_address":"m.google_ads_campaign_budget.b"}]
    }"#;

    fn declared(suffix: &str, params: &str) -> ExportInput {
        input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"m.google_ads_campaign.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.google_ads_campaign_budget.b"{suffix}{params}}}],
            "campaign_budgets": [{{"id":"m.google_ads_campaign_budget.b","name":"B","amount_micros":1000000}}]
        }}"#
        ))
    }

    #[test]
    fn a_changed_suffix_is_an_update_not_a_recreate() {
        let changes = changed_fields(
            &declared(r#","final_url_suffix":"utm_source=bing""#, ""),
            &input(LIVE),
            "m.google_ads_campaign.c",
        );
        assert_eq!(
            changes,
            vec![r#"final_url_suffix: "utm_source=google" -> "utm_source=bing""#.to_string()]
        );
    }

    #[test]
    fn an_omitted_suffix_is_unmanaged_not_a_request_to_clear_it() {
        // The rule everywhere else in bidsmith: saying nothing is not the same
        // as saying "none".
        let changes = changed_fields(&declared("", ""), &input(LIVE), "m.google_ads_campaign.c");
        assert!(changes.is_empty(), "{changes:?}");
    }

    #[test]
    fn custom_parameters_render_as_a_readable_map_in_the_diff() {
        let changes = changed_fields(
            &declared(
                r#","final_url_suffix":"utm_source=google""#,
                r#","custom_parameters":[{"key":"region","value":"eu"}]"#,
            ),
            &input(LIVE),
            "m.google_ads_campaign.c",
        );
        assert_eq!(
            changes,
            vec![r#"custom_parameters: "{region=us}" -> "{region=eu}""#.to_string()]
        );
    }

    #[test]
    fn a_declared_empty_map_clears_the_parameters() {
        // Unlike omitting the attribute, writing `custom_parameters = {}` is an
        // explicit statement that there should be none.
        let changes = changed_fields(
            &declared(r#","final_url_suffix":"utm_source=google""#, r#","custom_parameters":[]"#),
            &input(LIVE),
            "m.google_ads_campaign.c",
        );
        assert_eq!(changes, vec![r#"custom_parameters: "{region=us}" -> "{}""#.to_string()]);
    }
}

#[cfg(test)]
mod asset_adoption_tests {
    use super::*;

    /// Both sides normalized the way plan normalizes them, so an omitted
    /// `status` does not read as drift against the live default.
    fn input(json: &str) -> ExportInput {
        let mut i: ExportInput = serde_json::from_str(json).expect("valid test input");
        i.apply_schema_defaults();
        i
    }

    const DECLARED: &str = r#"{
        "customer_id": "9",
        "sitelink_assets": [
            {"id":"m.google_ads_sitelink_asset.shop","link_text":"Shop",
             "final_urls":["https://example.com/shop"]}
        ],
        "customer_assets": [
            {"id":"m.google_ads_customer_asset.shop_link",
             "asset":"m.google_ads_sitelink_asset.shop","field_type":"SITELINK"}
        ]
    }"#;

    fn action_for(report: &DiffReport, address: &str) -> String {
        let d = report
            .diffs
            .iter()
            .find(|d| d.address == address)
            .unwrap_or_else(|| panic!("no diff for {address}"));
        match &d.action {
            Action::NoOp { .. } => "noop".to_string(),
            Action::Create => "create".to_string(),
            Action::Update { .. } => "update".to_string(),
            Action::Delete { .. } => "delete".to_string(),
            Action::Pause { .. } => "pause".to_string(),
        }
    }

    #[test]
    fn an_unlabeled_live_sitelink_and_its_account_link_are_adopted_not_recreated() {
        // The on-ramp case: both were made in the UI, so neither carries a
        // bidsmith label. Content is all there is to match on, and it is enough.
        let live = input(
            r#"{
            "customer_id": "9",
            "sitelink_assets": [
                {"id":"4001","link_text":"Shop","final_urls":["https://example.com/shop"]}
            ],
            "customer_assets": [
                {"id":"4001~SITELINK","asset":"4001","field_type":"SITELINK","status":"ENABLED"}
            ]
        }"#,
        );
        let report = diff(&input(DECLARED), &live);
        assert_eq!(action_for(&report, "m.google_ads_sitelink_asset.shop"), "noop");
        assert_eq!(
            action_for(&report, "m.google_ads_customer_asset.shop_link"),
            "noop",
            "an account-level link that already exists must not be created twice",
        );
    }

    #[test]
    fn two_identical_live_sitelinks_are_reported_rather_than_picked_from_silently() {
        let live = input(
            r#"{
            "customer_id": "9",
            "sitelink_assets": [
                {"id":"4001","link_text":"Shop","final_urls":["https://example.com/shop"]},
                {"id":"4002","link_text":"Shop","final_urls":["https://example.com/shop"]}
            ]
        }"#,
        );
        let report = diff(&input(DECLARED), &live);
        let w = report
            .warnings
            .iter()
            .find(|w| w.contains("m.google_ads_sitelink_asset.shop"))
            .unwrap_or_else(|| panic!("{:?}", report.warnings));
        assert!(w.contains("4001") && w.contains("4002"), "{w}");
        assert!(w.contains("adopted 4001"), "the adopted one has to be named: {w}");
    }

    #[test]
    fn a_single_match_says_nothing() {
        let live = input(
            r#"{
            "customer_id": "9",
            "sitelink_assets": [
                {"id":"4001","link_text":"Shop","final_urls":["https://example.com/shop"]},
                {"id":"4002","link_text":"Support","final_urls":["https://example.com/help"]}
            ]
        }"#,
        );
        let report = diff(&input(DECLARED), &live);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    }
}

#[cfg(test)]
mod asset_prune_tests {
    use super::*;

    fn input(json: &str) -> ExportInput {
        serde_json::from_str(json).expect("valid test input")
    }

    /// The same campaign, claiming whatever Google invented for it.
    fn declared_owning_automatic() -> ExportInput {
        let mut d = declared("");
        d.campaigns[0].owns_automatic_assets = true;
        d
    }

    /// One campaign, one declared sitelink on it. `extra` adds declared blocks.
    fn declared(extra: &str) -> ExportInput {
        input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}}],
            "sitelink_assets": [
                {{"id":"m.docs","link_text":"Docs","final_urls":["https://example.com/docs"]}}
            ],
            "campaign_assets": [
                {{"id":"m.link","campaign":"m.c","asset":"m.docs","field_type":"SITELINK"}}
            ]{extra}
        }}"#
        ))
    }

    /// The same campaign live, serving the declared sitelink plus a second one
    /// nobody declared, plus an undeclared callout. `extra` adds live state.
    fn live(source: &str, extra: &str) -> ExportInput {
        input(&format!(
            r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}}],
            "sitelink_assets": [
                {{"id":"900","link_text":"Docs","final_urls":["https://example.com/docs"]}},
                {{"id":"901","link_text":"Also on Firefox","final_urls":["https://example.com/ff"]}}
            ],
            "callout_assets": [{{"id":"910","text":"Install Now!"}}],
            "campaign_assets": [
                {{"id":"100~900~SITELINK","campaign":"100","asset":"900","field_type":"SITELINK","source":"ADVERTISER"}},
                {{"id":"100~901~SITELINK","campaign":"100","asset":"901","field_type":"SITELINK","source":"{source}"}},
                {{"id":"100~910~CALLOUT","campaign":"100","asset":"910","field_type":"CALLOUT","source":"ADVERTISER"}}
            ]{extra}
        }}"#
        ))
    }

    fn destroys(report: &DiffReport) -> Vec<(&str, &str)> {
        report
            .diffs
            .iter()
            .filter(|d| matches!(d.action, Action::Delete { .. }))
            .map(|d| (d.kind, d.address.as_str()))
            .collect()
    }

    #[test]
    fn a_declared_campaign_owns_the_field_type_it_declares() {
        let report = diff(&declared(""), &live("ADVERTISER", ""));
        assert_eq!(
            destroys(&report),
            vec![("campaign_asset", "m.c (removed sitelink \"Also on Firefox\")")],
            "the undeclared sitelink goes, the callout stays: {:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        assert!(report.claim_plans.iter().any(|p| p.category == "asset_sitelink"
            && p.kind == "campaign"
            && p.stale_assoc_rn.is_none()));
    }

    #[test]
    fn a_campaign_declaring_no_links_is_left_exactly_as_it_was() {
        let bare = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
        }"#,
        );
        let report = diff(&bare, &live("ADVERTISER", ""));
        assert_eq!(report.delete_count, 0, "{:?}", destroys(&report));
        assert!(
            !report.claim_plans.iter().any(|p| is_asset_category(p.category)),
            "nothing was declared, so nothing is claimed: {:?}",
            report.claim_plans
        );
    }

    #[test]
    fn a_live_claim_prunes_after_the_last_declared_link_goes() {
        // Same shape as issue #88 for criteria: the file no longer declares any
        // sitelink, but the campaign's claim label proves bidsmith owned them.
        let bare = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
        }"#,
        );
        let report = diff(
            &bare,
            &live(
                "ADVERTISER",
                r#",
            "campaign_claims": {"100": ["asset_sitelink"]},
            "claim_labels": {"asset_sitelink": "customers/1/labels/777"}"#,
            ),
        );
        let mut gone = destroys(&report);
        gone.sort();
        assert_eq!(
            gone,
            vec![
                ("campaign_asset", "m.c (removed sitelink \"Also on Firefox\")"),
                ("campaign_asset", "m.c (removed sitelink \"Docs\")"),
            ],
            "{:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        let release = report
            .claim_plans
            .iter()
            .find(|p| p.category == "asset_sitelink" && p.stale_assoc_rn.is_some())
            .unwrap_or_else(|| panic!("{:?}", report.claim_plans));
        assert_eq!(
            release.stale_assoc_rn.as_deref(),
            Some("customers/1/campaignLabels/100~777")
        );
    }

    fn pauses(report: &DiffReport) -> Vec<(&str, &str)> {
        report
            .diffs
            .iter()
            .filter(|d| matches!(d.action, Action::Pause { .. }))
            .map(|d| (d.kind, d.address.as_str()))
            .collect()
    }

    /// The campaign declares a sitelink, so it owns its sitelinks — and what
    /// Google invented inside that partition stops serving without the file
    /// having to say anything more.
    #[test]
    fn an_automatically_created_link_in_an_owned_partition_is_paused() {
        let report = diff(&declared(""), &live(AUTOMATICALLY_CREATED, ""));
        assert_eq!(report.delete_count, 0, "{:?}", destroys(&report));
        assert_eq!(
            pauses(&report),
            vec![("campaign_asset", "m.c (paused sitelink \"Also on Firefox\")")],
            "{:?}",
            report.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        assert_eq!(report.pause_count, 1);
        assert_eq!(report.skipped_removal_count, 0);
    }

    /// Pausing has to be a fixed point, or every plan would propose it again.
    #[test]
    fn a_link_already_paused_is_left_alone() {
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "sitelink_assets": [
                {"id":"900","link_text":"Docs","final_urls":["https://example.com/docs"]},
                {"id":"901","link_text":"Also on Firefox","final_urls":["https://example.com/ff"]}
            ],
            "campaign_assets": [
                {"id":"100~900~SITELINK","campaign":"100","asset":"900","field_type":"SITELINK","source":"ADVERTISER"},
                {"id":"100~901~SITELINK","campaign":"100","asset":"901","field_type":"SITELINK","source":"AUTOMATICALLY_CREATED","status":"PAUSED"}
            ]
        }"#,
        );
        let report = diff(&declared(""), &live);
        assert_eq!(report.pause_count, 0, "{:?}", pauses(&report));
        assert_eq!(report.delete_count, 0, "{:?}", destroys(&report));
    }

    /// A business name is not a kind of block, so no declaration could ever
    /// prove the claim over it — the campaign has to say so outright.
    #[test]
    fn a_field_type_nothing_can_declare_waits_for_the_opt_in() {
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "campaign_assets": [
                {"id":"100~950~BUSINESS_NAME","campaign":"100","asset":"950","field_type":"BUSINESS_NAME","source":"AUTOMATICALLY_CREATED"}
            ]
        }"#,
        );
        let reported = diff(&declared(""), &live);
        assert_eq!(reported.pause_count, 0, "{:?}", pauses(&reported));
        assert!(
            reported.warnings.iter().any(|w| w.contains("1 BUSINESS_NAME")),
            "{:?}",
            reported.warnings
        );

        let claimed = diff(&declared_owning_automatic(), &live);
        assert_eq!(
            pauses(&claimed),
            vec![("campaign_asset", "m.c (paused business_name asset 950)")],
            "{:?}",
            claimed.diffs.iter().map(|d| (&d.address, &d.action)).collect::<Vec<_>>()
        );
        assert!(
            !claimed.warnings.iter().any(|w| w.contains("asset automation")),
            "nothing is left to report once it is being paused: {:?}",
            claimed.warnings
        );
    }

    /// Google picks the level it attaches to, and no ad-group block could have
    /// named a business name either, so the campaign's claim reaches its
    /// ad groups.
    #[test]
    fn an_ad_group_link_is_covered_by_its_campaigns_claim() {
        let live = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}],
            "ad_groups": [{"id":"300","name":"G","campaign":"100"}],
            "ad_group_assets": [
                {"id":"300~960~LOGO","ad_group":"300","asset":"960","field_type":"LOGO","source":"AUTOMATICALLY_CREATED"}
            ]
        }"#,
        );
        let declared = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b","owns_automatic_assets":true}],
            "ad_groups": [{"id":"m.g","name":"G","campaign":"m.c"}]
        }"#,
        );
        assert_eq!(
            pauses(&diff(&declared, &live)),
            vec![("ad_group_asset", "m.g (paused logo asset 960)")]
        );
    }

    /// Account-wide links reach every campaign at once, so the claim over them
    /// is the provider block's, never a campaign's.
    #[test]
    fn an_account_level_automatic_link_waits_for_the_provider_claim() {
        let live_account = |extra: &str| {
            input(&format!(
                r#"{{
            "customer_id": "1",
            "campaigns": [{{"id":"100","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"200"}}],
            "callout_assets": [{{"id":"970","text":"Install Now!"}}],
            "customer_assets": [
                {{"id":"970~CALLOUT","asset":"970","field_type":"CALLOUT","source":"AUTOMATICALLY_CREATED"}}
            ]{extra}
        }}"#
            ))
        };
        let ignored = diff(&declared_owning_automatic(), &live_account(""));
        assert_eq!(ignored.pause_count, 0, "{:?}", pauses(&ignored));

        let mut claiming = declared("");
        claiming.owns_account_automatic_assets = true;
        assert_eq!(
            pauses(&diff(&claiming, &live_account(""))),
            vec![("customer_asset", "account (paused account-level callout \"Install Now!\")")]
        );
    }

    /// A campaign declaring no links of its own owns nothing to prune, so a
    /// sitelink Google invented for it used to leave no trace in the plan at
    /// all. The switch behind it is account-level and outside the API, so
    /// saying so on every run is the whole of the enforcement (issue #152).
    #[test]
    fn assets_google_invented_are_reported_even_where_nothing_is_pruned() {
        let bare = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
        }"#,
        );
        let report = diff(&bare, &live(AUTOMATICALLY_CREATED, ""));
        assert_eq!(report.delete_count, 0, "{:?}", destroys(&report));
        let w = report
            .warnings
            .iter()
            .find(|w| w.contains("asset automation"))
            .unwrap_or_else(|| panic!("{:?}", report.warnings));
        assert!(w.contains("1 SITELINK"), "{w}");
        assert!(w.contains("automatically_created_assets"), "{w}");
    }

    /// Only what Google put there: a link someone added in the UI is a
    /// different problem, and prune is already the answer to it.
    #[test]
    fn links_an_advertiser_added_are_not_reported_as_automation() {
        let bare = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"SEARCH","campaign_budget":"m.b"}]
        }"#,
        );
        let report = diff(&bare, &live("ADVERTISER", ""));
        assert!(
            !report.warnings.iter().any(|w| w.contains("asset automation")),
            "{:?}",
            report.warnings
        );
    }

    #[test]
    fn account_level_links_need_the_provider_to_claim_them() {
        let account_live = r#",
            "customer_assets": [
                {"id":"910~CALLOUT","asset":"910","field_type":"CALLOUT","source":"ADVERTISER"}
            ]"#;

        let report = diff(&declared(""), &live("ADVERTISER", account_live));
        assert!(
            !destroys(&report).iter().any(|(kind, _)| *kind == "customer_asset"),
            "an account-wide link is never pruned implicitly: {:?}",
            destroys(&report)
        );

        let owning = declared(r#", "owned_account_assets": ["CALLOUT"]"#);
        let report = diff(&owning, &live("ADVERTISER", account_live));
        assert!(
            destroys(&report)
                .contains(&("customer_asset", "account (removed account-level callout \"Install Now!\")")),
            "{:?}",
            destroys(&report)
        );
    }

    #[test]
    fn a_link_on_a_video_campaign_is_skipped_not_destroyed() {
        let declared_video = input(
            r#"{
            "customer_id": "1",
            "campaigns": [{"id":"m.c","name":"C","advertising_channel_type":"VIDEO","campaign_budget":"m.b"}],
            "sitelink_assets": [
                {"id":"m.docs","link_text":"Docs","final_urls":["https://example.com/docs"]}
            ],
            "campaign_assets": [
                {"id":"m.link","campaign":"m.c","asset":"m.docs","field_type":"SITELINK"}
            ]
        }"#,
        );
        let mut video_live = live("ADVERTISER", "");
        video_live.campaigns[0].advertising_channel_type = "VIDEO".to_string();
        let report = diff(&declared_video, &video_live);
        assert_eq!(report.delete_count, 0, "{:?}", destroys(&report));
        assert_eq!(report.skipped_removal_count, 1);
        assert!(
            report.warnings.iter().any(|w| w.contains("VIDEO channel")),
            "{:?}",
            report.warnings
        );
    }
}
