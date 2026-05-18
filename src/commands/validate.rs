use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::diagnostics::Diag;
use crate::parser::{parse_file, ParsedFile};
use crate::schema::validate_files;

pub fn run(target: &str) -> ExitCode {
    let target = Path::new(target);
    if !target.exists() {
        eprintln!("No such file or directory: {}", target.display());
        return ExitCode::from(1);
    }

    let files: Vec<PathBuf> = if target.is_file() {
        vec![target.to_path_buf()]
    } else {
        match collect_bid_files(target) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        }
    };

    if files.is_empty() {
        eprintln!("No .bid files found under {}", target.display());
        return ExitCode::from(1);
    }

    let mut parsed: Vec<ParsedFile> = Vec::new();
    let mut diags: Vec<Diag> = Vec::new();

    for f in &files {
        match parse_file(f) {
            Ok(pf) => parsed.push(pf),
            Err(d) => diags.push(d),
        }
    }

    diags.extend(validate_files(&parsed));

    if diags.is_empty() {
        println!("OK: {} file(s) valid.", parsed.len());
        return ExitCode::SUCCESS;
    }

    let total = diags.len();
    for d in diags {
        let report = miette::Report::new(d);
        eprintln!("{report:?}");
    }
    eprintln!(
        "{} error{} in {} file{}.",
        total,
        if total == 1 { "" } else { "s" },
        files.len(),
        if files.len() == 1 { "" } else { "s" }
    );
    ExitCode::from(1)
}

fn collect_bid_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    walk(dir, &mut out).map_err(|e| format!("failed to walk {}: {e}", dir.display()))?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
            continue;
        }
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&path, out)?;
        } else if ft.is_file() && path.extension().and_then(|e| e.to_str()) == Some("bid") {
            out.push(path);
        }
    }
    Ok(())
}
