use std::path::Path;
use std::process::ExitCode;

use crate::api::live_state::CacheMode;
use crate::api::{auth, client, live_state};
use crate::commands::export::{
    canonicalize, filter_removed, render_split, ExportInput,
};

pub fn run(
    output: Option<&str>,
    dir: Option<&str>,
    include_removed: bool,
    verbose: bool,
) -> ExitCode {
    if output.is_some() && dir.is_some() {
        eprintln!("refresh: --output and --dir are mutually exclusive");
        return ExitCode::from(2);
    }

    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("refresh: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::get_access_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("refresh: {e}");
            return ExitCode::from(1);
        }
    };

    if verbose {
        eprintln!(
            "refresh: customers/{} via /{}/googleAds:searchStream",
            client.customer_id,
            client::api_version(),
        );
    }

    let outcome = match live_state::fetch_with_cache(
        &client,
        &token.token,
        CacheMode::ReadWrite,
        "refresh",
    ) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("refresh: live-state fetch failed: {e}");
            return ExitCode::from(1);
        }
    };
    let mut input: ExportInput = outcome.state;
    if !include_removed {
        filter_removed(&mut input);
    }

    let (account_raw, campaigns_raw) = render_split(&input);
    let account = canonicalize(&account_raw);
    let campaigns = canonicalize(&campaigns_raw);

    match (output, dir) {
        (Some(path), None) => write_single(path, &account, &campaigns),
        (None, Some(d)) => write_split(d, &account, &campaigns, verbose),
        (None, None) => {
            print!("{}", account);
            if !campaigns.is_empty() {
                if !account.is_empty() && !account.ends_with("\n\n") {
                    println!();
                }
                print!("{}", campaigns);
            }
            ExitCode::SUCCESS
        }
        (Some(_), Some(_)) => unreachable!(),
    }
}

fn write_single(path: &str, account: &str, campaigns: &str) -> ExitCode {
    let mut combined = String::new();
    combined.push_str(account);
    if !campaigns.is_empty() {
        if !account.is_empty() && !account.ends_with("\n\n") {
            combined.push('\n');
        }
        combined.push_str(campaigns);
    }
    match std::fs::write(path, &combined) {
        Ok(()) => {
            eprintln!("refresh: wrote {path}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("refresh: failed to write {path}: {e}");
            ExitCode::from(1)
        }
    }
}

fn write_split(dir: &str, account: &str, campaigns: &str, verbose: bool) -> ExitCode {
    let dir_path = Path::new(dir);
    if let Err(e) = std::fs::create_dir_all(dir_path) {
        eprintln!("refresh: failed to create {dir}: {e}");
        return ExitCode::from(1);
    }

    let account_path = dir_path.join("account.bid");
    if let Err(e) = std::fs::write(&account_path, account) {
        eprintln!(
            "refresh: failed to write {}: {e}",
            account_path.display()
        );
        return ExitCode::from(1);
    }
    if verbose {
        eprintln!("refresh: wrote {}", account_path.display());
    }

    if campaigns.is_empty() {
        eprintln!(
            "refresh: wrote {} (no campaign-scoped resources to write)",
            account_path.display()
        );
        return ExitCode::SUCCESS;
    }

    let campaigns_path = dir_path.join("campaigns.bid");
    if let Err(e) = std::fs::write(&campaigns_path, campaigns) {
        eprintln!(
            "refresh: failed to write {}: {e}",
            campaigns_path.display()
        );
        return ExitCode::from(1);
    }
    eprintln!(
        "refresh: wrote {} and {}",
        account_path.display(),
        campaigns_path.display()
    );
    ExitCode::SUCCESS
}
