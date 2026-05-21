//! Live round-trip e2e test against a dedicated Google Ads test manager
//! account. Opt-in via `cargo test --features e2e`; requires
//! `BIDSMITH_E2E_CUSTOMER_ID` to be set (plus the usual
//! `GOOGLE_ADS_DEVELOPER_TOKEN` / `_CLIENT_ID` / `_CLIENT_SECRET` /
//! `_REFRESH_TOKEN`). The test forces `GOOGLE_ADS_CUSTOMER_ID` to the
//! test value for every subprocess, so a developer's normal account
//! env can't be hit by accident.

#![cfg(feature = "e2e")]

use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const BINARY: &str = env!("CARGO_BIN_EXE_bidsmith");
const FIXTURE: &str = include_str!("fixtures/e2e.bid");

#[test]
fn live_round_trip() {
    let test_customer_id = std::env::var("BIDSMITH_E2E_CUSTOMER_ID").expect(
        "BIDSMITH_E2E_CUSTOMER_ID must be set to a dedicated Google Ads test manager account",
    );

    // Disable the project-folder cache for every bidsmith subprocess so the
    // round-trip exercises the live API on each step instead of replaying a
    // previous run's snapshot.
    std::env::set_var("BIDSMITH_NO_CACHE", "1");

    let run_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before unix epoch")
        .as_secs();
    let prefix = format!("bidsmith-e2e-{run_id}-");
    eprintln!("e2e: run prefix = {prefix}");

    let tmp = TempDir::new(run_id);
    let fixture_path = tmp.path.join("e2e.bid");
    let dump_path = tmp.path.join("dump.json");
    let roundtrip_path = tmp.path.join("roundtrip.bid");

    let rewritten = FIXTURE.replace("__PREFIX__", &prefix);
    std::fs::write(&fixture_path, rewritten).expect("write fixture");

    let cust = test_customer_id.clone();

    let _guard = CleanupGuard {
        prefix: prefix.clone(),
        customer_id: cust.clone(),
    };

    run_or_panic(
        "_e2e-cleanup (preflight)",
        Command::new(BINARY)
            .args(["_e2e-cleanup", "--prefix", &prefix])
            .env("GOOGLE_ADS_CUSTOMER_ID", &cust),
    );

    run_or_panic(
        "apply --auto-approve",
        Command::new(BINARY)
            .args(["apply", "--auto-approve", fixture_path.to_str().unwrap()])
            .env("GOOGLE_ADS_CUSTOMER_ID", &cust),
    );

    run_or_panic(
        "pull",
        Command::new(BINARY)
            .args(["pull", "-o", dump_path.to_str().unwrap()])
            .env("GOOGLE_ADS_CUSTOMER_ID", &cust),
    );

    run_or_panic(
        "export --from-gads-search-response",
        Command::new(BINARY).args([
            "export",
            "--from-gads-search-response",
            dump_path.to_str().unwrap(),
            "-o",
            roundtrip_path.to_str().unwrap(),
            "--customer-id",
            &cust,
        ]),
    );

    run_or_panic(
        "fmt --check",
        Command::new(BINARY).args(["fmt", "--check", roundtrip_path.to_str().unwrap()]),
    );

    let plan_output = run_or_panic(
        "plan (roundtrip)",
        Command::new(BINARY)
            .args(["plan", roundtrip_path.to_str().unwrap()])
            .env("GOOGLE_ADS_CUSTOMER_ID", &cust),
    );
    let stdout = String::from_utf8_lossy(&plan_output.stdout);
    assert!(
        stdout.contains("0 to create, 0 to update"),
        "plan was not a no-op after round-trip. stdout:\n{stdout}",
    );
}

fn run_or_panic(label: &str, cmd: &mut Command) -> Output {
    eprintln!("e2e: $ bidsmith {label}");
    let output = cmd.output().expect("failed to spawn bidsmith");
    if !output.status.success() {
        panic!(
            "e2e: `{label}` failed (exit {:?})\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }
    output
}

struct CleanupGuard {
    prefix: String,
    customer_id: String,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        eprintln!("e2e: teardown sweep for prefix {}", self.prefix);
        let _ = Command::new(BINARY)
            .args(["_e2e-cleanup", "--prefix", &self.prefix])
            .env("GOOGLE_ADS_CUSTOMER_ID", &self.customer_id)
            .status();
    }
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(run_id: u64) -> Self {
        let dir = std::env::temp_dir().join(format!("bidsmith-e2e-{run_id}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        Self { path: dir }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
