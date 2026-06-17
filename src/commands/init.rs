use std::fs;
use std::path::Path;
use std::process::ExitCode;

const BIDSMITH_TOML: &str = include_str!("../../templates/init/bidsmith.toml.tmpl");
const GITIGNORE: &str = include_str!("../../templates/init/gitignore.tmpl");
const WORKFLOW: &str = include_str!("../../templates/init/workflow.yml.tmpl");
const STARTER_BID: &str = include_str!("../../templates/init/starter.bid.tmpl");
const README: &str = include_str!("../../templates/init/README.md.tmpl");

struct Scaffold {
    relative: &'static str,
    contents: &'static str,
}

const FILES: &[Scaffold] = &[
    Scaffold { relative: "bidsmith.toml", contents: BIDSMITH_TOML },
    Scaffold { relative: ".gitignore", contents: GITIGNORE },
    Scaffold { relative: ".github/workflows/bidsmith.yml", contents: WORKFLOW },
    Scaffold { relative: "campaigns.bid", contents: STARTER_BID },
    Scaffold { relative: "README.md", contents: README },
];

pub fn run(path: &str, force: bool) -> ExitCode {
    let root = Path::new(path);

    let mut wrote = 0usize;
    let mut skipped = 0usize;
    for file in FILES {
        let dest = root.join(file.relative);
        if dest.exists() && !force {
            eprintln!(
                "init: {} already exists — skipped (pass --force to overwrite).",
                dest.display(),
            );
            skipped += 1;
            continue;
        }
        if let Some(parent) = dest.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("init: failed to create {}: {e}", parent.display());
                return ExitCode::from(1);
            }
        }
        if let Err(e) = fs::write(&dest, file.contents) {
            eprintln!("init: failed to write {}: {e}", dest.display());
            return ExitCode::from(1);
        }
        eprintln!("init: wrote {}", dest.display());
        wrote += 1;
    }

    eprintln!();
    eprintln!("Scaffolded a bidsmith GitOps project ({wrote} written, {skipped} skipped).");
    eprintln!("Next steps:");
    eprintln!("  1. Set customer_id + login_customer_id in bidsmith.toml.");
    eprintln!("  2. Edit campaigns.bid, then run `bidsmith validate`.");
    eprintln!("  3. `bidsmith auth login`, then `bidsmith plan` to preview against live.");
    eprintln!("  4. Push to GitHub and add the GOOGLE_ADS_* secrets (see README.md)");
    eprintln!("     to turn on plan-on-PR and apply-on-merge.");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_paths_are_distinct() {
        let mut seen = std::collections::HashSet::new();
        for f in FILES {
            assert!(seen.insert(f.relative), "duplicate scaffold path: {}", f.relative);
            assert!(!f.contents.is_empty(), "empty template for {}", f.relative);
        }
    }

    #[test]
    fn project_config_template_parses_as_toml() {
        let parsed: Result<toml::Value, _> = toml::from_str(BIDSMITH_TOML);
        assert!(parsed.is_ok(), "bidsmith.toml template is not valid TOML: {parsed:?}");
    }
}
