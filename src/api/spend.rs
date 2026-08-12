use std::collections::{HashMap, HashSet};

use crate::api::diff::{Action, DiffReport};
use crate::commands::export::{ExportInput, JsonBudget};

const ENABLED: &str = "ENABLED";
const MICROS_PER_UNIT: u64 = 1_000_000;

/// How much daily spend the account has committed before and after the
/// changeset — the question a reviewer of an ads diff actually has, and the one
/// operation counts never answer (issue #117).
///
/// Only budgets backing a campaign that will be `ENABLED` after apply are
/// counted: a budget on a paused campaign commits nothing. A budget several
/// campaigns share counts once.
///
/// A `CUSTOM_PERIOD` budget commits a lifetime total rather than a daily rate,
/// so it is reported on its own line instead of being summed into a figure
/// labelled `/day` (issue #131).
pub struct SpendSummary {
    pub before_micros: i64,
    pub after_micros: i64,
    pub enabled_campaigns_after: usize,
    pub custom_before_micros: i64,
    pub custom_after_micros: i64,
    pub custom_budgets_after: usize,
    pub currency_code: Option<String>,
}

/// Identifies a budget across the live / declared boundary, so a declared
/// budget and the live one it adopts collapse into a single entry.
#[derive(PartialEq, Eq, Hash, Clone, Copy)]
enum BudgetKey<'a> {
    Live(&'a str),
    Declared(&'a str),
}

impl SpendSummary {
    pub fn delta_micros(&self) -> i64 {
        self.after_micros - self.before_micros
    }

    /// The summary, one line per unit of commitment — empty when the account
    /// commits nothing either way and the lines would only add noise.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(daily) = self.daily_line() {
            out.push(daily);
        }
        if let Some(custom) = self.custom_period_line() {
            out.push(if out.is_empty() {
                custom
            } else {
                format!("plus {custom}")
            });
        }
        out
    }

    fn daily_line(&self) -> Option<String> {
        if self.before_micros == 0 && self.after_micros == 0 {
            return None;
        }
        let campaigns = match self.enabled_campaigns_after {
            1 => "1 enabled campaign".to_string(),
            n => format!("{n} enabled campaigns"),
        };
        let delta = self.delta_micros();
        if delta == 0 {
            return Some(format!(
                "committed daily spend {}/day across {campaigns}",
                self.money(self.after_micros),
            ));
        }
        Some(format!(
            "committed daily spend {} -> {} ({}{}/day) across {campaigns}",
            self.money(self.before_micros),
            self.money(self.after_micros),
            if delta > 0 { "+" } else { "-" },
            self.money(delta.abs()),
        ))
    }

    /// Custom-period budgets are lifetime caps, so they get their own sentence
    /// with no `/day` on it and no delta to read as a rate change.
    fn custom_period_line(&self) -> Option<String> {
        if self.custom_budgets_after == 0 {
            return None;
        }
        let budgets = match self.custom_budgets_after {
            1 => "1 custom-period budget".to_string(),
            n => format!("{n} custom-period budgets"),
        };
        let totals = if self.custom_before_micros == self.custom_after_micros {
            self.money(self.custom_after_micros)
        } else {
            format!(
                "{} -> {}",
                self.money(self.custom_before_micros),
                self.money(self.custom_after_micros),
            )
        };
        Some(format!("{budgets} totalling {totals} over their lifetime"))
    }

    fn money(&self, micros: i64) -> String {
        match &self.currency_code {
            Some(code) => format!("{} {code}", format_micros(micros)),
            None => format_micros(micros),
        }
    }
}

/// Micros of the account currency as a plain decimal amount, rounded to the
/// nearest hundredth.
fn format_micros(micros: i64) -> String {
    let sign = if micros < 0 { "-" } else { "" };
    let hundredths = (micros.unsigned_abs() + MICROS_PER_UNIT / 200) / (MICROS_PER_UNIT / 100);
    format!("{sign}{}.{:02}", hundredths / 100, hundredths % 100)
}

pub fn summarize(declared: &ExportInput, live: &ExportInput, report: &DiffReport) -> SpendSummary {
    let mut budget_match: HashMap<&str, &str> = HashMap::new();
    let mut campaign_match: HashMap<&str, &str> = HashMap::new();
    let mut removed_campaigns: HashSet<&str> = HashSet::new();
    for d in &report.diffs {
        match (d.kind, &d.action) {
            ("campaign_budget", action) => {
                if let Some(live_id) = action.live_id() {
                    budget_match.insert(d.address.as_str(), live_id);
                }
            }
            ("campaign", Action::Delete { live_id }) => {
                removed_campaigns.insert(live_id.as_str());
            }
            ("campaign", action) => {
                if let Some(live_id) = action.live_id() {
                    campaign_match.insert(d.address.as_str(), live_id);
                }
            }
            _ => {}
        }
    }

    let live_amount: HashMap<&str, Commitment> = live
        .campaign_budgets
        .iter()
        .map(|b| (b.id.as_str(), Commitment::of(b)))
        .collect();
    let mut after_amount: HashMap<BudgetKey, Commitment> = live_amount
        .iter()
        .map(|(id, c)| (BudgetKey::Live(id), *c))
        .collect();
    for b in &declared.campaign_budgets {
        after_amount.insert(budget_key(&b.id, &budget_match), Commitment::of(b));
    }

    let mut before_used: HashSet<&str> = HashSet::new();
    for c in &live.campaigns {
        if is_enabled(c.status.as_deref()) {
            before_used.insert(c.campaign_budget.as_str());
        }
    }

    // A live campaign a declared one matched is superseded by it: the declared
    // status and budget are what the account ends up with.
    let superseded: HashSet<&str> = campaign_match.values().copied().collect();
    let mut after_used: HashSet<BudgetKey> = HashSet::new();
    let mut enabled_campaigns_after = 0usize;
    for c in &live.campaigns {
        let id = c.id.as_str();
        if superseded.contains(id) || removed_campaigns.contains(id) {
            continue;
        }
        if is_enabled(c.status.as_deref()) {
            enabled_campaigns_after += 1;
            after_used.insert(BudgetKey::Live(c.campaign_budget.as_str()));
        }
    }
    for c in &declared.campaigns {
        if is_enabled(c.status.as_deref()) {
            enabled_campaigns_after += 1;
            after_used.insert(budget_key(&c.campaign_budget, &budget_match));
        }
    }

    let before = Totals::of(before_used.iter().filter_map(|id| live_amount.get(id)));
    let after = Totals::of(after_used.iter().filter_map(|k| after_amount.get(k)));

    SpendSummary {
        before_micros: before.daily_micros,
        after_micros: after.daily_micros,
        enabled_campaigns_after,
        custom_before_micros: before.custom_micros,
        custom_after_micros: after.custom_micros,
        custom_budgets_after: after.custom_budgets,
        currency_code: live.currency_code.clone(),
    }
}

/// What one budget commits, in the unit its period implies.
#[derive(Clone, Copy)]
struct Commitment {
    micros: i64,
    custom_period: bool,
}

impl Commitment {
    fn of(b: &JsonBudget) -> Self {
        Commitment {
            micros: b.committed_micros(),
            custom_period: b.is_custom_period(),
        }
    }
}

#[derive(Default)]
struct Totals {
    daily_micros: i64,
    custom_micros: i64,
    custom_budgets: usize,
}

impl Totals {
    fn of<'a>(commitments: impl Iterator<Item = &'a Commitment>) -> Self {
        let mut t = Totals::default();
        for c in commitments {
            if c.custom_period {
                t.custom_micros += c.micros;
                t.custom_budgets += 1;
            } else {
                t.daily_micros += c.micros;
            }
        }
        t
    }
}

fn budget_key<'a>(address: &'a str, matched: &HashMap<&'a str, &'a str>) -> BudgetKey<'a> {
    match matched.get(address) {
        Some(live_id) => BudgetKey::Live(live_id),
        None => BudgetKey::Declared(address),
    }
}

/// Campaigns with no status are treated as enabled, matching the schema default
/// both sides carry by the time the diff runs.
fn is_enabled(status: Option<&str>) -> bool {
    status.unwrap_or(ENABLED) == ENABLED
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::diff;

    fn input(json: &str) -> ExportInput {
        let mut v: ExportInput = serde_json::from_str(json).expect("valid test input");
        v.apply_schema_defaults();
        v
    }

    fn summary(declared: &str, live: &str) -> SpendSummary {
        let declared = input(declared);
        let live = input(live);
        let report = diff::diff(&declared, &live);
        summarize(&declared, &live, &report)
    }

    const NO_LIVE: &str = r#"{"customer_id":"1","currency_code":"EUR"}"#;

    #[test]
    fn micros_render_as_money() {
        assert_eq!(format_micros(20_000_000), "20.00");
        assert_eq!(format_micros(0), "0.00");
        assert_eq!(format_micros(1_234_500), "1.23");
        assert_eq!(format_micros(1_235_000), "1.24");
        assert_eq!(format_micros(-20_000_000), "-20.00");
    }

    #[test]
    fn adopting_a_live_campaign_reports_the_budget_it_commits() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"m.c","name":"Preroll","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}}]
            }"#,
            r#"{
                "customer_id":"1","currency_code":"EUR",
                "campaign_budgets":[{"id":"900","name":"Other","amount_micros":300000000}],
                "campaigns":[{"id":"500","name":"Running","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"900"}]
            }"#,
        );
        assert_eq!(s.before_micros, 300_000_000);
        assert_eq!(s.after_micros, 320_000_000);
        assert_eq!(s.enabled_campaigns_after, 2);
        assert_eq!(
            s.lines(),
            ["committed daily spend 300.00 EUR -> 320.00 EUR (+20.00 EUR/day) \
              across 2 enabled campaigns"]
        );
    }

    #[test]
    fn a_paused_campaign_commits_nothing() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"m.c","name":"Preroll","status":"PAUSED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}}]
            }"#,
            NO_LIVE,
        );
        assert_eq!(s.after_micros, 0);
        assert_eq!(s.enabled_campaigns_after, 0);
        assert!(s.lines().is_empty());
    }

    #[test]
    fn pausing_a_live_campaign_releases_its_budget() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"m.c","name":"Running","status":"PAUSED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}}]
            }"#,
            r#"{
                "customer_id":"1","currency_code":"EUR",
                "campaign_budgets":[{"id":"900","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"500","name":"Running","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"900"}]
            }"#,
        );
        assert_eq!(s.before_micros, 20_000_000);
        assert_eq!(s.after_micros, 0);
        assert_eq!(
            s.lines(),
            ["committed daily spend 20.00 EUR -> 0.00 EUR (-20.00 EUR/day) \
              across 0 enabled campaigns"]
        );
    }

    #[test]
    fn a_shared_budget_counts_once() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":20000000,
                  "explicitly_shared":true}],
                "campaigns":[
                  {"id":"m.a","name":"A","status":"ENABLED",
                   "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}},
                  {"id":"m.z","name":"Z","status":"ENABLED",
                   "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}}
                ]
            }"#,
            NO_LIVE,
        );
        assert_eq!(s.after_micros, 20_000_000);
        assert_eq!(s.enabled_campaigns_after, 2);
    }

    #[test]
    fn raising_an_adopted_budget_reports_only_the_increase() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":30000000}],
                "campaigns":[{"id":"m.c","name":"Running","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}}]
            }"#,
            r#"{
                "customer_id":"1","currency_code":"EUR",
                "campaign_budgets":[{"id":"900","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"500","name":"Running","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"900"}]
            }"#,
        );
        assert_eq!(s.before_micros, 20_000_000);
        assert_eq!(s.after_micros, 30_000_000);
        assert_eq!(s.enabled_campaigns_after, 1);
    }

    #[test]
    fn destroying_a_managed_campaign_releases_its_budget() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":20000000}]
            }"#,
            r#"{
                "customer_id":"1","currency_code":"EUR",
                "campaign_budgets":[{"id":"900","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"500","name":"Gone","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"900",
                  "managed_address":"m.google_ads_campaign.gone"}]
            }"#,
        );
        assert_eq!(s.before_micros, 20_000_000);
        assert_eq!(s.after_micros, 0);
    }

    #[test]
    fn an_unmanaged_live_campaign_stays_in_the_total() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"m.c","name":"Mine","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}}]
            }"#,
            r#"{
                "customer_id":"1","currency_code":"EUR",
                "campaign_budgets":[{"id":"900","name":"Theirs","amount_micros":50000000}],
                "campaigns":[{"id":"500","name":"Hand Made","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"900"}]
            }"#,
        );
        assert_eq!(s.before_micros, 50_000_000);
        assert_eq!(s.after_micros, 70_000_000);
        assert_eq!(s.enabled_campaigns_after, 2);
    }

    #[test]
    fn an_unchanged_plan_still_reports_the_account_total() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"m.c","name":"Running","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}}]
            }"#,
            r#"{
                "customer_id":"1","currency_code":"EUR",
                "campaign_budgets":[{"id":"900","name":"B","amount_micros":20000000,
                  "delivery_method":"STANDARD","explicitly_shared":false}],
                "campaigns":[{"id":"500","name":"Running","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"900"}]
            }"#,
        );
        assert_eq!(s.delta_micros(), 0);
        assert_eq!(
            s.lines(),
            ["committed daily spend 20.00 EUR/day across 1 enabled campaign"]
        );
    }

    #[test]
    fn an_unknown_currency_still_reports_the_amount() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.b","name":"B","amount_micros":20000000}],
                "campaigns":[{"id":"m.c","name":"C","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.b","manual_cpc":{}}]
            }"#,
            r#"{"customer_id":"1"}"#,
        );
        assert_eq!(
            s.lines(),
            ["committed daily spend 0.00 -> 20.00 (+20.00/day) across 1 enabled campaign"]
        );
    }

    #[test]
    fn a_custom_period_budget_is_not_folded_into_the_daily_figure() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[
                  {"id":"m.d","name":"Daily","amount_micros":20000000},
                  {"id":"m.t","name":"Flight","total_amount_micros":91000000,
                   "period":"CUSTOM_PERIOD"}
                ],
                "campaigns":[
                  {"id":"m.a","name":"A","status":"ENABLED",
                   "advertising_channel_type":"SEARCH","campaign_budget":"m.d","manual_cpc":{}},
                  {"id":"m.z","name":"Z","status":"ENABLED",
                   "advertising_channel_type":"SEARCH","campaign_budget":"m.t","manual_cpc":{}}
                ]
            }"#,
            NO_LIVE,
        );
        assert_eq!(s.after_micros, 20_000_000);
        assert_eq!(s.custom_after_micros, 91_000_000);
        assert_eq!(s.custom_budgets_after, 1);
        assert_eq!(
            s.lines(),
            [
                "committed daily spend 0.00 EUR -> 20.00 EUR (+20.00 EUR/day) \
                 across 2 enabled campaigns",
                "plus 1 custom-period budget totalling 0.00 EUR -> 91.00 EUR over their lifetime",
            ]
        );
    }

    #[test]
    fn a_custom_period_budget_on_a_paused_campaign_commits_nothing() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.t","name":"Flight",
                  "total_amount_micros":91000000,"period":"CUSTOM_PERIOD"}],
                "campaigns":[{"id":"m.z","name":"Z","status":"PAUSED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.t","manual_cpc":{}}]
            }"#,
            NO_LIVE,
        );
        assert_eq!(s.custom_budgets_after, 0);
        assert!(s.lines().is_empty());
    }

    #[test]
    fn a_lifetime_only_account_reports_the_lifetime_line_alone() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.t","name":"Flight",
                  "total_amount_micros":150000000,"period":"CUSTOM_PERIOD"}],
                "campaigns":[{"id":"m.z","name":"Z","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.t","manual_cpc":{}}]
            }"#,
            NO_LIVE,
        );
        assert_eq!(
            s.lines(),
            ["1 custom-period budget totalling 0.00 EUR -> 150.00 EUR over their lifetime"]
        );
    }

    #[test]
    fn raising_a_lifetime_cap_reads_as_a_total_not_a_rate() {
        let s = summary(
            r#"{
                "customer_id":"1",
                "campaign_budgets":[{"id":"m.t","name":"Flight",
                  "total_amount_micros":150000000,"period":"CUSTOM_PERIOD"}],
                "campaigns":[{"id":"m.z","name":"Flight","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"m.t","manual_cpc":{}}]
            }"#,
            r#"{
                "customer_id":"1","currency_code":"EUR",
                "campaign_budgets":[{"id":"900","name":"Flight",
                  "total_amount_micros":91000000,"period":"CUSTOM_PERIOD"}],
                "campaigns":[{"id":"500","name":"Flight","status":"ENABLED",
                  "advertising_channel_type":"SEARCH","campaign_budget":"900"}]
            }"#,
        );
        assert_eq!(s.before_micros, 0);
        assert_eq!(s.after_micros, 0);
        assert_eq!(
            s.lines(),
            ["1 custom-period budget totalling 91.00 EUR -> 150.00 EUR over their lifetime"]
        );
    }
}
