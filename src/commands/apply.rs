use std::process::ExitCode;

use crate::commands::plan;

pub fn run(path: Option<&str>, confirm: bool, verbose: bool) -> ExitCode {
    let Some(path) = path else {
        eprintln!("apply: provide a .bid file or directory.");
        return ExitCode::from(2);
    };

    if !confirm {
        // Dry run: same as plan with apply branding + footer.
        eprintln!("apply: dry run (no --confirm) — running validateOnly against the live API.");
        let code = plan::run_apply(path, /* validate_only */ true, verbose);
        eprintln!();
        eprintln!("apply: re-run with --confirm to mutate the account.");
        return code;
    }

    eprintln!(
        "apply: --confirm set — mutating Google Ads. There is no undo from bidsmith."
    );
    plan::run_apply(path, /* validate_only */ false, verbose)
}
