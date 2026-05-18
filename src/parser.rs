use std::path::{Path, PathBuf};
use std::sync::Arc;

use hcl_edit::structure::Body;
use miette::NamedSource;

use crate::diagnostics::Diag;

pub struct ParsedFile {
    pub path: PathBuf,
    pub src: Arc<NamedSource<String>>,
    pub body: Body,
}

pub fn parse_file(path: &Path) -> Result<ParsedFile, Diag> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        let name = path.display().to_string();
        let empty = Arc::new(NamedSource::new(name, String::new()));
        Diag::new(empty, 0..0, format!("cannot read file: {e}"))
    })?;
    let name = path.display().to_string();
    let src = Arc::new(NamedSource::new(name, raw.clone()));
    match raw.parse::<Body>() {
        Ok(body) => Ok(ParsedFile {
            path: path.to_path_buf(),
            src,
            body,
        }),
        Err(e) => {
            let (span, message) = parse_error_span(&e, raw.len());
            Err(Diag::new(src, span, message))
        }
    }
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
