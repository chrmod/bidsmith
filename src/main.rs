mod api;
mod commands;
mod diagnostics;
mod lint;
mod parser;
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
        /// Dump the outgoing request body and raw API response
        #[arg(long)]
        verbose: bool,
    },
    /// Reconcile the live account with the .bid files
    Apply {
        /// .bid file or directory to apply
        path: Option<String>,
        /// Skip the interactive confirmation prompt and apply immediately
        /// after the validateOnly plan is shown. Required for non-TTY runs.
        #[arg(long = "auto-approve")]
        auto_approve: bool,
        /// Dump the outgoing request body and raw API response
        #[arg(long)]
        verbose: bool,
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
    Refresh,
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Validate { path } => commands::validate::run(&path),
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
        Cmd::Plan { path, whoami, read_live, verbose } => {
            commands::plan::run(path.as_deref(), whoami, read_live, verbose)
        }
        Cmd::Apply { path, auto_approve, verbose } => {
            commands::apply::run(path.as_deref(), auto_approve, verbose)
        }
        Cmd::Pull { output, verbose } => {
            commands::pull::run(output.as_deref(), verbose)
        }
        Cmd::Refresh => commands::stub(
            "refresh",
            "Will pull live Google Ads state into .bid files.",
        ),
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
