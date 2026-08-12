use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hcl_edit::expr::{Array, Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Attribute, Block, Body, Structure};
use hcl_edit::{Decor, Decorate, RawString};

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

/// One node's comments; an empty `lead` entry renders as a blank line.
#[derive(Default, Clone)]
struct Comments {
    blank_before: bool,
    lead: Vec<String>,
    trail: Option<String>,
}

impl Comments {
    fn is_empty(&self) -> bool {
        self.lead.is_empty() && self.trail.is_none()
    }

    fn add_trail(&mut self, text: Option<String>) {
        let Some(text) = text else { return };
        self.trail = Some(match self.trail.take() {
            Some(prev) => format!("{prev} {text}"),
            None => text,
        });
    }
}

fn raw_text(r: Option<&RawString>) -> String {
    r.map(|r| r.to_string()).unwrap_or_default()
}

/// Each comment token in a decor string, paired with the newlines before it.
fn scan_comments(raw: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut newlines = 0usize;
    let mut rest = raw;
    while let Some(c) = rest.chars().next() {
        if c == '\n' {
            newlines += 1;
            rest = &rest[1..];
        } else if c.is_whitespace() {
            rest = &rest[c.len_utf8()..];
        } else if c == '#' || rest.starts_with("//") {
            let end = rest.find('\n').unwrap_or(rest.len());
            out.push((newlines, rest[..end].trim_end().to_string()));
            newlines = 0;
            rest = &rest[end..];
        } else if rest.starts_with("/*") {
            let end = rest[2..].find("*/").map_or(rest.len(), |k| k + 4);
            out.push((newlines, rest[..end].to_string()));
            newlines = 0;
            rest = &rest[end..];
        } else {
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// A single newline here already means a blank line, not a line break.
fn leading_comments(raw: &str) -> (bool, Vec<String>) {
    let tokens = scan_comments(raw);
    let mut lines: Vec<String> = Vec::new();
    let mut blank_before = false;
    for (i, (newlines, text)) in tokens.iter().enumerate() {
        if i == 0 {
            blank_before = *newlines >= 1;
        } else if *newlines >= 2 {
            lines.push(String::new());
        }
        push_comment_lines(&mut lines, text);
    }
    if !lines.is_empty() && trailing_newlines(raw) >= 2 {
        lines.push(String::new());
    }
    (blank_before, lines)
}

fn trailing_newlines(raw: &str) -> usize {
    raw.chars()
        .rev()
        .take_while(|c| c.is_whitespace())
        .filter(|c| *c == '\n')
        .count()
}

/// Dedent a `/* … */` block so re-indenting it keeps its internal shape.
fn push_comment_lines(lines: &mut Vec<String>, text: &str) {
    let mut parts = text.split('\n');
    lines.push(parts.next().unwrap_or_default().trim_end().to_string());
    let rest: Vec<&str> = parts.collect();
    let dedent = rest
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    for line in rest {
        let line = line.trim_end();
        if line.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(line.get(dedent..).unwrap_or(line.trim_start()).to_string());
        }
    }
}

/// Flattened, so a multi-line comment can't swallow the rest of the line.
fn trailing_comment(raw: &str) -> Option<String> {
    let tokens = scan_comments(raw);
    if tokens.is_empty() {
        return None;
    }
    Some(
        tokens
            .iter()
            .map(|(_, t)| t.split('\n').map(str::trim).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn comments_of_decor(decor: &Decor) -> Comments {
    let (blank_before, lead) = leading_comments(&raw_text(decor.prefix()));
    Comments {
        blank_before,
        lead,
        trail: trailing_comment(&raw_text(decor.suffix())),
    }
}

fn comments_of(s: &Structure) -> Comments {
    let mut c = comments_of_decor(s.decor());
    match s {
        Structure::Attribute(a) => {
            c.add_trail(trailing_comment(&raw_text(a.key.decor().suffix())));
            c.add_trail(trailing_comment(&raw_text(a.value.decor().prefix())));
            c.add_trail(trailing_comment(&raw_text(a.value.decor().suffix())));
        }
        Structure::Block(b) => {
            c.add_trail(trailing_comment(&raw_text(b.ident.decor().suffix())));
            for label in &b.labels {
                c.add_trail(trailing_comment(&raw_text(label.decor().prefix())));
                c.add_trail(trailing_comment(&raw_text(label.decor().suffix())));
            }
        }
    }
    c
}

fn emit_comment_lines(lines: &[String], indent: usize, out: &mut String) {
    for line in lines {
        if line.is_empty() {
            out.push('\n');
        } else {
            write_indent(out, indent);
            out.push_str(line);
            out.push('\n');
        }
    }
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
        let comments = comments_of(s);
        if minimal && comments.is_empty() {
            if let Structure::Attribute(a) = s {
                if should_strip(schema, a) {
                    continue;
                }
            }
        }
        if let Some(p) = prev {
            if needs_blank_line(p, s) || (comments.blank_before && !comments.lead.is_empty()) {
                out.push('\n');
            }
        }
        emit_comment_lines(&comments.lead, indent, out);
        emit_structure(s, indent, schema, minimal, &comments, out);
        prev = Some(s);
    }
    emit_dangling(body, indent, prev.is_some(), out);
}

/// Comments with no node after them — before a closing brace, or at end of file.
fn emit_dangling(body: &Body, indent: usize, had_structures: bool, out: &mut String) {
    let mut raw = raw_text(body.decor().suffix());
    if body.is_empty() {
        raw.insert_str(0, &raw_text(body.decor().prefix()));
    }
    let (blank_before, lines) = leading_comments(&raw);
    if lines.is_empty() {
        return;
    }
    if had_structures && blank_before {
        out.push('\n');
    }
    emit_comment_lines(&lines, indent, out);
}

fn body_has_comments(body: &Body) -> bool {
    let decor = body.decor();
    !scan_comments(&raw_text(decor.prefix())).is_empty()
        || !scan_comments(&raw_text(decor.suffix())).is_empty()
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
    comments: &Comments,
    out: &mut String,
) {
    match s {
        Structure::Attribute(a) => {
            write_indent(out, indent);
            out.push_str(a.key.as_str());
            out.push_str(" = ");
            emit_expr(&a.value, indent, out);
            emit_trailing_comment(comments, out);
            out.push('\n');
        }
        Structure::Block(b) => emit_block(b, indent, schema, minimal, comments, out),
    }
}

fn emit_trailing_comment(comments: &Comments, out: &mut String) {
    if let Some(trail) = &comments.trail {
        out.push(' ');
        out.push_str(trail);
    }
}

fn emit_block(
    b: &Block,
    indent: usize,
    parent_schema: Option<&'static BlockSchema>,
    minimal: bool,
    comments: &Comments,
    out: &mut String,
) {
    write_indent(out, indent);
    out.push_str(b.ident.as_str());
    for label in &b.labels {
        out.push(' ');
        emit_string_literal(label.as_str(), out);
    }
    if b.body.is_empty() && !body_has_comments(&b.body) {
        out.push_str(" {}");
        emit_trailing_comment(comments, out);
        out.push('\n');
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
    out.push('}');
    emit_trailing_comment(comments, out);
    out.push('\n');
}

fn emit_expr(e: &Expression, indent: usize, out: &mut String) {
    match e {
        Expression::String(s) => emit_string_literal(s.as_str(), out),
        Expression::Number(n) => out.push_str(&n.to_string()),
        Expression::Bool(b) => out.push_str(if *b.as_ref() { "true" } else { "false" }),
        Expression::Null(_) => out.push_str("null"),
        Expression::Variable(v) => out.push_str(v.as_str()),
        Expression::Traversal(t) => emit_traversal(t, indent, out),
        Expression::Array(arr) => emit_array(arr, indent, out),
        _ => {
            // `to_string` re-encodes decor the caller has already placed.
            let mut bare = e.clone();
            bare.decor_mut().set_prefix("");
            bare.decor_mut().set_suffix("");
            out.push_str(bare.to_string().trim());
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

fn emit_array(arr: &Array, indent: usize, out: &mut String) {
    let items: Vec<&Expression> = arr.iter().collect();
    let (item_comments, dangling) = array_comments(arr, &items);

    if items.is_empty() {
        if dangling.is_empty() {
            out.push_str("[]");
        } else {
            out.push_str("[\n");
            emit_comment_lines(&dangling, indent + 1, out);
            write_indent(out, indent);
            out.push(']');
        }
        return;
    }
    let commented = item_comments.iter().any(|c| !c.is_empty()) || !dangling.is_empty();
    if !commented {
        let single = render_single_line_array(&items, indent);
        if single.len() <= 80 && !single.contains('\n') {
            out.push_str(&single);
            return;
        }
    }
    out.push_str("[\n");
    for (item, comments) in items.iter().zip(&item_comments) {
        emit_comment_lines(&comments.lead, indent + 1, out);
        write_indent(out, indent + 1);
        emit_expr(item, indent + 1, out);
        out.push(',');
        emit_trailing_comment(comments, out);
        out.push('\n');
    }
    emit_comment_lines(&dangling, indent + 1, out);
    write_indent(out, indent);
    out.push(']');
}

/// An element's end-of-line comment lands in the *next* element's prefix.
fn array_comments(arr: &Array, items: &[&Expression]) -> (Vec<Comments>, Vec<String>) {
    let mut per_item = vec![Comments::default(); items.len()];
    for (i, item) in items.iter().enumerate() {
        let prefix = raw_text(item.decor().prefix());
        let (head, tail) = split_at_first_newline(&prefix);
        match i.checked_sub(1) {
            Some(p) => per_item[p].add_trail(trailing_comment(head)),
            None => per_item[0].lead.extend(trailing_comment(head)),
        }
        per_item[i].lead.extend(leading_comments(tail).1);
        per_item[i].add_trail(trailing_comment(&raw_text(item.decor().suffix())));
    }
    let trailing = arr.trailing().to_string();
    let (head, tail) = split_at_first_newline(&trailing);
    let mut dangling = Vec::new();
    match items.len().checked_sub(1) {
        Some(last) => per_item[last].add_trail(trailing_comment(head)),
        None => dangling.extend(trailing_comment(head)),
    }
    dangling.extend(leading_comments(tail).1);
    (per_item, dangling)
}

fn split_at_first_newline(raw: &str) -> (&str, &str) {
    match raw.find('\n') {
        Some(k) => (&raw[..k], &raw[k..]),
        None => ("", raw),
    }
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

    fn canonical(src: &str) -> String {
        let body: Body = src.parse().expect("parse");
        format_body(&body)
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

    #[test]
    fn keeps_leading_and_trailing_comments() {
        let src = r#"# Summer 2026 budgets.

resource "google_ads_campaign_budget" "summer" {
  name = "Summer"

  # EUR 20/day, signed off by finance.
  amount_micros = 20000000 # 20 EUR
}

# End of file.
"#;
        assert_eq!(canonical(src), src);
    }

    #[test]
    fn keeps_comments_on_blocks_and_dangling_ones() {
        let src = r#"resource "google_ads_campaign" "c" {
  name = "C"

  # Search partners only, per the 2026 media plan.
  network_settings {
    target_search_network = true # partners
  }
  # Content network stays off until Q3.
}
"#;
        assert_eq!(canonical(src), src);
    }

    #[test]
    fn keeps_comments_inside_lists() {
        let src = r#"resource "google_ads_ad_group_criterion" "kw" {
  keywords = [
    # brand terms
    "acme shoes", # exact only
    "acme boots",
    # long tail lands in its own resource
  ]
}
"#;
        assert_eq!(canonical(src), src);
    }

    #[test]
    fn keeps_block_comments_and_slash_style() {
        let src = r#"resource "google_ads_campaign" "c" {
  /*
    Paused until legal signs off.
    Ticket ADS-4417.
  */
  status = "PAUSED"

  // Renamed from "Spring" in March.
  name = "C"
}
"#;
        assert_eq!(canonical(src), src);
    }

    #[test]
    fn formatting_is_idempotent_with_comments() {
        let src = r#"# header
resource "google_ads_campaign_budget"   "b" {
      name="B"   # the name
  # why
      amount_micros=20000000
    empty {
    # nothing yet
    }
}
# tail
"#;
        let once = canonical(src);
        assert_eq!(canonical(&once), once, "{once}");
        assert!(once.contains("# header"), "{once}");
        assert!(once.contains("# the name"), "{once}");
        assert!(once.contains("# why"), "{once}");
        assert!(once.contains("# nothing yet"), "{once}");
        assert!(once.contains("# tail"), "{once}");
    }

    #[test]
    fn minimal_keeps_a_default_attribute_that_carries_a_comment() {
        let out = minimal(
            r#"resource "google_ads_campaign_budget" "b" {
  name = "B"
  amount_micros = 1000000

  # Standard on purpose: accelerated overspends on weekends.
  delivery_method = "STANDARD"
  explicitly_shared = false
}
"#,
        );
        assert!(out.contains("delivery_method = \"STANDARD\""), "{out}");
        assert!(out.contains("accelerated overspends"), "{out}");
        assert!(!out.contains("explicitly_shared"), "{out}");
    }

    #[test]
    fn comment_free_files_format_exactly_as_before() {
        let src = r#"resource "google_ads_campaign" "c" {
  name = "C"
  status = "PAUSED"

  network_settings {
    target_search_network = true
  }
}
"#;
        assert_eq!(canonical(src), src);
    }
}
