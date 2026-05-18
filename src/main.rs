mod commands;
mod diagnostics;
mod parser;
mod schema;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bidsmith", version, about = "Declarative tooling for Google Ads")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Rewrite .bid files in canonical format
    Fmt,
    /// Check .bid syntax, schema, and references
    Validate {
        /// File or directory to validate
        #[arg(default_value = ".")]
        path: String,
    },
    /// Render a .bid file from a Google Ads campaign source
    Export {
        /// JSON file describing the campaign(s) to render
        #[arg(long = "from-json", value_name = "PATH")]
        from_json: String,
        /// Output file path (defaults to stdout)
        #[arg(short = 'o', long, value_name = "PATH")]
        output: Option<String>,
    },
    /// Show what would change against the live Google Ads account
    Plan,
    /// Reconcile the live account with the .bid files
    Apply,
    /// Pull live state into .bid files
    Refresh,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Cmd::Validate { path } => commands::validate::run(&path),
        Cmd::Export { from_json, output } => {
            commands::export::run(&from_json, output.as_deref())
        }
        Cmd::Fmt => commands::stub(
            "fmt",
            "Will rewrite .bid files in canonical HCL2 style.",
        ),
        Cmd::Plan => commands::stub(
            "plan",
            "Will diff .bid files against the live Google Ads account using validate-only mutates.",
        ),
        Cmd::Apply => commands::stub(
            "apply",
            "Will reconcile the live Google Ads account with the .bid files.",
        ),
        Cmd::Refresh => commands::stub(
            "refresh",
            "Will pull live Google Ads state into .bid files.",
        ),
    }
}
