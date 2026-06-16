use std::io::{BufRead, IsTerminal, Write};
use std::process::ExitCode;

use crate::api::live_state;
use crate::commands::{plan, vars};

pub fn run(
    path: Option<&str>,
    auto_approve: bool,
    refresh_state: bool,
    verbose: bool,
    cli_vars: &[String],
) -> ExitCode {
    let Some(path) = path else {
        eprintln!("apply: provide a .bid file or directory.");
        return ExitCode::from(2);
    };

    let inputs = match vars::collect(cli_vars) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("apply: {e}");
            return ExitCode::from(2);
        }
    };

    let prepared = match plan::prepare(path, "apply", refresh_state, /* offline */ false, &inputs) {
        Ok(Some(p)) => p,
        Ok(None) => return ExitCode::SUCCESS,
        Err(code) => return code,
    };

    if !plan::has_pending_changes(&prepared) {
        let code = plan::execute(
            &prepared,
            /* validate_only */ true,
            verbose,
            /* show_unchanged */ false,
            plan::DisplayMode::PerResource,
        );
        eprintln!();
        eprintln!("apply: no changes. Account already matches the .bid.");
        return code;
    }

    let plan_code = plan::execute(
        &prepared,
        /* validate_only */ true,
        verbose,
        /* show_unchanged */ false,
        plan::DisplayMode::PerResource,
    );
    if plan_code != ExitCode::SUCCESS {
        eprintln!();
        eprintln!("apply: validateOnly rejected changes — refusing to mutate.");
        return plan_code;
    }

    if !auto_approve {
        if !std::io::stdin().is_terminal() {
            eprintln!();
            eprintln!(
                "apply: refusing to prompt (stdin is not a TTY). \
                 Re-run with --auto-approve to apply non-interactively."
            );
            return ExitCode::from(2);
        }
        match prompt_for_yes() {
            Ok(true) => {}
            Ok(false) => {
                println!();
                println!("apply: cancelled.");
                return ExitCode::SUCCESS;
            }
            Err(e) => {
                eprintln!("apply: failed to read confirmation: {e}");
                return ExitCode::from(1);
            }
        }
    }

    eprintln!();
    eprintln!("apply: mutating Google Ads (no undo from bidsmith)...");
    let code = plan::execute(
        &prepared,
        /* validate_only */ false,
        verbose,
        /* show_unchanged */ false,
        plan::DisplayMode::Summary,
    );
    live_state::invalidate_cache();
    code
}

fn prompt_for_yes() -> std::io::Result<bool> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out)?;
    writeln!(out, "Apply these changes?")?;
    writeln!(out, "  bidsmith will perform the actions described above.")?;
    writeln!(out, "  Only 'yes' will be accepted to approve.")?;
    writeln!(out)?;
    write!(out, "  Enter a value: ")?;
    out.flush()?;

    let stdin = std::io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(line.trim() == "yes")
}
