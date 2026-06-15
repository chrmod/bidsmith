use std::path::Path;
use std::process::ExitCode;

use crate::commands::vars;
use crate::diagnostics::Diag;
use crate::program::{collect_bid_files, Program};
use crate::schema::validate_files;

pub fn run(target: &str, cli_vars: &[String]) -> ExitCode {
    let inputs = match vars::collect(cli_vars) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("validate: {e}");
            return ExitCode::from(2);
        }
    };
    let target = Path::new(target);
    if !target.exists() {
        eprintln!("No such file or directory: {}", target.display());
        return ExitCode::from(1);
    }

    let files = match collect_bid_files(target) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
    };

    if files.is_empty() {
        eprintln!("No .bid files found under {}", target.display());
        return ExitCode::from(1);
    }

    let top_input_count = files.len();
    let loaded = Program::load(&files, inputs);
    let program = loaded.program;
    let mut diags: Vec<Diag> = loaded.diagnostics;
    for scope in &program.scopes {
        diags.extend(validate_files(&scope.files, &scope.inputs));
    }
    for scope in &program.scopes {
        diags.extend(crate::lint::lint_files(&scope.files, &scope.inputs));
    }

    diags.sort_by(|a, b| {
        (a.src.name(), a.span.offset()).cmp(&(b.src.name(), b.span.offset()))
    });

    let file_count: usize = program.scopes.iter().map(|s| s.files.len()).sum();
    let errors = diags.iter().filter(|d| d.is_error()).count();
    let warnings = diags.len() - errors;

    if diags.is_empty() {
        println!("OK: {} file(s) valid.", file_count);
        return ExitCode::SUCCESS;
    }

    for d in diags {
        let report = miette::Report::new(d);
        eprintln!("{report:?}");
    }

    if errors == 0 {
        println!(
            "OK: {} file(s) valid ({} warning{}).",
            file_count,
            warnings,
            if warnings == 1 { "" } else { "s" }
        );
        return ExitCode::SUCCESS;
    }

    eprintln!(
        "{} error{}, {} warning{} in {} file{}.",
        errors,
        if errors == 1 { "" } else { "s" },
        warnings,
        if warnings == 1 { "" } else { "s" },
        top_input_count,
        if top_input_count == 1 { "" } else { "s" }
    );
    ExitCode::from(1)
}
