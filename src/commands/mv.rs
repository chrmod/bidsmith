use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hcl_edit::Ident;
use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Block, BlockLabel};

use crate::diagnostics::Diag;
use crate::parser::{parse_file, parse_str, ParsedFile};
use crate::program::collect_bid_files;
use crate::schema::{validate_files, InputBindings, ResourceRegistry, Resolution};

struct Addr {
    module: Option<String>,
    ty: String,
    name: String,
}

fn parse_addr(s: &str) -> Result<Addr, String> {
    let parts: Vec<&str> = s.split('.').collect();
    match parts.as_slice() {
        [ty, name] => Ok(Addr {
            module: None,
            ty: (*ty).to_string(),
            name: (*name).to_string(),
        }),
        [m, ty, name] => Ok(Addr {
            module: Some((*m).to_string()),
            ty: (*ty).to_string(),
            name: (*name).to_string(),
        }),
        _ => Err(format!(
            "'{s}' is not a resource address; expected '<type>.<name>' (optionally '<module>.<type>.<name>')"
        )),
    }
}

// A resolved rename: an existing resource (`from_qualified`) gets a new name,
// staying within its own module. Both halves are resolved against the original
// state, so a whole batch applies atomically against one snapshot.
struct Rename {
    from_qualified: String,
    to_qualified: String,
    to_name: String,
    module: String,
    ty: String,
}

pub fn run(
    from: Option<&str>,
    to: Option<&str>,
    from_file: Option<&str>,
    path: &str,
) -> ExitCode {
    let pairs: Vec<(String, String)> = match (from, to, from_file) {
        (Some(f), Some(t), None) => vec![(f.to_string(), t.to_string())],
        (None, None, Some(file)) => match read_pairs(file) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("mv: {e}");
                return ExitCode::from(2);
            }
        },
        (None, None, None) => {
            eprintln!("mv: nothing to do — give '<from> <to>', or --from-file <path>");
            return ExitCode::from(2);
        }
        (_, _, Some(_)) => {
            eprintln!("mv: use either positional '<from> <to>' or --from-file, not both");
            return ExitCode::from(2);
        }
        _ => {
            eprintln!("mv: both <from> and <to> are required");
            return ExitCode::from(2);
        }
    };
    if pairs.is_empty() {
        println!("mv: no renames to apply.");
        return ExitCode::SUCCESS;
    }

    let target = Path::new(path);
    if !target.exists() {
        eprintln!("mv: no such file or directory: {}", target.display());
        return ExitCode::from(1);
    }
    let paths = match collect_bid_files(target) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("mv: {e}");
            return ExitCode::from(1);
        }
    };
    if paths.is_empty() {
        eprintln!("mv: no .bid files found under {}", target.display());
        return ExitCode::from(1);
    }

    let mut files: Vec<ParsedFile> = Vec::new();
    for p in &paths {
        match parse_file(p) {
            Ok(pf) => files.push(pf),
            Err(d) => {
                eprintln!("{:?}", miette::Report::new(d));
                return ExitCode::from(1);
            }
        }
    }

    let (registry, reg_diags) = ResourceRegistry::build(&files);
    if reg_diags.iter().any(|d| d.is_error()) {
        for d in reg_diags {
            eprintln!("{:?}", miette::Report::new(d));
        }
        eprintln!("mv: fix duplicate-resource errors before moving");
        return ExitCode::from(1);
    }

    let plan = match build_plan(&pairs, &registry) {
        Ok(p) => p,
        Err(errs) => {
            for e in &errs {
                eprintln!("mv: {e}");
            }
            return ExitCode::from(1);
        }
    };
    if plan.is_empty() {
        println!("mv: nothing to do (every rename target already matches its source).");
        return ExitCode::SUCCESS;
    }

    let renames: HashMap<String, String> = plan
        .iter()
        .map(|r| (r.from_qualified.clone(), r.to_name.clone()))
        .collect();

    // Only a newly introduced error blocks the batch; pre-existing, unrelated
    // errors don't block a cleanup pass.
    let baseline_errors = error_signatures(&validate_files(&files, &InputBindings::default()));

    let (block_count, ref_count, changed) = apply_renames(&mut files, &registry, &renames);

    if block_count != renames.len() {
        eprintln!(
            "mv: internal error — expected to rename {} block(s) but found {block_count}; nothing was written",
            renames.len()
        );
        return ExitCode::from(1);
    }

    let rendered: Vec<(PathBuf, String)> = changed
        .iter()
        .map(|&i| (files[i].path.clone(), files[i].body.to_string()))
        .collect();

    if let Err(code) = revalidate(&files, &baseline_errors) {
        return code;
    }

    for (path, content) in &rendered {
        if let Err(e) = std::fs::write(path, content) {
            eprintln!("mv: failed to write {}: {e}", path.display());
            return ExitCode::from(1);
        }
    }

    report(&plan, block_count, ref_count, rendered.len());
    ExitCode::SUCCESS
}

fn build_plan(pairs: &[(String, String)], reg: &ResourceRegistry) -> Result<Vec<Rename>, Vec<String>> {
    let mut errors: Vec<String> = Vec::new();
    let mut plan: Vec<Rename> = Vec::new();
    let mut seen_from: HashSet<String> = HashSet::new();
    let mut seen_to: HashSet<String> = HashSet::new();

    for (from_s, to_s) in pairs {
        let from_addr = match parse_addr(from_s) {
            Ok(a) => a,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let to_addr = match parse_addr(to_s) {
            Ok(a) => a,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        if from_addr.ty == "ad_template" || to_addr.ty == "ad_template" {
            errors.push(format!(
                "'{from_s}' -> '{to_s}': mv does not support ad_template blocks; rename them by hand and update each 'template = ad_template.<name>' reference"
            ));
            continue;
        }
        if from_addr.ty != to_addr.ty {
            errors.push(format!(
                "'{from_s}' -> '{to_s}': cannot change resource type ('{}' -> '{}')",
                from_addr.ty, to_addr.ty
            ));
            continue;
        }
        if Ident::try_new(to_addr.name.as_str()).is_err() {
            errors.push(format!(
                "'{}' is not a valid resource name (letters, digits, '-' or '_', not starting with a digit)",
                to_addr.name
            ));
            continue;
        }
        let from_qualified = match resolve_from(reg, &from_addr) {
            Ok(q) => q,
            Err(e) => {
                errors.push(e);
                continue;
            }
        };
        let qp: Vec<&str> = from_qualified.split('.').collect();
        let (module, from_name) = (qp[0].to_string(), qp[2].to_string());
        if let Some(m) = &to_addr.module {
            if m != &module {
                errors.push(format!(
                    "'{from_s}' -> '{to_s}': target module '{m}' differs from source module '{module}'; cross-module moves aren't supported"
                ));
                continue;
            }
        }
        if to_addr.name == from_name {
            continue;
        }
        let to_qualified = ResourceRegistry::qualified(&module, &from_addr.ty, &to_addr.name);
        if !seen_from.insert(from_qualified.clone()) {
            errors.push(format!("'{from_qualified}' is renamed by more than one rule"));
            continue;
        }
        if !seen_to.insert(to_qualified.clone()) {
            errors.push(format!("two rules both rename to '{to_qualified}'"));
            continue;
        }
        plan.push(Rename {
            from_qualified,
            to_qualified,
            to_name: to_addr.name.clone(),
            module,
            ty: from_addr.ty.clone(),
        });
    }

    let from_set: HashSet<&str> = plan.iter().map(|r| r.from_qualified.as_str()).collect();
    for r in &plan {
        if from_set.contains(r.to_qualified.as_str()) {
            errors.push(format!(
                "'{}' targets '{}', which is itself being renamed — rename chains aren't followed in one pass; split them into separate runs",
                r.from_qualified, r.to_qualified
            ));
        } else if reg.declared(&r.module, &r.ty, &r.to_name) {
            errors.push(format!(
                "target '{}' already exists; pick a free name or move it first",
                r.to_qualified
            ));
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(plan)
}

fn apply_renames(
    files: &mut [ParsedFile],
    reg: &ResourceRegistry,
    renames: &HashMap<String, String>,
) -> (usize, usize, Vec<usize>) {
    let mut block_count = 0usize;
    let mut ref_count = 0usize;
    let mut changed: Vec<usize> = Vec::new();

    for (i, f) in files.iter_mut().enumerate() {
        let module = f.module.clone();
        let mut file_changed = false;

        for mut s in f.body.iter_mut() {
            if let Some(b) = s.as_block_mut() {
                if b.ident.as_str() == "resource" && b.labels.len() == 2 {
                    let q = ResourceRegistry::qualified(
                        &module,
                        b.labels[0].as_str(),
                        b.labels[1].as_str(),
                    );
                    if let Some(to_name) = renames.get(&q) {
                        set_label(&mut b.labels[1], to_name);
                        block_count += 1;
                        file_changed = true;
                    }
                }
                ref_count += rewrite_block(b, reg, &module, renames, &mut file_changed);
            } else if let Some(mut a) = s.as_attribute_mut() {
                ref_count += rewrite_expr(a.value_mut(), reg, &module, renames, &mut file_changed);
            }
        }

        if file_changed {
            changed.push(i);
        }
    }

    (block_count, ref_count, changed)
}

fn resolve_from(reg: &ResourceRegistry, a: &Addr) -> Result<String, String> {
    match &a.module {
        Some(m) => {
            if reg.declared(m, &a.ty, &a.name) {
                Ok(ResourceRegistry::qualified(m, &a.ty, &a.name))
            } else {
                Err(format!("no resource '{m}.{}.{}' found", a.ty, a.name))
            }
        }
        // "\0" is a sentinel that can never be a real (slugified) module name,
        // so this falls through to the global short-name lookup.
        None => match reg.resolve("\0", &a.ty, &a.name) {
            Resolution::Found(q) => Ok(q),
            Resolution::Missing => Err(format!("no resource '{}.{}' found", a.ty, a.name)),
            Resolution::Ambiguous(mods) => {
                let mut s: Vec<&str> = mods.iter().map(String::as_str).collect();
                s.sort();
                Err(format!(
                    "'{}.{}' is declared in multiple modules [{}]; qualify it as '<module>.{}.{}'",
                    a.ty,
                    a.name,
                    s.join(", "),
                    a.ty,
                    a.name
                ))
            }
        },
    }
}

fn rewrite_block(
    b: &mut Block,
    reg: &ResourceRegistry,
    module: &str,
    renames: &HashMap<String, String>,
    file_changed: &mut bool,
) -> usize {
    let mut n = 0;
    for mut s in b.body.iter_mut() {
        if let Some(inner) = s.as_block_mut() {
            n += rewrite_block(inner, reg, module, renames, file_changed);
        } else if let Some(mut a) = s.as_attribute_mut() {
            n += rewrite_expr(a.value_mut(), reg, module, renames, file_changed);
        }
    }
    n
}

fn rewrite_expr(
    e: &mut Expression,
    reg: &ResourceRegistry,
    module: &str,
    renames: &HashMap<String, String>,
    file_changed: &mut bool,
) -> usize {
    match e {
        Expression::Traversal(t) => {
            if let Some((rty, rname)) = traversal_ref(t) {
                if let Resolution::Found(q) = reg.resolve(module, &rty, &rname) {
                    if let Some(to_name) = renames.get(&q) {
                        set_traversal_name(t, to_name);
                        *file_changed = true;
                        return 1;
                    }
                }
            }
            0
        }
        Expression::Array(arr) => {
            let mut n = 0;
            for item in arr.iter_mut() {
                n += rewrite_expr(item, reg, module, renames, file_changed);
            }
            n
        }
        Expression::Object(obj) => {
            let mut n = 0;
            for (_k, v) in obj.iter_mut() {
                n += rewrite_expr(v.expr_mut(), reg, module, renames, file_changed);
            }
            n
        }
        _ => 0,
    }
}

fn traversal_ref(t: &Traversal) -> Option<(String, String)> {
    let Expression::Variable(v) = &t.expr else {
        return None;
    };
    let ty = v.as_str().to_string();
    let TraversalOperator::GetAttr(name) = &**t.operators.first()? else {
        return None;
    };
    Some((ty, name.as_str().to_string()))
}

fn set_traversal_name(t: &mut Traversal, to_name: &str) {
    if let Some(op) = t.operators.first_mut() {
        if let TraversalOperator::GetAttr(ident) = op.value_mut() {
            *ident.value_mut() = Ident::new(to_name);
        }
    }
}

fn set_label(label: &mut BlockLabel, to_name: &str) {
    match label {
        BlockLabel::String(s) => *s.value_mut() = to_name.to_string(),
        BlockLabel::Ident(id) => *id.value_mut() = Ident::new(to_name),
    }
}

fn revalidate(
    files: &[ParsedFile],
    baseline: &HashMap<(String, String), usize>,
) -> Result<(), ExitCode> {
    // `files` is already mutated in place, so re-serializing each body reflects
    // the rename. Re-parse from those strings and run the full validator so a
    // rename that breaks a reference (or introduces an ambiguity) aborts before
    // anything is written.
    let mut reparsed: Vec<ParsedFile> = Vec::with_capacity(files.len());
    for f in files {
        let content = f.body.to_string();
        match parse_str(&f.path, &content) {
            Ok(pf) => reparsed.push(pf),
            Err(d) => {
                eprintln!("{:?}", miette::Report::new(d));
                eprintln!("mv: the rename would produce an unparseable file; nothing was written");
                return Err(ExitCode::from(1));
            }
        }
    }

    // Re-serializing shifts span offsets, so compare errors by (file, message),
    // not position; only a higher count for a signature is newly introduced.
    let errors = validate_files(&reparsed, &InputBindings::default());
    let after = error_signatures(&errors);
    let regressed = after
        .iter()
        .any(|(sig, &n)| n > baseline.get(sig).copied().unwrap_or(0));
    if regressed {
        let mut seen: HashMap<(String, String), usize> = HashMap::new();
        for d in errors.into_iter().filter(|d| d.is_error()) {
            let sig = (d.src.name().to_string(), d.message.clone());
            let allowed = baseline.get(&sig).copied().unwrap_or(0);
            let count = seen.entry(sig).or_insert(0);
            *count += 1;
            if *count > allowed {
                eprintln!("{:?}", miette::Report::new(d));
            }
        }
        eprintln!("mv: the rename would break the project; nothing was written");
        return Err(ExitCode::from(1));
    }
    Ok(())
}

fn error_signatures(diags: &[Diag]) -> HashMap<(String, String), usize> {
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for d in diags.iter().filter(|d| d.is_error()) {
        *counts
            .entry((d.src.name().to_string(), d.message.clone()))
            .or_insert(0) += 1;
    }
    counts
}

fn report(plan: &[Rename], blocks: usize, refs: usize, files: usize) {
    if plan.len() == 1 {
        let r = &plan[0];
        println!(
            "Renamed {} -> {}: {blocks} block + {refs} reference{} across {files} file{}.",
            r.from_qualified,
            r.to_qualified,
            if refs == 1 { "" } else { "s" },
            if files == 1 { "" } else { "s" },
        );
    } else {
        for r in plan {
            println!("  {} -> {}", r.from_qualified, r.to_qualified);
        }
        println!(
            "Renamed {} resources: {blocks} block{} + {refs} reference{} across {files} file{}.",
            plan.len(),
            if blocks == 1 { "" } else { "s" },
            if refs == 1 { "" } else { "s" },
            if files == 1 { "" } else { "s" },
        );
    }
    println!(
        "This is an address-only change: it does not delete or recreate the live resource. Run `bidsmith plan` to confirm a no-op."
    );
}

fn read_pairs(file: &str) -> Result<Vec<(String, String)>, String> {
    let content = if file == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        s
    } else {
        std::fs::read_to_string(file).map_err(|e| format!("failed to read {file}: {e}"))?
    };
    parse_pairs(&content)
}

fn parse_pairs(content: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (i, raw) in content.lines().enumerate() {
        // Addresses never contain '#', so anything from the first '#' is a
        // comment — handles both whole-line and trailing comments.
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let normalized = line.replace("->", " ");
        let toks: Vec<&str> = normalized.split_whitespace().collect();
        if toks.len() != 2 {
            return Err(format!(
                "line {}: expected '<from> <to>' (or '<from> -> <to>'), got '{line}'",
                i + 1
            ));
        }
        out.push((toks[0].to_string(), toks[1].to_string()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ok(code: ExitCode) -> bool {
        format!("{code:?}") == format!("{:?}", ExitCode::SUCCESS)
    }

    fn one(from: &str, to: &str, path: &str) -> ExitCode {
        run(Some(from), Some(to), None, path)
    }

    fn workdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bidsmith-mv-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    const PROVIDER: &str = "provider \"google_ads\" {\n  customer_id = \"1234567890\"\n}\n\n";

    fn campaign(name: &str) -> String {
        format!(
            r#"resource "google_ads_campaign_budget" "budget" {{
  name = "Budget"
  amount_micros = 10000000
  delivery_method = "STANDARD"
}}

resource "google_ads_campaign" "{name}" {{
  name = "Search"
  status = "ENABLED"
  advertising_channel_type = "SEARCH"
  campaign_budget = google_ads_campaign_budget.budget.id
}}
"#
        )
    }

    fn sample() -> String {
        format!(
            r#"{PROVIDER}{campaign}
# Singapore 12s preroll
resource "google_ads_ad_group" "old_group" {{
  name     = "Custom video"
  campaign = google_ads_campaign.search.id
  status   = "ENABLED"
}}

resource "google_ads_ad_group_ad" "reklama_1_7" {{
  ad_group = google_ads_ad_group.old_group.id # parent ad group
  status   = "ENABLED"

  ad {{
    name       = "Reklama #1"
    final_urls = ["https://example.com/"]
  }}
}}
"#,
            campaign = campaign("search"),
        )
    }

    #[test]
    fn renames_block_and_references_preserving_format() {
        let dir = workdir("rename");
        let file = dir.join("main.bid");
        fs::write(&file, sample()).unwrap();

        let code = one(
            "google_ads_ad_group.old_group",
            "google_ads_ad_group.preroll",
            dir.to_str().unwrap(),
        );
        assert!(ok(code));

        let out = fs::read_to_string(&file).unwrap();
        assert!(out.contains(r#"resource "google_ads_ad_group" "preroll""#), "{out}");
        assert!(out.contains("ad_group = google_ads_ad_group.preroll.id"), "{out}");
        assert!(!out.contains("old_group"), "{out}");
        assert!(out.contains("# Singapore 12s preroll"), "{out}");
        assert!(out.contains("# parent ad group"), "{out}");
        assert!(out.contains("campaign = google_ads_campaign.search.id"), "{out}");
        assert!(out.contains(r#"name       = "Reklama #1""#), "{out}");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rewrites_references_across_files() {
        let dir = workdir("crossfile");
        fs::write(dir.join("account.bid"), format!("{PROVIDER}{}", campaign("search"))).unwrap();
        let ads = dir.join("ads.bid");
        fs::write(
            &ads,
            r#"resource "google_ads_ad_group" "grp" {
  name = "Default"
  campaign = google_ads_campaign.search.id
  status = "ENABLED"
}
"#,
        )
        .unwrap();

        let code = one(
            "google_ads_campaign.search",
            "google_ads_campaign.brand_search",
            dir.to_str().unwrap(),
        );
        assert!(ok(code));

        let ads_out = fs::read_to_string(&ads).unwrap();
        assert!(
            ads_out.contains("campaign = google_ads_campaign.brand_search.id"),
            "{ads_out}"
        );
        let acct_out = fs::read_to_string(dir.join("account.bid")).unwrap();
        assert!(acct_out.contains(r#"resource "google_ads_campaign" "brand_search""#), "{acct_out}");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn identical_address_is_a_noop() {
        let dir = workdir("identical");
        let file = dir.join("main.bid");
        fs::write(&file, sample()).unwrap();
        let before = fs::read_to_string(&file).unwrap();

        let code = one(
            "google_ads_ad_group_ad.reklama_1_7",
            "google_ads_ad_group_ad.reklama_1_7",
            dir.to_str().unwrap(),
        );
        assert!(ok(code));
        assert_eq!(before, fs::read_to_string(&file).unwrap());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_occupied_target() {
        let dir = workdir("occupied");
        let file = dir.join("main.bid");
        fs::write(
            &file,
            format!(
                r#"{PROVIDER}{}
resource "google_ads_ad_group" "grp_a" {{
  name = "A"
  campaign = google_ads_campaign.search.id
  status = "ENABLED"
}}

resource "google_ads_ad_group" "grp_b" {{
  name = "B"
  campaign = google_ads_campaign.search.id
  status = "ENABLED"
}}
"#,
                campaign("search"),
            ),
        )
        .unwrap();
        let before = fs::read_to_string(&file).unwrap();

        let code = one(
            "google_ads_ad_group.grp_a",
            "google_ads_ad_group.grp_b",
            dir.to_str().unwrap(),
        );
        assert!(!ok(code));
        assert_eq!(before, fs::read_to_string(&file).unwrap());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_type_change() {
        let dir = workdir("typechange");
        fs::write(dir.join("main.bid"), sample()).unwrap();
        let code = one(
            "google_ads_ad_group.old_group",
            "google_ads_campaign.old_group",
            dir.to_str().unwrap(),
        );
        assert!(!ok(code));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_source_errors() {
        let dir = workdir("missing");
        fs::write(dir.join("main.bid"), sample()).unwrap();
        let code = one(
            "google_ads_ad_group.nope",
            "google_ads_ad_group.whatever",
            dir.to_str().unwrap(),
        );
        assert!(!ok(code));
        fs::remove_dir_all(&dir).unwrap();
    }

    // ---- bulk mode --------------------------------------------------------

    #[test]
    fn bulk_renames_many_in_one_pass() {
        let dir = workdir("bulk");
        let file = dir.join("main.bid");
        fs::write(&file, sample()).unwrap();
        let pairs = dir.join("renames.txt");
        fs::write(
            &pairs,
            "# clean up machine names\n\
             google_ads_ad_group.old_group -> google_ads_ad_group.preroll\n\
             google_ads_ad_group_ad.reklama_1_7  google_ads_ad_group_ad.preroll_ad\n",
        )
        .unwrap();

        let code = run(None, None, Some(pairs.to_str().unwrap()), dir.to_str().unwrap());
        assert!(ok(code));

        let out = fs::read_to_string(&file).unwrap();
        assert!(out.contains(r#"resource "google_ads_ad_group" "preroll""#), "{out}");
        assert!(out.contains(r#"resource "google_ads_ad_group_ad" "preroll_ad""#), "{out}");
        // the cross-resource reference followed the ad group's rename
        assert!(out.contains("ad_group = google_ads_ad_group.preroll.id"), "{out}");
        assert!(!out.contains("old_group"), "{out}");
        assert!(!out.contains("reklama_1_7"), "{out}");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bulk_is_all_or_nothing() {
        let dir = workdir("bulk-atomic");
        let file = dir.join("main.bid");
        fs::write(&file, sample()).unwrap();
        let before = fs::read_to_string(&file).unwrap();
        let pairs = dir.join("renames.txt");
        // first rule is fine; second names a resource that doesn't exist
        fs::write(
            &pairs,
            "google_ads_ad_group.old_group google_ads_ad_group.preroll\n\
             google_ads_ad_group.does_not_exist google_ads_ad_group.whatever\n",
        )
        .unwrap();

        let code = run(None, None, Some(pairs.to_str().unwrap()), dir.to_str().unwrap());
        assert!(!ok(code));
        // nothing written — the good rename was rolled back with the bad one
        assert_eq!(before, fs::read_to_string(&file).unwrap());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bulk_rejects_chains() {
        let dir = workdir("bulk-chain");
        let file = dir.join("main.bid");
        fs::write(
            &file,
            format!(
                r#"{PROVIDER}{}
resource "google_ads_ad_group" "a" {{
  name = "A"
  campaign = google_ads_campaign.search.id
  status = "ENABLED"
}}

resource "google_ads_ad_group" "b" {{
  name = "B"
  campaign = google_ads_campaign.search.id
  status = "ENABLED"
}}
"#,
                campaign("search"),
            ),
        )
        .unwrap();
        let before = fs::read_to_string(&file).unwrap();
        let pairs = dir.join("renames.txt");
        // a -> b while b -> c is a chain; reject the whole batch
        fs::write(
            &pairs,
            "google_ads_ad_group.a google_ads_ad_group.b\n\
             google_ads_ad_group.b google_ads_ad_group.c\n",
        )
        .unwrap();

        let code = run(None, None, Some(pairs.to_str().unwrap()), dir.to_str().unwrap());
        assert!(!ok(code));
        assert_eq!(before, fs::read_to_string(&file).unwrap());

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parse_pairs_handles_comments_blanks_and_arrows() {
        let parsed =
            parse_pairs("# header\n\na.b c.d\n\n  e.f -> g.h  # trailing comment\n").unwrap();
        assert_eq!(parsed, vec![
            ("a.b".to_string(), "c.d".to_string()),
            ("e.f".to_string(), "g.h".to_string()),
        ]);

        // a malformed line (3 tokens) reports its line number
        let err = parse_pairs("a.b c.d\nx y z\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn pre_existing_error_does_not_block_unrelated_rename() {
        // A dangling reference is a pre-existing error. A clean, unrelated rename
        // must still succeed — and the byte-offset shift it causes must not make
        // that pre-existing error read as newly introduced.
        let dir = workdir("preexisting");
        let file = dir.join("main.bid");
        fs::write(
            &file,
            format!(
                r#"{PROVIDER}{}
resource "google_ads_ad_group" "old_group" {{
  name = "G"
  campaign = google_ads_campaign.search.id
  status = "ENABLED"
}}

resource "google_ads_ad_group" "h" {{
  name = "H"
  campaign = google_ads_campaign.ghost.id
  status = "ENABLED"
}}
"#,
                campaign("search"),
            ),
        )
        .unwrap();

        let code = one(
            "google_ads_ad_group.old_group",
            "google_ads_ad_group.preroll",
            dir.to_str().unwrap(),
        );
        assert!(ok(code), "clean rename should survive a pre-existing error");

        let out = fs::read_to_string(&file).unwrap();
        assert!(out.contains(r#"resource "google_ads_ad_group" "preroll""#), "{out}");
        assert!(
            out.contains("google_ads_campaign.ghost.id"),
            "pre-existing dangling reference left intact: {out}"
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_ad_template_rename_with_clear_message() {
        let dir = workdir("adtemplate");
        let file = dir.join("main.bid");
        fs::write(
            &file,
            format!(
                r#"{PROVIDER}ad_template "tmpl" {{
  final_urls = ["https://example.com/"]
  responsive_search_ad {{
    headlines = ["One", "Two", "Three"]
    descriptions = ["Description one", "Description two"]
  }}
}}
"#
            ),
        )
        .unwrap();

        let parsed = vec![parse_file(&file).expect("parses")];
        let (reg, _diags) = ResourceRegistry::build(&parsed);
        let err = match build_plan(
            &[(
                "ad_template.tmpl".to_string(),
                "ad_template.renamed".to_string(),
            )],
            &reg,
        ) {
            Err(e) => e,
            Ok(_) => panic!("ad_template rename should be rejected"),
        };
        assert!(
            err.iter().any(|e| e.contains("ad_template")),
            "expected a clear ad_template message, got {err:?}"
        );

        fs::remove_dir_all(&dir).unwrap();
    }
}
