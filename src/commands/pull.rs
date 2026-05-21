use std::process::ExitCode;

use serde_json::Value;

use crate::api::{auth, client, live_state};

pub fn run(output: Option<&str>, verbose: bool) -> ExitCode {
    let client = match client::Client::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pull: {e}");
            return ExitCode::from(1);
        }
    };
    let token = match auth::get_access_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("pull: {e}");
            return ExitCode::from(1);
        }
    };

    if verbose {
        eprintln!(
            "pull: customers/{} via /{}/googleAds:searchStream",
            client.customer_id,
            client::api_version(),
        );
    } else {
        eprintln!(
            "pull: fetching live state from customers/{}...",
            client.customer_id,
        );
    }

    let batches = match live_state::fetch_raw(&client, &token.token) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("pull: live-state fetch failed: {e}");
            return ExitCode::from(1);
        }
    };

    let payload = Value::Array(batches);
    let rendered = match serde_json::to_string_pretty(&payload) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("pull: failed to serialise response batches: {e}");
            return ExitCode::from(1);
        }
    };

    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(path, format!("{rendered}\n")) {
                eprintln!("pull: failed to write {path}: {e}");
                return ExitCode::from(1);
            }
            eprintln!("pull: wrote {path}");
        }
        None => {
            println!("{rendered}");
        }
    }

    ExitCode::SUCCESS
}
