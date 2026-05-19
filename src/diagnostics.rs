use std::sync::Arc;

use miette::{Diagnostic, LabeledSpan, NamedSource, Severity, SourceCode, SourceSpan};
use thiserror::Error;

#[derive(Debug, Error, Clone)]
#[error("{message}")]
pub struct Diag {
    pub severity: Severity,
    pub message: String,
    pub src: Arc<NamedSource<String>>,
    pub span: SourceSpan,
}

impl Diag {
    pub fn new(
        src: Arc<NamedSource<String>>,
        span: std::ops::Range<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
            src,
            span: SourceSpan::from((span.start, span.len())),
        }
    }

    pub fn warning(
        src: Arc<NamedSource<String>>,
        span: std::ops::Range<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
            src,
            span: SourceSpan::from((span.start, span.len())),
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.severity, Severity::Error)
    }
}

impl Diagnostic for Diag {
    fn severity(&self) -> Option<Severity> {
        Some(self.severity)
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        Some(self.src.as_ref())
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        Some(Box::new(std::iter::once(LabeledSpan::new_with_span(
            None, self.span,
        ))))
    }
}
