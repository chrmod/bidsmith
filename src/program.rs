use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hcl_edit::Span;
use hcl_edit::expr::Expression;
use hcl_edit::structure::{Block, Structure};
use miette::NamedSource;

use crate::diagnostics::Diag;
use crate::parser::{parse_file, ParsedFile};
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

        let (top_bindings, _bind_diags) = Bindings::build(&top_files, &cli_inputs);

        let mut scopes = vec![Scope {
            files: top_files,
            inputs: cli_inputs.clone(),
        }];

        let mut seen_instances: HashSet<String> = HashSet::new();
        let mut module_specs: Vec<ModuleSpec> = Vec::new();

        for file in &scopes[0].files {
            for s in file.body.iter() {
                let Structure::Block(b) = s else { continue };
                if b.ident.as_str() != "module" {
                    continue;
                }
                match validate_block_shape(file, b, &mut seen_instances) {
                    Ok(spec) => module_specs.push(spec),
                    Err(ds) => diags.extend(ds),
                }
            }
        }

        for spec in &module_specs {
            match load_module(spec, &top_bindings) {
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

fn validate_block_shape(
    file: &ParsedFile,
    block: &Block,
    seen: &mut HashSet<String>,
) -> Result<ModuleSpec, Vec<Diag>> {
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
    if !seen.insert(instance.clone()) {
        diags.push(Diag::new(
            file.src.clone(),
            span_of(block.labels[0].span()),
            format!("duplicate module instance '{instance}'"),
        ));
        return Err(diags);
    }

    let mut source: Option<String> = None;
    let mut source_span: Option<std::ops::Range<usize>> = None;
    let mut inputs: Vec<(String, Expression, std::ops::Range<usize>)> = Vec::new();
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
                } else {
                    inputs.push((key, a.value.clone(), span_of(a.value.span())));
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

    Ok(ModuleSpec {
        instance,
        caller_module: file.module.clone(),
        src: file.src.clone(),
        span: source_span.unwrap_or_else(|| span_of(block.ident.span())),
        source_path,
        inputs,
    })
}

fn load_module(spec: &ModuleSpec, top_bindings: &Bindings) -> Result<Scope, Vec<Diag>> {
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
        let value = match resolved {
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
    use super::collect_bid_files;
    use std::fs;

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

