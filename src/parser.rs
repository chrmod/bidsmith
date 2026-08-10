use std::path::{Path, PathBuf};
use std::sync::Arc;

use hcl_edit::structure::Body;
use miette::NamedSource;

use crate::diagnostics::Diag;

#[derive(Clone)]
pub struct ParsedFile {
    pub path: PathBuf,
    pub src: Arc<NamedSource<String>>,
    pub body: Body,
    pub module: String,
}

pub fn parse_file(path: &Path) -> Result<ParsedFile, Diag> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        let name = path.display().to_string();
        let empty = Arc::new(NamedSource::new(name, String::new()));
        Diag::new(empty, 0..0, format!("cannot read file: {e}"))
    })?;
    parse_str(path, &raw)
}

pub fn parse_str(path: &Path, raw: &str) -> Result<ParsedFile, Diag> {
    let name = path.display().to_string();
    let src = Arc::new(NamedSource::new(name, raw.to_string()));
    let module = module_name(path);
    match raw.parse::<Body>() {
        Ok(body) => Ok(ParsedFile {
            path: path.to_path_buf(),
            src,
            body,
            module,
        }),
        Err(e) => {
            let (span, message) = parse_error_span(&e, raw.len());
            Err(Diag::new(src, span, message))
        }
    }
}

pub fn module_name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("module");
    slugify(stem)
}

fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_underscore = true;
    for ch in s.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            out.push(lower);
            prev_underscore = false;
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("module");
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn parse_error_span(
    e: &hcl_edit::parser::Error,
    src_len: usize,
) -> (std::ops::Range<usize>, String) {
    let loc = e.location();
    let start = loc.offset();
    let end = (start + 1).min(src_len.max(start + 1));
    (start..end, format!("{}", e))
}
