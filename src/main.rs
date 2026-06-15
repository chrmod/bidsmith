mod api;
mod commands;
mod diagnostics;
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
  bidsmith plan .                  # diff against live, server-validated
  bidsmith plan --offline .        # diff against the cache, no API calls
  bidsmith plan --refresh-state .  # force a fresh live-state fetch

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
  bidsmith refresh -d ads/       # ads/account.bid + ads/campaigns.bid
  bidsmith refresh -o main.bid   # everything in one file

Bootstrap mode: existing output files are overwritten, not merged.")]
    Refresh {
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
        Cmd::Plan { path, whoami, read_live, refresh_state, offline, verbose, var } => {
            commands::plan::run(
                path.as_deref(),
                whoami,
                read_live,
                refresh_state,
                offline,
                verbose,
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
        Cmd::Refresh { output, dir, include_removed, verbose } => commands::refresh::run(
            output.as_deref(),
            dir.as_deref(),
            include_removed,
            verbose,
        ),
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
    }
}
