mod api;
mod commands;
mod diagnostics;
mod lint;
mod parser;
mod program;
mod schema;

use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "bidsmith", version, about = "Declarative tooling for Google Ads")]
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
        Cmd::Fmt { path, check } => commands::fmt::run(&path, check),
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
