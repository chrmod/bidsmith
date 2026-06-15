use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Attribute, Block, Body, Structure};

use crate::parser::parse_file;
use crate::program::collect_bid_files;
use crate::schema::{expr_matches_default, resource_schema, BlockSchema};

pub fn run(target: &str, check: bool, minimal: bool) -> ExitCode {
    let target = Path::new(target);
    if !target.exists() {
        eprintln!("No such file or directory: {}", target.display());
        return ExitCode::from(1);
    }

    let files: Vec<PathBuf> = match collect_bid_files(target) {
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

    let mut changed: Vec<PathBuf> = Vec::new();
    let mut parse_errors = 0usize;

    for path in &files {
        let parsed = match parse_file(path) {
            Ok(p) => p,
            Err(d) => {
                let report = miette::Report::new(d);
                eprintln!("{report:?}");
                parse_errors += 1;
                continue;
            }
        };
        let original = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("failed to read {}: {e}", path.display());
                parse_errors += 1;
                continue;
            }
        };
        let canonical = if minimal {
            format_body_minimal(&parsed.body)
        } else {
            format_body(&parsed.body)
        };
        if canonical == original {
            continue;
        }
        changed.push(path.clone());
        if !check {
            if let Err(e) = std::fs::write(path, &canonical) {
                eprintln!("failed to write {}: {e}", path.display());
                return ExitCode::from(1);
            }
        }
    }

    if parse_errors > 0 {
        return ExitCode::from(1);
    }

    if check {
        if changed.is_empty() {
            println!("fmt: {} file(s) already canonical.", files.len());
            ExitCode::SUCCESS
        } else {
            for p in &changed {
                println!("would reformat {}", p.display());
            }
            ExitCode::from(1)
        }
    } else {
        if changed.is_empty() {
            println!("fmt: {} file(s) already canonical.", files.len());
        } else {
            for p in &changed {
                println!("reformatted {}", p.display());
            }
        }
        ExitCode::SUCCESS
    }
}

pub fn format_body(body: &Body) -> String {
    let mut out = String::new();
    emit_body(body, 0, None, false, &mut out);
    out
}

/// Like [`format_body`], but strips optional attributes whose literal value
/// equals their schema default (skipping `always_emit` ones). This is the
/// canonical "minimal" form that `refresh` / `export` emit and `fmt --minimal`
/// rewrites to.
pub fn format_body_minimal(body: &Body) -> String {
    let mut out = String::new();
    emit_body(body, 0, None, true, &mut out);
    out
}

fn emit_body(
    body: &Body,
    indent: usize,
    schema: Option<&'static BlockSchema>,
    minimal: bool,
    out: &mut String,
) {
    let mut prev: Option<&Structure> = None;
    for s in body.iter() {
        if minimal {
            if let Structure::Attribute(a) = s {
                if should_strip(schema, a) {
                    continue;
                }
            }
        }
        if let Some(p) = prev {
            if needs_blank_line(p, s) {
                out.push('\n');
            }
        }
        emit_structure(s, indent, schema, minimal, out);
        prev = Some(s);
    }
}

fn should_strip(schema: Option<&'static BlockSchema>, a: &Attribute) -> bool {
    let Some(schema) = schema else { return false };
    let Some(attr) = schema.attributes.iter().find(|x| x.name == a.key.as_str()) else {
        return false;
    };
    match attr.droppable_default() {
        Some(def) => expr_matches_default(&a.value, def),
        None => false,
    }
}

/// The schema governing a block's body: the resource type's schema for a
/// `resource "<type>" "<name>"` block, or the matching nested-block schema
/// when descending inside one. `None` for provider/locals/variable/module and
/// anything unrecognised — those bodies are emitted verbatim.
fn child_schema_for(
    b: &Block,
    parent: Option<&'static BlockSchema>,
) -> Option<&'static BlockSchema> {
    if b.ident.as_str() == "resource" && b.labels.len() == 2 {
        return resource_schema(b.labels[0].as_str());
    }
    parent.and_then(|p| {
        p.blocks
            .iter()
            .find(|n| n.name == b.ident.as_str())
            .map(|n| &n.schema)
    })
}

fn needs_blank_line(prev: &Structure, next: &Structure) -> bool {
    !matches!((prev, next), (Structure::Attribute(_), Structure::Attribute(_)))
}

fn emit_structure(
    s: &Structure,
    indent: usize,
    schema: Option<&'static BlockSchema>,
    minimal: bool,
    out: &mut String,
) {
    match s {
        Structure::Attribute(a) => {
            write_indent(out, indent);
            out.push_str(a.key.as_str());
            out.push_str(" = ");
            emit_expr(&a.value, indent, out);
            out.push('\n');
        }
        Structure::Block(b) => emit_block(b, indent, schema, minimal, out),
    }
}

fn emit_block(
    b: &Block,
    indent: usize,
    parent_schema: Option<&'static BlockSchema>,
    minimal: bool,
    out: &mut String,
) {
    write_indent(out, indent);
    out.push_str(b.ident.as_str());
    for label in &b.labels {
        out.push(' ');
        emit_string_literal(label.as_str(), out);
    }
    if b.body.is_empty() {
        out.push_str(" {}\n");
        return;
    }
    let child_schema = if minimal {
        child_schema_for(b, parent_schema)
    } else {
        None
    };
    out.push_str(" {\n");
    emit_body(&b.body, indent + 1, child_schema, minimal, out);
    write_indent(out, indent);
    out.push_str("}\n");
}

fn emit_expr(e: &Expression, indent: usize, out: &mut String) {
    match e {
        Expression::String(s) => emit_string_literal(s.as_str(), out),
        Expression::Number(n) => out.push_str(&n.to_string()),
        Expression::Bool(b) => out.push_str(if *b.as_ref() { "true" } else { "false" }),
        Expression::Null(_) => out.push_str("null"),
        Expression::Variable(v) => out.push_str(v.as_str()),
        Expression::Traversal(t) => emit_traversal(t, indent, out),
        Expression::Array(arr) => emit_array(arr.iter(), indent, out),
        _ => {
            let s = e.to_string();
            out.push_str(s.trim());
        }
    }
}

fn emit_traversal(t: &Traversal, indent: usize, out: &mut String) {
    emit_expr(&t.expr, indent, out);
    for op in t.operators.iter() {
        match &**op {
            TraversalOperator::GetAttr(name) => {
                out.push('.');
                out.push_str(name.as_str());
            }
            TraversalOperator::Index(inner) => {
                out.push('[');
                emit_expr(inner, indent, out);
                out.push(']');
            }
            TraversalOperator::LegacyIndex(n) => {
                out.push('.');
                out.push_str(&(**n).to_string());
            }
            TraversalOperator::AttrSplat(_) => out.push_str(".*"),
            TraversalOperator::FullSplat(_) => out.push_str("[*]"),
        }
    }
}

fn emit_array<'a, I: IntoIterator<Item = &'a Expression>>(
    items: I,
    indent: usize,
    out: &mut String,
) {
    let items: Vec<&Expression> = items.into_iter().collect();
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    let single = render_single_line_array(&items, indent);
    if single.len() <= 80 && !single.contains('\n') {
        out.push_str(&single);
        return;
    }
    out.push_str("[\n");
    for item in &items {
        write_indent(out, indent + 1);
        emit_expr(item, indent + 1, out);
        out.push_str(",\n");
    }
    write_indent(out, indent);
    out.push(']');
}

fn render_single_line_array(items: &[&Expression], indent: usize) -> String {
    let mut buf = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        emit_expr(item, indent, &mut buf);
    }
    buf.push(']');
    buf
}

fn emit_string_literal(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_indent(out: &mut String, indent: usize) {
    for _ in 0..indent {
        out.push_str("  ");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(src: &str) -> String {
        let body: Body = src.parse().expect("parse");
        format_body_minimal(&body)
    }

    #[test]
    fn strips_attributes_equal_to_default() {
        let out = minimal(
            r#"resource "google_ads_campaign_budget" "b" {
  name = "B"
  amount_micros = 1000000
  delivery_method = "STANDARD"
  explicitly_shared = false
}
"#,
        );
        assert!(!out.contains("delivery_method"), "{out}");
        assert!(!out.contains("explicitly_shared"), "{out}");
        assert!(out.contains("amount_micros = 1000000"), "{out}");
    }

    #[test]
    fn keeps_non_default_and_always_emit_values() {
        let out = minimal(
            r#"resource "google_ads_campaign" "c" {
  name = "C"
  status = "PAUSED"
  advertising_channel_type = "SEARCH"
  campaign_budget = google_ads_campaign_budget.b.id
  contains_eu_political_advertising = "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"
}
"#,
        );
        // status carries signal (PAUSED != ENABLED) — kept.
        assert!(out.contains("status = \"PAUSED\""), "{out}");
        // EU political is a compliance declaration — always emitted even at default.
        assert!(out.contains("contains_eu_political_advertising"), "{out}");
    }

    #[test]
    fn strips_default_status_but_keeps_compliance_field() {
        let out = minimal(
            r#"resource "google_ads_campaign" "c" {
  name = "C"
  status = "ENABLED"
  advertising_channel_type = "SEARCH"
  campaign_budget = google_ads_campaign_budget.b.id
  contains_eu_political_advertising = "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"
}
"#,
        );
        assert!(!out.contains("status"), "{out}");
        assert!(out.contains("contains_eu_political_advertising"), "{out}");
    }

    #[test]
    fn preserves_non_literal_default_candidates() {
        // A reference can't be proven equal to the default, so it stays.
        let out = minimal(
            r#"resource "google_ads_ad_group" "g" {
  name = "G"
  campaign = google_ads_campaign.c.id
  status = local.ag_status
}
"#,
        );
        assert!(out.contains("status = local.ag_status"), "{out}");
    }

    #[test]
    fn does_not_strip_lookalike_attrs_outside_their_schema() {
        // `status` inside a provider block isn't a managed default — left alone.
        let out = minimal(
            r#"provider "google_ads" {
  customer_id = "1234567890"
}
"#,
        );
        assert!(out.contains("customer_id = \"1234567890\""), "{out}");
    }
}
