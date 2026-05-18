use std::sync::Arc;

use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

#[derive(Debug, Error, Diagnostic, Clone)]
#[error("{message}")]
pub struct Diag {
    pub message: String,
    #[source_code]
    pub src: Arc<NamedSource<String>>,
    #[label]
    pub span: SourceSpan,
}

impl Diag {
    pub fn new(
        src: Arc<NamedSource<String>>,
        span: std::ops::Range<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            message: message.into(),
            src,
            span: SourceSpan::from((span.start, span.len())),
        }
    }
}
