use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hcl_edit::Span;
use hcl_edit::expr::{Expression, ObjectKey};
use hcl_edit::structure::{Block, Structure};
use miette::NamedSource;

use crate::diagnostics::Diag;
use crate::parser::{parse_file, InheritedDefaults, ParsedFile};
use crate::schema::{Bindings, InputBindings};

pub struct Scope {
    pub files: Vec<ParsedFile>,
    pub inputs: InputBindings,
}

pub struct Program {
    pub scopes: Vec<Scope>,
}

impl Program {
    pub fn load(paths: &[PathBuf], cli_inputs: InputBindings) -> Loaded {
        let mut diags: Vec<Diag> = Vec::new();
        let mut top_files: Vec<ParsedFile> = Vec::new();
        for p in paths {
            match parse_file(p) {
                Ok(pf) => top_files.push(pf),
                Err(d) => diags.push(d),
            }
        }

        let mut seen_labels: HashSet<String> = HashSet::new();
        let mut raw_modules: Vec<RawModule> = Vec::new();
        for file in &top_files {
            for s in file.body.iter() {
                let Structure::Block(b) = s else { continue };
                if b.ident.as_str() != "module" {
                    continue;
                }
                match validate_block_shape(file, b, &mut seen_labels) {
                    Ok(raw) => raw_modules.push(raw),
                    Err(ds) => diags.extend(ds),
                }
            }
        }

        // A module source is a template, not a root file — evaluating it at the
        // top level would fail (its variables have no values there).
        let source_paths: HashSet<PathBuf> = raw_modules
            .iter()
            .map(|r| canonical(&r.source_path))
            .collect();
        let root_files: Vec<ParsedFile> = top_files
            .into_iter()
            .filter(|f| !source_paths.contains(&canonical(&f.path)))
            .collect();

        let (top_bindings, _bind_diags) = Bindings::build(&root_files, &cli_inputs);
        let shared_defaults = collect_defaults(&root_files);

        let mut scopes = vec![Scope {
            files: root_files,
            inputs: cli_inputs.clone(),
        }];

        let mut seen_instances: HashSet<String> = HashSet::new();
        let mut module_specs: Vec<ModuleSpec> = Vec::new();
        for raw in &raw_modules {
            match expand_module(raw, &top_bindings, &mut seen_instances) {
                Ok(specs) => module_specs.extend(specs),
                Err(ds) => diags.extend(ds),
            }
        }

        for spec in &module_specs {
            match load_module(spec, &top_bindings, &shared_defaults) {
                Ok(scope) => scopes.push(scope),
                Err(ds) => diags.extend(ds),
            }
        }

        Loaded {
            program: Program { scopes },
            diagnostics: diags,
        }
    }
}

pub struct Loaded {
    pub program: Program,
    pub diagnostics: Vec<Diag>,
}

struct ModuleSpec {
    instance: String,
    caller_module: String,
    src: Arc<NamedSource<String>>,
    span: std::ops::Range<usize>,
    source_path: PathBuf,
    inputs: Vec<(String, Expression, std::ops::Range<usize>)>,
}

/// A `module` block as parsed, before `for_each` expansion into N `ModuleSpec`s.
struct RawModule {
    instance: String,
    caller_module: String,
    src: Arc<NamedSource<String>>,
    source_span: std::ops::Range<usize>,
    source_path: PathBuf,
    shared_inputs: Vec<(String, Expression, std::ops::Range<usize>)>,
    for_each: Option<(Expression, std::ops::Range<usize>)>,
}

fn validate_block_shape(
    file: &ParsedFile,
    block: &Block,
    seen_labels: &mut HashSet<String>,
) -> Result<RawModule, Vec<Diag>> {
    let mut diags = Vec::new();
    if block.labels.len() != 1 {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "'module' block requires exactly one label (the instance name), got {}",
                block.labels.len()
            ),
        ));
        return Err(diags);
    }
    let instance = block.labels[0].as_str().to_string();
    if !seen_labels.insert(instance.clone()) {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(block.labels[0].span()),
            format!("duplicate module instance '{instance}'"),
        ));
        return Err(diags);
    }

    let mut source: Option<String> = None;
    let mut source_span: Option<std::ops::Range<usize>> = None;
    let mut shared_inputs: Vec<(String, Expression, std::ops::Range<usize>)> = Vec::new();
    let mut for_each: Option<(Expression, std::ops::Range<usize>)> = None;
    let mut seen_keys: HashSet<String> = HashSet::new();

    for inner in block.body.iter() {
        match inner {
            Structure::Attribute(a) => {
                let key = a.key.as_str().to_string();
                if !seen_keys.insert(key.clone()) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        span_of(a.key.span()),
                        format!("duplicate attribute '{key}' in module '{instance}'"),
                    ));
                    continue;
                }
                if key == "source" {
                    match &a.value {
                        Expression::String(s) => {
                            source = Some(s.as_str().to_string());
                            source_span = Some(span_of(a.value.span()));
                        }
                        other => {
                            diags.push(Diag::new(
                                file.src.clone(),
                                span_of(a.value.span()),
                                format!(
                                    "module 'source' must be a string path, got {}",
                                    describe_expr_brief(other)
                                ),
                            ));
                        }
                    }
                } else if key == "for_each" {
                    for_each = Some((a.value.clone(), span_of(a.value.span())));
                } else {
                    shared_inputs.push((key, a.value.clone(), span_of(a.value.span())));
                }
            }
            Structure::Block(inner_block) => {
                diags.push(Diag::new(
                    file.src.clone(),
                    span_of(inner_block.ident.span()),
                    format!(
                        "nested block '{}' is not allowed inside 'module' — module only takes attributes",
                        inner_block.ident.as_str()
                    ),
                ));
            }
        }
    }

    let Some(source) = source else {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(block.ident.span()),
            format!("module '{instance}' is missing required attribute 'source'"),
        ));
        return Err(diags);
    };

    let caller_dir = file
        .path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let source_path = caller_dir.join(&source);

    if !diags.is_empty() {
        return Err(diags);
    }

    Ok(RawModule {
        instance,
        caller_module: file.module.clone(),
        src: file.src.clone(),
        source_span: source_span.unwrap_or_else(|| span_of(block.ident.span())),
        source_path,
        shared_inputs,
        for_each,
    })
}

/// Turn a `RawModule` into instances: one named after the label without
/// `for_each`, else one per map entry named `<label>.<key>`.
fn expand_module(
    raw: &RawModule,
    top_bindings: &Bindings,
    seen_instances: &mut HashSet<String>,
) -> Result<Vec<ModuleSpec>, Vec<Diag>> {
    let Some((for_each_expr, for_each_span)) = &raw.for_each else {
        let instance = raw.instance.clone();
        if !seen_instances.insert(instance.clone()) {
            return Err(vec![Diag::new(
                raw.src.clone(),
                raw.source_span.clone(),
                format!("duplicate module instance '{instance}'"),
            )]);
        }
        return Ok(vec![ModuleSpec {
            instance,
            caller_module: raw.caller_module.clone(),
            src: raw.src.clone(),
            span: raw.source_span.clone(),
            source_path: raw.source_path.clone(),
            inputs: raw.shared_inputs.clone(),
        }]);
    };

    let resolved = top_bindings.resolve_value(&raw.caller_module, for_each_expr);
    let Expression::Object(obj) = resolved.as_ref() else {
        return Err(vec![Diag::new(
            raw.src.clone(),
            for_each_span.clone(),
            format!(
                "module '{}' for_each must be an object (a map of instance keys to input objects), got {}",
                raw.instance,
                describe_expr_brief(resolved.as_ref())
            ),
        )]);
    };
    if obj.is_empty() {
        return Err(vec![Diag::new(
            raw.src.clone(),
            for_each_span.clone(),
            format!(
                "module '{}' for_each map is empty; declare at least one instance or remove for_each",
                raw.instance
            ),
        )]);
    }

    let mut diags = Vec::new();
    let mut specs = Vec::new();
    let mut seen_keys: HashSet<String> = HashSet::new();
    for (key, val) in obj.iter() {
        let Some(key_str) = object_key_str(key) else {
            diags.push(Diag::new(
                raw.src.clone(),
                for_each_span.clone(),
                format!(
                    "module '{}' for_each keys must be identifiers or strings",
                    raw.instance
                ),
            ));
            continue;
        };
        if !seen_keys.insert(key_str.clone()) {
            diags.push(Diag::new(
                raw.src.clone(),
                for_each_span.clone(),
                format!(
                    "module '{}' for_each has a duplicate key '{key_str}'",
                    raw.instance
                ),
            ));
            continue;
        }

        let entry = top_bindings.resolve_value(&raw.caller_module, val.expr());
        let Expression::Object(entry_obj) = entry.as_ref() else {
            diags.push(Diag::new(
                raw.src.clone(),
                span_of(val.expr().span()),
                format!(
                    "module '{}' for_each[\"{key_str}\"] must be an object mapping input names to literals, got {}",
                    raw.instance,
                    describe_expr_brief(entry.as_ref())
                ),
            ));
            continue;
        };

        let mut entry_inputs: Vec<(String, Expression, std::ops::Range<usize>)> = Vec::new();
        let mut entry_keys: HashSet<String> = HashSet::new();
        let mut field_err = false;
        for (fk, fv) in entry_obj.iter() {
            let Some(fk_str) = object_key_str(fk) else {
                diags.push(Diag::new(
                    raw.src.clone(),
                    span_of(val.expr().span()),
                    format!(
                        "module '{}' for_each[\"{key_str}\"] keys must be identifiers or strings",
                        raw.instance
                    ),
                ));
                field_err = true;
                continue;
            };
            if !entry_keys.insert(fk_str.clone()) {
                diags.push(Diag::new(
                    raw.src.clone(),
                    span_of(fv.expr().span()),
                    format!(
                        "module '{}' for_each[\"{key_str}\"] has a duplicate input '{fk_str}'",
                        raw.instance
                    ),
                ));
                field_err = true;
                continue;
            }
            entry_inputs.push((fk_str, fv.expr().clone(), span_of(fv.expr().span())));
        }
        if field_err {
            continue;
        }

        let instance = format!("{}.{}", raw.instance, key_str);
        if !seen_instances.insert(instance.clone()) {
            diags.push(Diag::new(
                raw.src.clone(),
                for_each_span.clone(),
                format!("duplicate module instance '{instance}'"),
            ));
            continue;
        }
        specs.push(ModuleSpec {
            instance,
            caller_module: raw.caller_module.clone(),
            src: raw.src.clone(),
            span: raw.source_span.clone(),
            source_path: raw.source_path.clone(),
            inputs: merge_inputs(&raw.shared_inputs, &entry_inputs),
        });
    }

    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(specs)
}

fn merge_inputs(
    shared: &[(String, Expression, std::ops::Range<usize>)],
    entry: &[(String, Expression, std::ops::Range<usize>)],
) -> Vec<(String, Expression, std::ops::Range<usize>)> {
    let mut out = shared.to_vec();
    for item in entry {
        if let Some(slot) = out.iter_mut().find(|(name, _, _)| name == &item.0) {
            *slot = item.clone();
        } else {
            out.push(item.clone());
        }
    }
    out
}

fn object_key_str(key: &ObjectKey) -> Option<String> {
    if let Some(ident) = key.as_ident() {
        return Some(ident.as_str().to_string());
    }
    if let ObjectKey::Expression(Expression::String(s)) = key {
        return Some(s.as_str().to_string());
    }
    None
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The root tree's `defaults` blocks, which every module instance can see. A
/// `defaults` block is a type-scoped shell rather than a value, so factoring
/// campaigns into templates must not force the shell to be written out again
/// per template (issue #148).
fn collect_defaults(root_files: &[ParsedFile]) -> Vec<InheritedDefaults> {
    let mut out = Vec::new();
    for f in root_files {
        for s in f.body.iter() {
            let Structure::Block(b) = s else { continue };
            if b.ident.as_str() != "defaults" {
                continue;
            }
            out.push(InheritedDefaults {
                file: f.path.display().to_string(),
                block: b.clone(),
            });
        }
    }
    out
}

fn load_module(
    spec: &ModuleSpec,
    top_bindings: &Bindings,
    shared_defaults: &[InheritedDefaults],
) -> Result<Scope, Vec<Diag>> {
    let mut diags = Vec::new();
    if !spec.source_path.exists() {
        diags.push(Diag::new(
            spec.src.clone(),
            spec.span.clone(),
            format!(
                "module '{}' source '{}' does not exist",
                spec.instance,
                spec.source_path.display()
            ),
        ));
        return Err(diags);
    }
    if !spec.source_path.is_file() {
        diags.push(Diag::new(
            spec.src.clone(),
            spec.span.clone(),
            format!(
                "module '{}' source '{}' must be a .bid file (directory sources not supported yet)",
                spec.instance,
                spec.source_path.display()
            ),
        ));
        return Err(diags);
    }

    let mut parsed = match parse_file(&spec.source_path) {
        Ok(p) => p,
        Err(d) => {
            diags.push(d);
            return Err(diags);
        }
    };

    if let Some(nested) = nested_module_block(&parsed) {
        diags.push(Diag::new(
            parsed.src.clone(),
            nested,
            format!(
                "module '{}' source has its own 'module' block — nested modules are not supported yet",
                spec.instance
            ),
        ));
        return Err(diags);
    }

    parsed.module = spec.instance.clone();
    parsed.inherited_defaults = shared_defaults.to_vec();

    let inputs = build_inputs(spec, top_bindings, &mut diags);
    if !diags.is_empty() {
        return Err(diags);
    }
    Ok(Scope {
        files: vec![parsed],
        inputs,
    })
}

fn build_inputs(
    spec: &ModuleSpec,
    top_bindings: &Bindings,
    diags: &mut Vec<Diag>,
) -> InputBindings {
    let mut out = InputBindings::default();
    for (key, expr, span) in &spec.inputs {
        let resolved = top_bindings.resolve_value(&spec.caller_module, expr);
        let value = match resolved.as_ref() {
            Expression::String(s) => s.as_str().to_string(),
            Expression::Number(n) => n.to_string(),
            Expression::Bool(b) => {
                if *b.as_ref() {
                    "true".to_string()
                } else {
                    "false".to_string()
                }
            }
            other => {
                diags.push(Diag::new(
                    spec.src.clone(),
                    span.clone(),
                    format!(
                        "module input '{key}' must be a literal (string / number / bool) or a local./var. reference that resolves to one, got {}",
                        describe_expr_brief(other)
                    ),
                ));
                continue;
            }
        };
        out.vars.insert(key.clone(), value);
    }
    out
}

fn nested_module_block(file: &ParsedFile) -> Option<std::ops::Range<usize>> {
    for s in file.body.iter() {
        let Structure::Block(b) = s else { continue };
        if b.ident.as_str() == "module" {
            return Some(span_of(b.ident.span()));
        }
    }
    None
}

fn span_of(s: Option<std::ops::Range<usize>>) -> std::ops::Range<usize> {
    s.unwrap_or(0..0)
}

fn describe_expr_brief(expr: &Expression) -> String {
    match expr {
        Expression::String(s) => format!("string \"{}\"", s.as_str()),
        Expression::Number(n) => format!("number {}", **n),
        Expression::Bool(b) => format!("boolean {}", **b),
        Expression::Array(_) => "array".to_string(),
        Expression::Object(_) => "object".to_string(),
        Expression::Null(_) => "null".to_string(),
        _ => "expression".to_string(),
    }
}

pub fn collect_bid_files(target: &Path) -> Result<Vec<PathBuf>, String> {
    if target.is_file() {
        return Ok(vec![target.to_path_buf()]);
    }
    let mut out = Vec::new();
    walk_bid_files(target, &mut out)
        .map_err(|e| format!("failed to read {}: {e}", target.display()))?;
    out.sort();
    Ok(out)
}

fn walk_bid_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
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
            walk_bid_files(&path, out)?;
        } else if ft.is_file() && path.extension().and_then(|e| e.to_str()) == Some("bid") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::InputBindings;
    use std::collections::HashSet;
    use std::fs;

    fn write_and_load(dir_name: &str, files: &[(&str, &str)]) -> Loaded {
        let root = std::env::temp_dir().join(dir_name);
        let _ = fs::remove_dir_all(&root);
        for (rel, content) in files {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
        }
        let collected = collect_bid_files(&root).unwrap();
        let loaded = Program::load(&collected, InputBindings::default());
        let _ = fs::remove_dir_all(&root);
        loaded
    }

    const TEMPLATE: &str = r#"
variable "campaign_name" {
  type = string
}

variable "region" {
  type = string
  default = "US"
}

resource "google_ads_campaign_budget" "budget" {
  name = var.campaign_name
  amount_micros = 1000000
}
"#;

    fn module_names(loaded: &Loaded) -> HashSet<String> {
        loaded
            .program
            .scopes
            .iter()
            .skip(1)
            .map(|s| s.files[0].module.clone())
            .collect()
    }

    fn errors(loaded: &Loaded) -> Vec<String> {
        loaded
            .diagnostics
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn for_each_expands_to_one_instance_per_entry() {
        let main = r#"
module "m" {
  source = "./t.bid"
  for_each = {
    privacy = { campaign_name = "Privacy" }
    adblock = { campaign_name = "Ad Blocker" }
  }
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-expand",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert_eq!(errors(&loaded), Vec::<String>::new());
        // root scope + two instances
        assert_eq!(loaded.program.scopes.len(), 3);
        assert_eq!(
            module_names(&loaded),
            HashSet::from(["m.privacy".to_string(), "m.adblock".to_string()])
        );
        let privacy = loaded
            .program
            .scopes
            .iter()
            .find(|s| s.files[0].module == "m.privacy")
            .unwrap();
        assert_eq!(
            privacy.inputs.vars.get("campaign_name"),
            Some(&"Privacy".to_string())
        );
    }

    #[test]
    fn for_each_merges_shared_attrs_entry_wins() {
        let main = r#"
module "m" {
  source = "./t.bid"
  region = "US"
  for_each = {
    a = { campaign_name = "A" }
    b = { campaign_name = "B", region = "CA" }
  }
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-merge",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert_eq!(errors(&loaded), Vec::<String>::new());
        let a = loaded
            .program
            .scopes
            .iter()
            .find(|s| s.files[0].module == "m.a")
            .unwrap();
        let b = loaded
            .program
            .scopes
            .iter()
            .find(|s| s.files[0].module == "m.b")
            .unwrap();
        // shared `region` flows to instances that don't set it...
        assert_eq!(a.inputs.vars.get("region"), Some(&"US".to_string()));
        // ...and the entry value wins where it does.
        assert_eq!(b.inputs.vars.get("region"), Some(&"CA".to_string()));
    }

    #[test]
    fn for_each_accepts_string_keys() {
        let main = r#"
module "m" {
  source = "./t.bid"
  for_each = {
    "sg-12" = { campaign_name = "SG 12" }
  }
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-strkey",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert_eq!(errors(&loaded), Vec::<String>::new());
        assert!(module_names(&loaded).contains("m.sg-12"));
    }

    #[test]
    fn for_each_value_from_a_local_map() {
        let main = r#"
locals {
  variants = {
    a = { campaign_name = "A" }
    b = { campaign_name = "B" }
  }
}

module "m" {
  source = "./t.bid"
  for_each = local.variants
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-local",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert_eq!(errors(&loaded), Vec::<String>::new());
        assert_eq!(
            module_names(&loaded),
            HashSet::from(["m.a".to_string(), "m.b".to_string()])
        );
    }

    #[test]
    fn for_each_empty_map_errors() {
        let main = r#"
module "m" {
  source = "./t.bid"
  for_each = {}
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-empty",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert!(errors(&loaded).iter().any(|m| m.contains("for_each map is empty")));
    }

    #[test]
    fn for_each_non_object_errors() {
        let main = r#"
module "m" {
  source = "./t.bid"
  for_each = "oops"
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-nonobj",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert!(errors(&loaded).iter().any(|m| m.contains("for_each must be an object")));
    }

    #[test]
    fn for_each_entry_not_object_errors() {
        let main = r#"
module "m" {
  source = "./t.bid"
  for_each = {
    a = "not an object"
  }
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-entry",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert!(errors(&loaded).iter().any(|m| m.contains("must be an object mapping input names")));
    }

    #[test]
    fn for_each_entry_field_not_scalar_errors() {
        let main = r#"
module "m" {
  source = "./t.bid"
  for_each = {
    a = { campaign_name = ["nope"] }
  }
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-field",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        // build_inputs rejects non-literal inputs with the same message v1 uses.
        assert!(errors(&loaded)
            .iter()
            .any(|m| m.contains("must be a literal")));
    }

    #[test]
    fn module_source_excluded_from_root_scope() {
        // The template declares a variable with no default; if it were loaded as
        // a root file it would error. It must only appear in instance scopes.
        let main = r#"
module "m" {
  source = "./t.bid"
  for_each = {
    a = { campaign_name = "A" }
  }
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-exclude",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert_eq!(errors(&loaded), Vec::<String>::new());
        // root scope holds only main.bid
        assert_eq!(loaded.program.scopes[0].files.len(), 1);
        assert_eq!(loaded.program.scopes[0].files[0].module, "main");
    }

    #[test]
    fn for_each_and_plain_module_coexist() {
        let main = r#"
module "single" {
  source = "./t.bid"
  campaign_name = "Solo"
}

module "many" {
  source = "./t.bid"
  for_each = {
    a = { campaign_name = "A" }
  }
}
"#;
        let loaded = write_and_load(
            "bidsmith-fe-coexist",
            &[("main.bid", main), ("t.bid", TEMPLATE)],
        );
        assert_eq!(errors(&loaded), Vec::<String>::new());
        assert_eq!(
            module_names(&loaded),
            HashSet::from(["single".to_string(), "many.a".to_string()])
        );
    }

    const SHARED_DEFAULTS: &str = r#"
defaults "google_ads_campaign" "video_plain" {
  advertising_channel_type = "VIDEO"
  status                   = "PAUSED"
}
"#;

    const CAMPAIGN_TEMPLATE: &str = r#"
variable "campaign_name" {
  type = string
}

resource "google_ads_campaign_budget" "budget" {
  name          = var.campaign_name
  amount_micros = 1000000
}

resource "google_ads_campaign" "campaign" {
  name            = var.campaign_name
  defaults        = defaults.video_plain
  campaign_budget = google_ads_campaign_budget.budget.id
}
"#;

    const MODULE_CALL: &str = r#"
provider "google_ads" {
  customer_id = "1234567890"
}

module "m" {
  source = "./t.bid"
  for_each = {
    sg = { campaign_name = "SG" }
    my = { campaign_name = "MY" }
  }
}
"#;

    fn validation_errors(loaded: &Loaded) -> Vec<String> {
        let mut out = errors(loaded);
        for scope in &loaded.program.scopes {
            out.extend(
                crate::schema::validate_files(&scope.files, &scope.inputs)
                    .into_iter()
                    .filter(|d| d.is_error())
                    .map(|d| d.message),
            );
        }
        out
    }

    fn campaigns(loaded: &Loaded) -> Vec<crate::commands::export::JsonCampaign> {
        crate::api::import::import_program(&loaded.program)
            .expect("import")
            .input
            .campaigns
    }

    #[test]
    fn root_defaults_reach_a_module_body() {
        let loaded = write_and_load(
            "bidsmith-defaults-inherit",
            &[
                ("main.bid", MODULE_CALL),
                ("shared.bid", SHARED_DEFAULTS),
                ("t.bid", CAMPAIGN_TEMPLATE),
            ],
        );
        assert_eq!(validation_errors(&loaded), Vec::<String>::new());

        let merged = campaigns(&loaded);
        assert_eq!(merged.len(), 2);
        for c in &merged {
            assert_eq!(c.advertising_channel_type, "VIDEO");
            assert_eq!(c.status.as_deref(), Some("PAUSED"));
        }
    }

    #[test]
    fn an_unnamed_root_defaults_block_reaches_a_module_body_too() {
        let shared = r#"
defaults "google_ads_campaign" {
  advertising_channel_type = "SEARCH"
  status                   = "PAUSED"
}
"#;
        let template = r#"
variable "campaign_name" {
  type = string
}

resource "google_ads_campaign_budget" "budget" {
  name          = var.campaign_name
  amount_micros = 1000000
}

resource "google_ads_campaign" "campaign" {
  name            = var.campaign_name
  campaign_budget = google_ads_campaign_budget.budget.id
}
"#;
        let main = r#"
provider "google_ads" {
  customer_id = "1234567890"
}

module "m" {
  source        = "./t.bid"
  campaign_name = "Solo"
}
"#;
        let loaded = write_and_load(
            "bidsmith-defaults-inherit-unnamed",
            &[
                ("main.bid", main),
                ("shared.bid", shared),
                ("t.bid", template),
            ],
        );
        assert_eq!(validation_errors(&loaded), Vec::<String>::new());
        assert_eq!(campaigns(&loaded)[0].advertising_channel_type, "SEARCH");
    }

    #[test]
    fn a_modules_own_defaults_shadow_the_inherited_block() {
        let template = format!(
            r#"
defaults "google_ads_campaign" "video_plain" {{
  advertising_channel_type = "SEARCH"
  status                   = "ENABLED"
}}
{CAMPAIGN_TEMPLATE}"#
        );
        let loaded = write_and_load(
            "bidsmith-defaults-shadow",
            &[
                ("main.bid", MODULE_CALL),
                ("shared.bid", SHARED_DEFAULTS),
                ("t.bid", &template),
            ],
        );
        assert_eq!(validation_errors(&loaded), Vec::<String>::new());
        for c in &campaigns(&loaded) {
            assert_eq!(c.advertising_channel_type, "SEARCH");
            assert_eq!(c.status.as_deref(), Some("ENABLED"));
        }
    }

    #[test]
    fn an_inherited_defaults_block_is_reported_once_not_once_per_instance() {
        let shared = r#"
defaults "google_ads_campaign" "video_plain" {
  advertising_channel_type = "VIDEO"
  nonsense                 = "oops"
}
"#;
        let loaded = write_and_load(
            "bidsmith-defaults-inherit-diag",
            &[
                ("main.bid", MODULE_CALL),
                ("shared.bid", shared),
                ("t.bid", CAMPAIGN_TEMPLATE),
            ],
        );
        let complaints: Vec<_> = validation_errors(&loaded)
            .into_iter()
            .filter(|m| m.contains("nonsense"))
            .collect();
        assert_eq!(complaints.len(), 1, "got {complaints:?}");
    }

    #[test]
    fn defaults_declared_in_a_template_stay_inside_it() {
        let template = format!(
            r#"
defaults "google_ads_campaign" "video_plain" {{
  advertising_channel_type = "VIDEO"
}}
{CAMPAIGN_TEMPLATE}"#
        );
        let main = r#"
provider "google_ads" {
  customer_id = "1234567890"
}

module "m" {
  source        = "./t.bid"
  campaign_name = "Solo"
}

resource "google_ads_campaign_budget" "b" {
  name          = "Root budget"
  amount_micros = 1000000
}

resource "google_ads_campaign" "root" {
  name            = "Root campaign"
  defaults        = defaults.video_plain
  campaign_budget = google_ads_campaign_budget.b.id
}
"#;
        let loaded = write_and_load(
            "bidsmith-defaults-no-leak-up",
            &[("main.bid", main), ("t.bid", &template)],
        );
        assert!(
            validation_errors(&loaded)
                .iter()
                .any(|m| m.contains("unknown defaults 'video_plain'")),
            "a template's defaults must not become visible to its caller"
        );
    }

    #[test]
    fn collect_bid_files_walks_subdirectories() {
        let root = std::env::temp_dir().join("bidsmith-collect-recursive-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("campaigns/search")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();

        fs::write(root.join("account.bid"), "").unwrap();
        fs::write(root.join("campaigns/search/brand.bid"), "").unwrap();
        fs::write(root.join("campaigns/notes.txt"), "").unwrap();
        fs::write(root.join(".hidden/secret.bid"), "").unwrap();
        fs::write(root.join("target/build.bid"), "").unwrap();

        let found = collect_bid_files(&root).unwrap();
        let rel: Vec<_> = found
            .iter()
            .map(|p| p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"))
            .collect();

        assert_eq!(rel, vec!["account.bid", "campaigns/search/brand.bid"]);

        fs::remove_dir_all(&root).unwrap();
    }
}

