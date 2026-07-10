mod api;
mod commands;
mod diagnostics;
mod eval;
mod expand;
mod lint;
mod parser;
mod program;
mod schema;
mod targeting;

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "bidsmith",
    version,
    about = "Declarative tooling for Google Ads",
    after_help = "\
The authoring loop:
  edit .bid files -> bidsmith validate -> bidsmith plan -> bidsmith apply

Discovery:
  bidsmith <command> --help   examples and flag details per command
  bidsmith schema             every resource type and attribute, as JSON
  bidsmith query --help       read-only stats and reports (GAQL)

Docs: https://chrmod.github.io/bidsmith/"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scaffold a GitOps project: bidsmith.toml, a starter .bid, a GitHub
    /// Actions workflow (plan on PRs, apply on merge), .gitignore, README
    #[command(after_help = "\
init writes the skeleton for managing a Google Ads account as code in a
GitHub repository: a starter campaigns.bid, a bidsmith.toml for the
account ids, a .github/workflows/bidsmith.yml that runs `plan` on every
pull request and `apply` on merge to main, plus a .gitignore and README.

Existing files are left untouched unless you pass --force.

Examples:
  bidsmith init               # scaffold into the current directory
  bidsmith init ./my-account  # scaffold into a new directory")]
    Init {
        /// Directory to scaffold into (created if missing)
        #[arg(default_value = ".")]
        path: String,
        /// Overwrite files that already exist
        #[arg(long)]
        force: bool,
    },
    /// Rewrite .bid files in canonical format
    Fmt {
        /// File or directory to format
        #[arg(default_value = ".")]
        path: String,
        /// Don't write; exit non-zero if any file would change
        #[arg(long)]
        check: bool,
        /// Also strip optional attributes whose value equals the schema default
        /// (the form `refresh` / `export` emit). Compliance fields stay.
        #[arg(long)]
        minimal: bool,
    },
    /// Rename a resource's address across .bid files (block + every reference)
    #[command(after_help = "\
mv rewrites source only: it renames the resource block and every reference
to it. Because bidsmith matches live resources by their content (campaign
name, keyword, ...), an address rename is invisible to the account — no
delete + create, no lost performance history or ad review. plan stays a
no-op afterward.

Use it to clean up refresh-generated names (the `_2` / `_7` dedupe suffixes)
without touching the live campaign.

Addresses are `<type>.<name>`, or `<module>.<type>.<name>` to disambiguate
a name shared across files.

Bulk: pass --from-file to rename many at once. The file lists one
`<from> <to>` pair per line (or `<from> -> <to>`); blank lines and lines
starting with '#' are ignored. The whole batch is applied atomically —
if any rule is invalid (missing source, occupied target, a rename
chain), nothing is written. Use '-' to read the pairs from stdin.

Examples:
  bidsmith mv google_ads_ad_group_ad.reklama_1_7 google_ads_ad_group_ad.preroll_ad
  bidsmith mv singapore_12sec.google_ads_ad_group.niestandardowa_wideo_2023_05_24_5 \\
              singapore_12sec.google_ads_ad_group.instream_12sec
  bidsmith mv google_ads_campaign.old google_ads_campaign.new --path campaigns/
  bidsmith mv --from-file renames.txt          # rename a whole batch")]
    Mv {
        /// Current address: `<type>.<name>` (or `<module>.<type>.<name>`)
        from: Option<String>,
        /// New address: same type, a new name
        to: Option<String>,
        /// Read `<from> <to>` rename pairs from a file (one per line; `-` for stdin)
        #[arg(long = "from-file", value_name = "PATH", conflicts_with_all = ["from", "to"])]
        from_file: Option<String>,
        /// File or directory to rewrite (references can span files)
        #[arg(long, value_name = "PATH", default_value = ".")]
        path: String,
    },
    /// Check .bid syntax, schema, and references
    Validate {
        /// File or directory to validate
        #[arg(default_value = ".")]
        path: String,
        /// Set a variable value (repeatable). Example: `--var city_radius_km=20`.
        /// Overrides any `default` in the matching `variable` block. Values from
        /// `BIDSMITH_VAR_<name>` env vars apply when this flag is not supplied.
        #[arg(long = "var", value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
        var: Vec<String>,
    },
    /// Render a .bid file from a Google Ads campaign source
    #[command(after_help = "\
Examples:
  bidsmith export --from-json campaign.json -o main.bid
  bidsmith export --from-gads-search-response dump.json -o main.bid

dump.json is the shape `bidsmith pull -o dump.json` writes, so
pull + export round-trips a live account into a .bid file.")]
    Export {
        /// Flat bidsmith JSON describing the campaign(s) to render
        #[arg(long = "from-json", value_name = "PATH", conflicts_with = "from_gads_search_response")]
        from_json: Option<String>,
        /// Raw Google Ads SearchStream JSON dump to adapt and render
        #[arg(long = "from-gads-search-response", value_name = "PATH")]
        from_gads_search_response: Option<String>,
        /// Output file path (defaults to stdout)
        #[arg(short = 'o', long, value_name = "PATH")]
        output: Option<String>,
        /// Include resources whose status is REMOVED (default: drop them)
        #[arg(long)]
        include_removed: bool,
        /// Override / set the provider login_customer_id (MCC)
        #[arg(long = "login-customer-id", value_name = "ID")]
        login_customer_id: Option<String>,
        /// Override the provider customer_id
        #[arg(long = "customer-id", value_name = "ID")]
        customer_id: Option<String>,
    },
    /// Show what would change against the live Google Ads account
    #[command(after_help = "\
Examples:
  bidsmith plan .                          # diff against live, server-validated
  bidsmith plan --offline .                # diff against the cache, no API calls
  bidsmith plan --refresh-state .          # force a fresh live-state fetch
  bidsmith plan --format markdown .        # render the diff as a Markdown table (PR comments)
  bidsmith plan --detailed-exitcode .      # exit 2 when the diff is non-empty (CI gating)

Live reads are cached in .bidsmith/cache/ (15-min TTL) to save quota.
plan never modifies the account: its mutate is sent with validateOnly.")]
    Plan {
        /// .bid file or directory to plan
        path: Option<String>,
        /// Exchange the refresh token and print the resulting access token's
        /// expiry, without making any Google Ads API call
        #[arg(long)]
        whoami: bool,
        /// Print a summary of the live state for this customer (no .bid
        /// required, no diff, no mutate). Useful for debugging.
        #[arg(long)]
        read_live: bool,
        /// Ignore any cached live state for this customer and refetch from the
        /// API. The fresh fetch is then written back to the cache.
        #[arg(long = "refresh-state", conflicts_with = "offline")]
        refresh_state: bool,
        /// Diff against the cached live state without contacting Google Ads.
        /// Errors if no fresh cache exists — run `bidsmith pull` first to
        /// warm it. Skips OAuth and the validateOnly mutate too.
        #[arg(long)]
        offline: bool,
        /// Dump the outgoing request body and raw API response
        #[arg(long)]
        verbose: bool,
        /// List every resource, including unchanged ones. By default plan shows
        /// only resources that would be created, updated, or destroyed.
        #[arg(long = "show-unchanged")]
        show_unchanged: bool,
        /// Render the diff as `text` (default, the aligned per-resource listing)
        /// or `markdown` (a table suited to posting as a pull-request comment).
        #[arg(long, value_enum, default_value_t = PlanFormat::Text)]
        format: PlanFormat,
        /// Exit 2 (not 0) when the diff is non-empty, keeping 1 for errors —
        /// like `terraform plan -detailed-exitcode`. Lets CI tell "changes
        /// pending" apart from "plan failed".
        #[arg(long = "detailed-exitcode")]
        detailed_exitcode: bool,
        /// Set a variable value (repeatable). Example: `--var city_radius_km=20`.
        /// Overrides any `default` in the matching `variable` block.
        #[arg(long = "var", value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
        var: Vec<String>,
    },
    /// Reconcile the live account with the .bid files
    #[command(after_help = "\
apply always shows the validateOnly plan first, then asks for a literal
'yes' before mutating. --auto-approve skips the prompt and is required
when stdin is not a TTY. Review the plan before approving.

Examples:
  bidsmith apply .
  bidsmith apply --auto-approve .   # CI / scripted runs")]
    Apply {
        /// .bid file or directory to apply
        path: Option<String>,
        /// Skip the interactive confirmation prompt and apply immediately
        /// after the validateOnly plan is shown. Required for non-TTY runs.
        #[arg(long = "auto-approve")]
        auto_approve: bool,
        /// Ignore any cached live state for this customer and refetch from the
        /// API. The fresh fetch is then written back to the cache.
        #[arg(long = "refresh-state")]
        refresh_state: bool,
        /// Dump the outgoing request body and raw API response
        #[arg(long)]
        verbose: bool,
        /// Set a variable value (repeatable). Example: `--var city_radius_km=20`.
        /// Overrides any `default` in the matching `variable` block.
        #[arg(long = "var", value_name = "NAME=VALUE", action = clap::ArgAction::Append)]
        var: Vec<String>,
    },
    /// Dump live Google Ads state as raw SearchStream JSON
    Pull {
        /// Output file path (defaults to stdout)
        #[arg(short = 'o', long, value_name = "PATH")]
        output: Option<String>,
        /// Print the outgoing request envelope
        #[arg(long)]
        verbose: bool,
    },
    /// Pull live state into .bid files
    #[command(after_help = "\
Examples:
  bidsmith refresh -d ads/            # ads/account.bid + ads/campaigns.bid (bootstrap)
  bidsmith refresh -o main.bid        # everything in one file (bootstrap)
  bidsmith refresh --in-place ads/    # update existing .bid in place (reconcile)
  bidsmith refresh --in-place --check ads/   # preview the reconcile, write nothing

Bootstrap mode (-o / -d) overwrites output files from live. Reconcile
mode (--in-place) edits the .bid files at PATH, updating drifted scalar
fields on resources bidsmith manages and leaving everything else intact.")]
    Refresh {
        /// Reconcile target: existing .bid file or directory (--in-place only)
        #[arg(value_name = "PATH")]
        path: Option<String>,
        /// Update existing .bid files at PATH in place instead of rendering
        /// fresh output. Matches live resources to your files by bidsmith
        /// label and writes back drifted scalar fields.
        #[arg(long = "in-place", conflicts_with_all = ["output", "dir"])]
        in_place: bool,
        /// With --in-place: show what would change without writing.
        #[arg(long, requires = "in_place")]
        check: bool,
        /// Write everything to a single .bid file
        #[arg(short = 'o', long, value_name = "PATH", conflicts_with = "dir")]
        output: Option<String>,
        /// Split account-level and campaign-scoped resources into
        /// <DIR>/account.bid and <DIR>/campaigns.bid
        #[arg(short = 'd', long, value_name = "DIR")]
        dir: Option<String>,
        /// Include resources whose status is REMOVED (default: drop them)
        #[arg(long)]
        include_removed: bool,
        /// Print the outgoing request envelope
        #[arg(long)]
        verbose: bool,
    },
    /// Run a GAQL query against the live Google Ads account (read-only)
    #[command(after_help = r#"Examples:
  # Campaign performance, last 30 days
  bidsmith query "SELECT campaign.name, metrics.clicks, metrics.cost_micros, metrics.conversions FROM campaign WHERE segments.date DURING LAST_30_DAYS"

  # What people searched before seeing an ad
  bidsmith query "SELECT search_term_view.search_term, metrics.clicks FROM search_term_view WHERE segments.date DURING LAST_30_DAYS ORDER BY metrics.clicks DESC LIMIT 50"

  # Per-keyword cost and conversions, as JSON for further analysis
  bidsmith query "SELECT ad_group_criterion.keyword.text, metrics.average_cpc, metrics.conversions FROM keyword_view WHERE segments.date DURING LAST_30_DAYS" --format json

Money fields (cost_micros, average_cpc, ...) are micros: divide by
1,000,000 for the account currency. Date ranges: DURING LAST_7_DAYS /
LAST_30_DAYS / THIS_MONTH, or segments.date BETWEEN '2026-05-01' AND
'2026-05-31'. GAQL is SELECT-only; this command cannot modify the account.

Grammar: https://developers.google.com/google-ads/api/docs/query/overview"#)]
    Query {
        /// GAQL query string (e.g. `SELECT campaign.name FROM campaign`)
        query: String,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryFormat::Table)]
        format: QueryFormat,
        /// Dump the outgoing request and raw API response
        #[arg(long)]
        verbose: bool,
    },
    /// Research keyword ideas from Keyword Planner: search volume,
    /// competition, and bid estimates for seed terms or a landing page
    #[command(name = "keyword-ideas", after_help = "\
keyword-ideas is a read-only research call to Google's Keyword Planner
(KeywordPlanIdeaService). Give it seed keywords, a landing-page --url, or
both, and it returns related keywords with their average monthly searches,
competition, and top-of-page bid estimates. It never touches the account
and reads no .bid files.

Locations and languages take the same human-readable codes as a campaign's
`locations` / `languages` (US, PL, en, de, ...), or raw
geoTargetConstants/NNNN / languageConstants/NNNN strings.

Examples:
  bidsmith keyword-ideas \"running shoes\" \"trail shoes\" --location US --language en
  bidsmith keyword-ideas --url https://example.com/shop --location DE
  bidsmith keyword-ideas \"energy storage\" --location PL --limit 100 --format tsv > ideas.tsv

Search volume and bids reflect the chosen locations + language. Bid
estimates are micros of the account currency (divide by 1,000,000).")]
    KeywordIdeas {
        /// Seed keywords to expand (zero or more; omit only when --url is set)
        #[arg(value_name = "SEED")]
        seeds: Vec<String>,
        /// Landing-page or site URL to derive ideas from (with or without seeds)
        #[arg(long, value_name = "URL")]
        url: Option<String>,
        /// Target location (repeatable): ISO country code like `US`, or a raw
        /// geoTargetConstants/NNNN. Omit for no geo constraint.
        #[arg(long = "location", value_name = "CODE", action = clap::ArgAction::Append)]
        location: Vec<String>,
        /// Language code (e.g. `en`), or a raw languageConstants/NNNN
        #[arg(long, value_name = "CODE", default_value = "en")]
        language: String,
        /// Max ideas to print, most-searched first (0 = all)
        #[arg(long, value_name = "N", default_value_t = 50)]
        limit: usize,
        /// Output format
        #[arg(long, value_enum, default_value_t = QueryFormat::Table)]
        format: QueryFormat,
        /// Dump the outgoing request and raw API response
        #[arg(long)]
        verbose: bool,
    },
    /// Dump the resource schema as JSON (drives doc generation, IDE tooling)
    Schema {
        /// Output file path (defaults to stdout)
        #[arg(short = 'o', long, value_name = "PATH")]
        output: Option<String>,
    },
    /// Sign in to Google Ads and manage saved credentials
    #[command(subcommand)]
    Auth(AuthCmd),
    /// Generate the Google Ads API Basic-Access design document
    #[command(name = "design-doc", subcommand)]
    DesignDoc(DesignDocCmd),
    /// (internal) Remove every resource whose name starts with --prefix.
    /// Used by the e2e test tier; not a public verb.
    #[command(name = "_e2e-cleanup", hide = true)]
    E2eCleanup {
        /// Name prefix to sweep (e.g. `bidsmith-e2e-`)
        #[arg(long, value_name = "STR")]
        prefix: String,
        /// Print every resource_name before removal
        #[arg(long)]
        verbose: bool,
    },
}

#[derive(Copy, Clone, ValueEnum)]
enum QueryFormat {
    Table,
    Json,
    Tsv,
}

#[derive(Copy, Clone, ValueEnum)]
enum PlanFormat {
    Text,
    Markdown,
}

#[derive(Subcommand)]
enum AuthCmd {
    /// Sign in with Google in your browser and save the credentials
    Login {
        /// OAuth client id (defaults to the bundled client, or your agency's)
        #[arg(long = "client-id", value_name = "ID")]
        client_id: Option<String>,
        /// OAuth client secret (required with --client-id for a bring-your-own client)
        #[arg(long = "client-secret", value_name = "SECRET")]
        client_secret: Option<String>,
        /// Developer token from your agency's manager account
        #[arg(long = "developer-token", value_name = "TOKEN")]
        developer_token: Option<String>,
        /// Manager account id (MCC) these calls log in through
        #[arg(long = "login-customer-id", value_name = "ID")]
        login_customer_id: Option<String>,
        /// Don't prompt for missing values (for scripts / non-interactive use)
        #[arg(long = "no-input")]
        no_input: bool,
    },
    /// Show which credentials are configured and verify they work
    Status,
    /// Remove the saved sign-in (keeps the team profile unless --all)
    Logout {
        /// Delete the whole credentials file, including developer token + MCC id
        #[arg(long)]
        all: bool,
    },
    /// Print a shareable team profile (developer token + manager account id)
    Profile {
        /// Also include the OAuth client id/secret in the profile
        #[arg(long = "with-client")]
        with_client: bool,
    },
}

#[derive(Subcommand)]
enum DesignDocCmd {
    /// Write a commented design-doc.toml template you can fill in
    Init {
        /// Output file path
        #[arg(short = 'o', long, value_name = "PATH", default_value = "design-doc.toml")]
        output: String,
        /// Overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Read design-doc.toml + bidsmith internals, write design-doc.html
    Render {
        /// Input TOML config
        #[arg(short = 'c', long, value_name = "PATH", default_value = "design-doc.toml")]
        config: String,
        /// Output HTML file (use `-` for stdout)
        #[arg(short = 'o', long, value_name = "PATH", default_value = "design-doc.html")]
        output: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Init { path, force } => commands::init::run(&path, force),
        Cmd::Validate { path, var } => commands::validate::run(&path, &var),
        Cmd::Export {
            from_json,
            from_gads_search_response,
            output,
            include_removed,
            login_customer_id,
            customer_id,
        } => commands::export::run(
            from_json.as_deref(),
            from_gads_search_response.as_deref(),
            output.as_deref(),
            include_removed,
            login_customer_id.as_deref(),
            customer_id.as_deref(),
        ),
        Cmd::Fmt { path, check, minimal } => commands::fmt::run(&path, check, minimal),
        Cmd::Mv { from, to, from_file, path } => {
            commands::mv::run(from.as_deref(), to.as_deref(), from_file.as_deref(), &path)
        }
        Cmd::Plan { path, whoami, read_live, refresh_state, offline, verbose, show_unchanged, format, detailed_exitcode, var } => {
            let fmt = match format {
                PlanFormat::Text => commands::plan::Format::Text,
                PlanFormat::Markdown => commands::plan::Format::Markdown,
            };
            commands::plan::run(
                path.as_deref(),
                whoami,
                read_live,
                refresh_state,
                offline,
                verbose,
                show_unchanged,
                fmt,
                detailed_exitcode,
                &var,
            )
        }
        Cmd::Apply { path, auto_approve, refresh_state, verbose, var } => commands::apply::run(
            path.as_deref(),
            auto_approve,
            refresh_state,
            verbose,
            &var,
        ),
        Cmd::Pull { output, verbose } => {
            commands::pull::run(output.as_deref(), verbose)
        }
        Cmd::Refresh { path, in_place, check, output, dir, include_removed, verbose } => {
            if in_place {
                commands::refresh::run_reconcile(path.as_deref(), check, verbose)
            } else {
                commands::refresh::run(
                    output.as_deref(),
                    dir.as_deref(),
                    include_removed,
                    verbose,
                )
            }
        }
        Cmd::Schema { output } => commands::schema::run(output.as_deref()),
        Cmd::Auth(sub) => match sub {
            AuthCmd::Login {
                client_id,
                client_secret,
                developer_token,
                login_customer_id,
                no_input,
            } => commands::auth::run_login(
                client_id.as_deref(),
                client_secret.as_deref(),
                developer_token.as_deref(),
                login_customer_id.as_deref(),
                no_input,
            ),
            AuthCmd::Status => commands::auth::run_status(),
            AuthCmd::Logout { all } => commands::auth::run_logout(all),
            AuthCmd::Profile { with_client } => commands::auth::run_profile(with_client),
        },
        Cmd::DesignDoc(sub) => match sub {
            DesignDocCmd::Init { output, force } => {
                commands::design_doc::run_init(&output, force)
            }
            DesignDocCmd::Render { config, output } => {
                commands::design_doc::run_render(&config, &output)
            }
        },
        Cmd::E2eCleanup { prefix, verbose } => {
            commands::e2e_cleanup::run(&prefix, verbose)
        }
        Cmd::Query { query, format, verbose } => {
            let fmt = match format {
                QueryFormat::Table => commands::query::Format::Table,
                QueryFormat::Json => commands::query::Format::Json,
                QueryFormat::Tsv => commands::query::Format::Tsv,
            };
            commands::query::run(&query, fmt, verbose)
        }
        Cmd::KeywordIdeas { seeds, url, location, language, limit, format, verbose } => {
            let fmt = match format {
                QueryFormat::Table => commands::keyword_ideas::Format::Table,
                QueryFormat::Json => commands::keyword_ideas::Format::Json,
                QueryFormat::Tsv => commands::keyword_ideas::Format::Tsv,
            };
            commands::keyword_ideas::run(
                &seeds,
                url.as_deref(),
                &location,
                &language,
                limit,
                fmt,
                verbose,
            )
        }
    }
}
