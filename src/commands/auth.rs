use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use crate::api::{auth, client, creds, oauth};

pub fn run_login(
    client_id: Option<&str>,
    client_secret: Option<&str>,
    developer_token: Option<&str>,
    login_customer_id: Option<&str>,
    no_input: bool,
) -> ExitCode {
    let stored = creds::StoredCreds::load();

    let cid = client_id
        .map(str::to_string)
        .or_else(|| creds::env_nonempty("GOOGLE_ADS_CLIENT_ID"))
        .or_else(|| stored.client_id.clone())
        .or_else(creds::default_client_id);
    let csecret = client_secret
        .map(str::to_string)
        .or_else(|| creds::env_nonempty("GOOGLE_ADS_CLIENT_SECRET"))
        .or_else(|| stored.client_secret.clone())
        .or_else(|| {
            if cid == creds::default_client_id() {
                creds::default_client_secret()
            } else {
                None
            }
        });

    let (cid, csecret) = match (cid, csecret) {
        (Some(id), Some(secret)) => (id, secret),
        _ => {
            eprintln!("auth login: no OAuth client to sign in with.");
            eprintln!("  This build has no bundled client, so pass your agency's instead:");
            eprintln!("    bidsmith auth login --client-id <ID> --client-secret <SECRET>");
            eprintln!("  Create one in Google Cloud Console as an OAuth \"Desktop app\" client.");
            return ExitCode::from(2);
        }
    };

    let tokens = match oauth::authorize(&cid, &csecret) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("auth login: {e}");
            return ExitCode::from(1);
        }
    };
    println!("✓ Signed in to Google.");

    let interactive = !no_input && std::io::stdin().is_terminal();
    let developer = developer_token
        .map(str::to_string)
        .or_else(|| stored.developer_token.clone())
        .or_else(|| creds::env_nonempty("GOOGLE_ADS_DEVELOPER_TOKEN"))
        .or_else(|| {
            interactive
                .then(|| prompt("Developer token (your agency MCC's API Center)"))
                .flatten()
        });
    let login = login_customer_id
        .map(str::to_string)
        .or_else(|| stored.login_customer_id.clone())
        .or_else(|| creds::env_nonempty("GOOGLE_ADS_LOGIN_CUSTOMER_ID"))
        .or_else(|| {
            interactive
                .then(|| prompt("Manager account ID (the MCC, digits only e.g. 1234567890)"))
                .flatten()
        })
        .map(digits_only);

    let is_default = Some(&cid) == creds::default_client_id().as_ref();
    let new = creds::StoredCreds {
        client_id: Some(cid),
        client_secret: if is_default { None } else { Some(csecret) },
        refresh_token: Some(tokens.refresh_token),
        developer_token: developer.filter(|s| !s.is_empty()),
        login_customer_id: login.filter(|s| !s.is_empty()),
        customer_id: stored.customer_id.clone(),
    };
    if let Err(e) = new.save() {
        eprintln!("auth login: could not write {}: {e}", creds::credentials_path().display());
        return ExitCode::from(1);
    }
    println!("✓ Saved to {}", creds::credentials_path().display());

    match new.developer_token.as_deref() {
        Some(dev) => match client::list_accessible_customers(dev, &tokens.access_token) {
            Ok(ids) if !ids.is_empty() => {
                println!("\nYou can manage these {} account(s):", ids.len());
                for id in &ids {
                    println!("  {}", format_cid(id));
                }
                println!("\nDrop the one you want into a project's provider block:");
                println!("  provider \"google_ads\" {{ customer_id = \"{}\" }}", ids[0]);
            }
            Ok(_) => println!("\nNo accounts are directly accessible to this login yet."),
            Err(e) => eprintln!("\nSigned in, but listing accounts failed: {e}"),
        },
        None => {
            println!("\nNo developer token yet — plan/apply/query need one.");
            println!("Add it later: bidsmith auth login --developer-token <token> --login-customer-id <mcc>");
        }
    }

    ExitCode::SUCCESS
}

pub fn run_status() -> ExitCode {
    let r = creds::Resolved::load();
    println!("Credentials (env var > {} > built-in):", creds::credentials_path().display());
    print_field("client_id", r.client_id().as_deref(), false);
    print_field("client_secret", r.client_secret().as_deref(), true);
    print_field("refresh_token", r.refresh_token().as_deref(), true);
    print_field("developer_token", r.developer_token().as_deref(), true);
    print_field("login_customer_id", r.login_customer_id().as_deref(), false);
    print_field("customer_id", r.customer_id().as_deref(), false);

    if let Some(msg) = r.client_mismatch() {
        eprintln!("\n! {msg}");
        return ExitCode::from(1);
    }

    let token = match auth::get_access_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("\n! not signed in: {e}");
            eprintln!("  Run `bidsmith auth login`.");
            return ExitCode::from(1);
        }
    };
    println!("\n✓ Google sign-in works (access token valid for {}s).", token.expires_in);

    let Some(dev) = r.developer_token() else {
        eprintln!("! no developer token — set one with `bidsmith auth login --developer-token <token>`.");
        return ExitCode::from(1);
    };
    match client::list_accessible_customers(&dev, &token.token) {
        Ok(ids) if !ids.is_empty() => {
            println!("✓ {} account(s) reachable:", ids.len());
            for id in &ids {
                println!("  {}", format_cid(id));
            }
            ExitCode::SUCCESS
        }
        Ok(_) => {
            println!("✓ API reachable, but no accounts are directly accessible to this login.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("! account check failed: {e}");
            ExitCode::from(1)
        }
    }
}

pub fn run_logout(all: bool) -> ExitCode {
    let path = creds::credentials_path();
    if all {
        return match std::fs::remove_file(&path) {
            Ok(_) => {
                println!("Removed {}.", path.display());
                ExitCode::SUCCESS
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("Nothing to remove ({} does not exist).", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("auth logout: {e}");
                ExitCode::from(1)
            }
        };
    }

    let mut stored = creds::StoredCreds::load();
    if stored.refresh_token.is_none() {
        println!("Already signed out (no saved sign-in).");
        return ExitCode::SUCCESS;
    }
    stored.refresh_token = None;
    stored.client_id = None;
    stored.client_secret = None;
    match stored.save() {
        Ok(_) => {
            println!("Signed out. Kept the developer token + manager account so the next login is one step.");
            println!("Use `bidsmith auth logout --all` to wipe everything.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("auth logout: {e}");
            ExitCode::from(1)
        }
    }
}

pub fn run_profile(with_client: bool) -> ExitCode {
    let stored = creds::StoredCreds::load();
    if stored.developer_token.is_none() && stored.login_customer_id.is_none() {
        eprintln!("auth profile: nothing to share yet — run `bidsmith auth login` first.");
        return ExitCode::from(1);
    }

    println!("# Bidsmith team profile — CONTAINS A SECRET (developer token).");
    println!("# Share over a trusted channel only (password manager, not email/Slack-public).");
    println!();
    let mut cmd = String::from("bidsmith auth login");
    if let Some(d) = &stored.developer_token {
        cmd.push_str(&format!(" --developer-token {d}"));
    }
    if let Some(l) = &stored.login_customer_id {
        cmd.push_str(&format!(" --login-customer-id {l}"));
    }
    if with_client {
        if let Some(c) = &stored.client_id {
            cmd.push_str(&format!(" --client-id {c}"));
        }
        if let Some(s) = &stored.client_secret {
            cmd.push_str(&format!(" --client-secret {s}"));
        }
    }
    println!("{cmd}");
    println!();
    println!("# A teammate runs that, finishes the browser sign-in, and is ready.");
    ExitCode::SUCCESS
}

fn prompt(label: &str) -> Option<String> {
    print!("{label}\n  > ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line).ok()? == 0 {
        return None;
    }
    let value = line.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn print_field(name: &str, value: Option<&str>, secret: bool) {
    match value {
        Some(v) if secret => println!("  {name:<18}: set ({}…, {} chars)", head(v, 6), v.len()),
        Some(v) => println!("  {name:<18}: {v}"),
        None => println!("  {name:<18}: (not set)"),
    }
}

fn digits_only(s: String) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

fn format_cid(id: &str) -> String {
    if id.len() == 10 && id.bytes().all(|b| b.is_ascii_digit()) {
        format!("{}-{}-{}", &id[0..3], &id[3..6], &id[6..10])
    } else {
        id.to_string()
    }
}

fn head(s: &str, n: usize) -> &str {
    &s[..s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())]
}
