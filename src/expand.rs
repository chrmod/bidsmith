use hcl_edit::expr::{Expression, ObjectKey, TraversalOperator};
use hcl_edit::structure::{Block, BlockLabel, Body, Structure};
use hcl_edit::{Decorate, Decorated, Span};

use crate::diagnostics::Diag;
use crate::eval::{EvalCtx, EvalError};
use crate::parser::ParsedFile;
use crate::schema::{extract_traversal_path, Bindings, InputBindings};

/// Fan a `resource` block with `for_each` out into one block per entry,
/// substituting `each.key` / `each.value` and keying the address label
/// (`t_devices` → `t_devices["MOBILE"]`). Files without `for_each` pass
/// through with their bodies shared as-is.
pub fn expand_resource_for_each(
    files: &[ParsedFile],
    inputs: &InputBindings,
) -> (Vec<ParsedFile>, Vec<Diag>) {
    let (bindings, _binding_diags) = Bindings::build(files, inputs);
    let mut diags = Vec::new();
    let out = files
        .iter()
        .map(|f| expand_file(f, &bindings, &mut diags))
        .collect();
    (out, diags)
}

fn expand_file(file: &ParsedFile, bindings: &Bindings, diags: &mut Vec<Diag>) -> ParsedFile {
    let needs_expansion = file.body.iter().any(|s| {
        matches!(s, Structure::Block(b) if is_for_each_resource(b))
    });
    if !needs_expansion {
        return file.clone();
    }

    let mut body = Body::new();
    for s in file.body.iter() {
        match s {
            Structure::Block(b) if is_for_each_resource(b) => {
                for block in expand_block(file, b, bindings, diags) {
                    body.push(block);
                }
            }
            other => body.push(other.clone()),
        }
    }
    ParsedFile {
        body,
        ..file.clone()
    }
}

fn is_for_each_resource(block: &Block) -> bool {
    block.ident.as_str() == "resource"
        && block.labels.len() == 2
        && block
            .body
            .iter()
            .any(|s| matches!(s, Structure::Attribute(a) if a.key.as_str() == "for_each"))
}

fn expand_block(
    file: &ParsedFile,
    block: &Block,
    bindings: &Bindings,
    diags: &mut Vec<Diag>,
) -> Vec<Block> {
    let address = format!("{}.{}", block.labels[0].as_str(), block.labels[1].as_str());
    let for_each_attr = block
        .body
        .iter()
        .find_map(|s| match s {
            Structure::Attribute(a) if a.key.as_str() == "for_each" => Some(a),
            _ => None,
        })
        .expect("checked by is_for_each_resource");
    let value_span = span_of(for_each_attr.value.span());

    let ctx = EvalCtx {
        locals: &bindings.locals,
        variables: &bindings.variables,
    };
    let resolved = match ctx.eval(&file.module, &for_each_attr.value) {
        Ok(v) => v,
        Err(EvalError::Silent) => return Vec::new(),
        Err(EvalError::Message(message)) => {
            diags.push(Diag::new(file.src.clone(), value_span, message));
            return Vec::new();
        }
    };

    let entries = match resolved.as_ref() {
        Expression::Array(arr) => {
            let mut entries: Vec<(String, Expression)> = Vec::new();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for item in arr.iter() {
                let item_span = item.span().unwrap_or(value_span.clone());
                let value = match ctx.eval(&file.module, item) {
                    Ok(v) => v,
                    Err(EvalError::Silent) => continue,
                    Err(EvalError::Message(message)) => {
                        diags.push(Diag::new(file.src.clone(), item_span, message));
                        continue;
                    }
                };
                let Expression::String(s) = value.as_ref() else {
                    diags.push(Diag::new(
                        file.src.clone(),
                        item_span,
                        format!(
                            "{address} for_each list entries must be strings, got {}; use a map for non-string values",
                            crate::schema::describe_expr(value.as_ref())
                        ),
                    ));
                    continue;
                };
                let key = s.value().clone();
                if !seen.insert(key.clone()) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        item_span,
                        format!("{address} for_each has a duplicate entry \"{key}\""),
                    ));
                    continue;
                }
                entries.push((key, value.into_owned()));
            }
            entries
        }
        Expression::Object(obj) => {
            let mut entries: Vec<(String, Expression)> = Vec::new();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for (key, value) in obj.iter() {
                let Some(key_str) = object_key_str(key) else {
                    diags.push(Diag::new(
                        file.src.clone(),
                        value_span.clone(),
                        format!("{address} for_each keys must be identifiers or strings"),
                    ));
                    continue;
                };
                if !seen.insert(key_str.clone()) {
                    diags.push(Diag::new(
                        file.src.clone(),
                        value_span.clone(),
                        format!("{address} for_each has a duplicate key \"{key_str}\""),
                    ));
                    continue;
                }
                entries.push((key_str, value.expr().clone()));
            }
            entries
        }
        other => {
            diags.push(Diag::new(
                file.src.clone(),
                value_span,
                format!(
                    "{address} for_each must be a list of strings or a map, got {}",
                    crate::schema::describe_expr(other)
                ),
            ));
            return Vec::new();
        }
    };

    if entries.is_empty() {
        diags.push(Diag::new(
            file.src.clone(),
            value_span,
            format!(
                "{address} for_each is empty; declare at least one entry or remove for_each"
            ),
        ));
        return Vec::new();
    }

    let mut out = Vec::new();
    for (key, value) in &entries {
        let mut instance = block.clone();
        instance.labels[1] = BlockLabel::String(Decorated::new(format!(
            "{}[\"{key}\"]",
            block.labels[1].as_str()
        )));
        instance.body = substitute_body(&instance.body, key, value, file, diags);
        out.push(instance);
    }
    out
}

fn substitute_body(
    body: &Body,
    key: &str,
    value: &Expression,
    file: &ParsedFile,
    diags: &mut Vec<Diag>,
) -> Body {
    let mut out = Body::new();
    for s in body.iter() {
        match s {
            Structure::Attribute(a) => {
                if a.key.as_str() == "for_each" {
                    continue;
                }
                let mut attr = a.clone();
                substitute_expr(&mut attr.value, key, value, file, diags);
                out.push(attr);
            }
            Structure::Block(b) => {
                let mut block = b.clone();
                block.body = substitute_body(&b.body, key, value, file, diags);
                out.push(block);
            }
        }
    }
    out
}

fn substitute_expr(
    expr: &mut Expression,
    key: &str,
    value: &Expression,
    file: &ParsedFile,
    diags: &mut Vec<Diag>,
) {
    match each_ref(expr) {
        EachRef::Key => {
            replace_preserving_decor(expr, Expression::from(key.to_string()));
            return;
        }
        EachRef::Value => {
            replace_preserving_decor(expr, value.clone());
            return;
        }
        EachRef::ValueField(fields) => {
            match lookup_value_field(value, &fields) {
                Ok(found) => replace_preserving_decor(expr, found.clone()),
                Err(message) => diags.push(Diag::new(
                    file.src.clone(),
                    span_of(expr.span()),
                    message,
                )),
            }
            return;
        }
        EachRef::Unsupported(path) => {
            diags.push(Diag::new(
                file.src.clone(),
                span_of(expr.span()),
                format!(
                    "unsupported reference '{path}'; only each.key and each.value are available inside a for_each resource"
                ),
            ));
            return;
        }
        EachRef::None => {}
    }
    walk_subexpressions(expr, &mut |sub| substitute_expr(sub, key, value, file, diags));
}

/// Visit every expression nested inside `expr` that a substitution could reach.
/// Shared by `each.*` expansion and `ad_template` input binding so the two
/// cannot drift on which positions they reach into.
pub(crate) fn walk_subexpressions(
    expr: &mut Expression,
    visit: &mut impl FnMut(&mut Expression),
) {
    match expr {
        Expression::Array(arr) => {
            for item in arr.iter_mut() {
                visit(item);
            }
        }
        Expression::Object(obj) => {
            for (_, item) in obj.iter_mut() {
                visit(item.expr_mut());
            }
        }
        Expression::StringTemplate(template) => {
            for element in template.iter_mut() {
                if let hcl_edit::template::Element::Interpolation(interp) = element {
                    visit(&mut interp.expr);
                }
            }
        }
        Expression::FuncCall(call) => {
            for arg in call.args.iter_mut() {
                visit(arg);
            }
        }
        Expression::Parenthesis(p) => {
            visit(p.inner_mut());
        }
        // `google_ads_callout_asset.co[each.key].id` — the index carries the
        // key that picks which generated instance is meant, so it has to be
        // substituted like any other occurrence.
        Expression::Traversal(t) => {
            for op in t.operators.iter_mut() {
                if let TraversalOperator::Index(inner) = op.value_mut() {
                    visit(inner);
                }
            }
        }
        _ => {}
    }
}

/// The meta-attribute that binds an `ad_template`'s parameters at the point of
/// use.
pub const TEMPLATE_INPUTS_ATTR: &str = "inputs";

/// The `<name>` in an `input.<name>` reference, else `None`.
fn input_ref_name(expr: &Expression) -> Option<String> {
    let Expression::Traversal(t) = expr else {
        return None;
    };
    let path = extract_traversal_path(t)?;
    if path.len() != 2 || path[0] != "input" {
        return None;
    }
    Some(path[1].clone())
}

/// The parameter names a template body references, in declaration order of
/// first use. A template declares its parameters by using them — there is no
/// second list to keep in sync, and no way for the two to disagree.
pub fn template_params(block: &Block) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    collect_params_body(&block.body, &mut out);
    out
}

fn collect_params_body(body: &Body, out: &mut Vec<String>) {
    for s in body.iter() {
        match s {
            Structure::Attribute(a) => collect_params_expr(&a.value, out),
            Structure::Block(b) => collect_params_body(&b.body, out),
        }
    }
}

fn collect_params_expr(expr: &Expression, out: &mut Vec<String>) {
    if let Some(name) = input_ref_name(expr) {
        if !out.contains(&name) {
            out.push(name);
        }
        return;
    }
    // The walker needs &mut; params are read-only, so walk a throwaway clone.
    let mut scratch = expr.clone();
    walk_subexpressions(&mut scratch, &mut |sub| collect_params_expr(sub, out));
}

/// Whether an expression reaches for a template parameter anywhere inside it.
pub fn uses_template_input(expr: &Expression) -> bool {
    if input_ref_name(expr).is_some() {
        return true;
    }
    let mut found = false;
    let mut scratch = expr.clone();
    walk_subexpressions(&mut scratch, &mut |sub| {
        if uses_template_input(sub) {
            found = true;
        }
    });
    found
}

/// A template body with every `input.<name>` replaced by the bound expression.
/// Names with no binding are left in place — the caller reports them against
/// the use site, where the author can actually fix them.
pub fn bind_template_inputs(
    block: &Block,
    inputs: &std::collections::HashMap<String, Expression>,
) -> Block {
    let mut out = block.clone();
    out.body = bind_body(&out.body, inputs);
    out
}

fn bind_body(body: &Body, inputs: &std::collections::HashMap<String, Expression>) -> Body {
    let mut out = Body::new();
    for s in body.iter() {
        match s {
            Structure::Attribute(a) => {
                let mut attr = a.clone();
                bind_expr(&mut attr.value, inputs);
                out.push(attr);
            }
            Structure::Block(b) => {
                let mut block = b.clone();
                block.body = bind_body(&b.body, inputs);
                out.push(block);
            }
        }
    }
    out
}

fn bind_expr(expr: &mut Expression, inputs: &std::collections::HashMap<String, Expression>) {
    if let Some(name) = input_ref_name(expr) {
        if let Some(bound) = inputs.get(&name) {
            replace_preserving_decor(expr, bound.clone());
        }
        return;
    }
    walk_subexpressions(expr, &mut |sub| bind_expr(sub, inputs));
}

fn replace_preserving_decor(slot: &mut Expression, mut replacement: Expression) {
    *replacement.decor_mut() = slot.decor().clone();
    *slot = replacement;
}

enum EachRef {
    None,
    Key,
    Value,
    /// `each.value.<field>[.<field>…]` — the entry's value is an object and
    /// this names a field inside it, so one map can carry a whole record
    /// (a sitelink's text, url, and two descriptions) instead of one scalar.
    ValueField(Vec<String>),
    Unsupported(String),
}

fn each_ref(expr: &Expression) -> EachRef {
    let Expression::Traversal(t) = expr else {
        return EachRef::None;
    };
    let Some(path) = extract_traversal_path(t) else {
        return EachRef::None;
    };
    if path.first().map(String::as_str) != Some("each") {
        return EachRef::None;
    }
    match path.get(1).map(String::as_str) {
        Some("key") if path.len() == 2 => EachRef::Key,
        Some("value") if path.len() == 2 => EachRef::Value,
        Some("value") => EachRef::ValueField(path[2..].to_vec()),
        _ => EachRef::Unsupported(path.join(".")),
    }
}

/// Walk `each.value.a.b` into the entry's value. Errors describe what the entry
/// actually holds — a missing field is far more often a typo than a design
/// change, and the available names are the fastest way to see which.
fn lookup_value_field<'a>(
    value: &'a Expression,
    fields: &[String],
) -> Result<&'a Expression, String> {
    let mut current = value;
    for (depth, field) in fields.iter().enumerate() {
        let so_far = || {
            if depth == 0 {
                "each.value".to_string()
            } else {
                format!("each.value.{}", fields[..depth].join("."))
            }
        };
        let Expression::Object(obj) = current else {
            return Err(format!(
                "each.value.{} needs {} to be an object, but it is {}",
                fields.join("."),
                so_far(),
                crate::schema::describe_expr(current),
            ));
        };
        let found = obj
            .iter()
            .find(|(k, _)| object_key_str(k).as_deref() == Some(field.as_str()))
            .map(|(_, v)| v.expr());
        match found {
            Some(v) => current = v,
            None => {
                let mut available: Vec<String> =
                    obj.iter().filter_map(|(k, _)| object_key_str(k)).collect();
                available.sort();
                let has = if available.is_empty() {
                    "it is empty".to_string()
                } else {
                    format!("it has {}", available.join(", "))
                };
                return Err(format!("{} has no field '{field}' — {has}", so_far()));
            }
        }
    }
    Ok(current)
}

pub(crate) fn object_key_str(key: &ObjectKey) -> Option<String> {
    if let Some(ident) = key.as_ident() {
        return Some(ident.as_str().to_string());
    }
    if let ObjectKey::Expression(Expression::String(s)) = key {
        return Some(s.as_str().to_string());
    }
    None
}

fn span_of(s: Option<std::ops::Range<usize>>) -> std::ops::Range<usize> {
    s.unwrap_or(0..0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_str;
    use std::path::Path;

    fn expand_str(name: &str, content: &str) -> (Vec<ParsedFile>, Vec<Diag>) {
        let pf = parse_str(Path::new(&format!("{name}.bid")), content).expect("parse");
        expand_resource_for_each(std::slice::from_ref(&pf), &InputBindings::default())
    }

    fn resource_labels(file: &ParsedFile) -> Vec<String> {
        file.body
            .iter()
            .filter_map(|s| match s {
                Structure::Block(b) if b.ident.as_str() == "resource" => {
                    Some(b.labels[1].as_str().to_string())
                }
                _ => None,
            })
            .collect()
    }

    fn errors(diags: &[Diag]) -> Vec<String> {
        diags
            .iter()
            .filter(|d| d.is_error())
            .map(|d| d.message.clone())
            .collect()
    }

    const DEVICES: &str = r#"
resource "google_ads_campaign_criterion" "t_devices" {
  for_each = ["MOBILE", "TABLET"]
  campaign = google_ads_campaign.t.id
  bid_modifier = 0

  device {
    type = each.value
  }
}
"#;

    #[test]
    fn list_form_fans_out_with_keyed_labels() {
        let (files, diags) = expand_str("devices", DEVICES);
        assert!(errors(&diags).is_empty(), "{:?}", errors(&diags));
        assert_eq!(
            resource_labels(&files[0]),
            vec!["t_devices[\"MOBILE\"]", "t_devices[\"TABLET\"]"]
        );
        let rendered = files[0].body.to_string();
        assert!(rendered.contains("type = \"MOBILE\""), "{rendered}");
        assert!(rendered.contains("type = \"TABLET\""), "{rendered}");
        assert!(!rendered.contains("for_each"), "{rendered}");
        assert!(!rendered.contains("each.value"), "{rendered}");
    }

    #[test]
    fn map_form_substitutes_key_and_value() {
        let (files, diags) = expand_str(
            "assets",
            r#"
resource "google_ads_campaign_asset" "sitelinks" {
  for_each = {
    neverconsent = google_ads_sitelink_asset.sl_neverconsent.id
    adblock = google_ads_sitelink_asset.sl_adblock.id
  }
  campaign = google_ads_campaign.c.id
  asset = each.value
  field_type = "SITELINK"
}
"#,
        );
        assert!(errors(&diags).is_empty(), "{:?}", errors(&diags));
        assert_eq!(
            resource_labels(&files[0]),
            vec!["sitelinks[\"neverconsent\"]", "sitelinks[\"adblock\"]"]
        );
        let rendered = files[0].body.to_string();
        assert!(
            rendered.contains("asset = google_ads_sitelink_asset.sl_neverconsent.id"),
            "{rendered}"
        );
        assert!(
            rendered.contains("asset = google_ads_sitelink_asset.sl_adblock.id"),
            "{rendered}"
        );
    }

    #[test]
    fn each_key_substitutes_inside_templates() {
        let (files, diags) = expand_str(
            "tmpl",
            r#"
resource "google_ads_campaign_budget" "b" {
  for_each = ["alpha"]
  name = "Budget ${each.key}"
  amount_micros = 1000000
}
"#,
        );
        assert!(errors(&diags).is_empty(), "{:?}", errors(&diags));
        let rendered = files[0].body.to_string();
        assert!(rendered.contains("alpha"), "{rendered}");
        assert!(!rendered.contains("each.key"), "{rendered}");
    }

    #[test]
    fn for_each_from_a_local_list() {
        let (files, diags) = expand_str(
            "local_list",
            r#"
locals {
  excluded_devices = ["MOBILE", "TABLET"]
}

resource "google_ads_campaign_criterion" "t_devices" {
  for_each = local.excluded_devices
  campaign = google_ads_campaign.t.id
  bid_modifier = 0

  device {
    type = each.value
  }
}
"#,
        );
        assert!(errors(&diags).is_empty(), "{:?}", errors(&diags));
        assert_eq!(
            resource_labels(&files[0]),
            vec!["t_devices[\"MOBILE\"]", "t_devices[\"TABLET\"]"]
        );
    }

    #[test]
    fn scalar_for_each_errors() {
        let (_files, diags) = expand_str(
            "scalar",
            r#"
resource "google_ads_campaign_budget" "b" {
  for_each = "oops"
  name = "B"
  amount_micros = 1000000
}
"#,
        );
        assert!(
            errors(&diags)
                .iter()
                .any(|m| m.contains("for_each must be a list of strings or a map")),
            "{:?}",
            errors(&diags)
        );
    }

    #[test]
    fn empty_for_each_errors() {
        let (_files, diags) = expand_str(
            "empty",
            r#"
resource "google_ads_campaign_budget" "b" {
  for_each = []
  name = "B"
  amount_micros = 1000000
}
"#,
        );
        assert!(
            errors(&diags).iter().any(|m| m.contains("for_each is empty")),
            "{:?}",
            errors(&diags)
        );
    }

    #[test]
    fn duplicate_list_entries_error() {
        let (_files, diags) = expand_str(
            "dup",
            r#"
resource "google_ads_campaign_budget" "b" {
  for_each = ["MOBILE", "MOBILE"]
  name = "B"
  amount_micros = 1000000
}
"#,
        );
        assert!(
            errors(&diags)
                .iter()
                .any(|m| m.contains("duplicate entry \"MOBILE\"")),
            "{:?}",
            errors(&diags)
        );
    }

    #[test]
    fn unsupported_each_attribute_errors() {
        let (_files, diags) = expand_str(
            "each_attr",
            r#"
resource "google_ads_campaign_budget" "b" {
  for_each = ["a"]
  name = each.other
  amount_micros = 1000000
}
"#,
        );
        assert!(
            errors(&diags)
                .iter()
                .any(|m| m.contains("only each.key and each.value are available")),
            "{:?}",
            errors(&diags)
        );
    }

    #[test]
    fn a_field_of_an_object_valued_entry_substitutes() {
        // One map entry carrying a whole record, so N sitelinks fan out from one
        // block instead of one resource each (issue #145).
        let (files, diags) = expand_str(
            "each_value_field",
            r#"
resource "google_ads_sitelink_asset" "sl" {
  for_each = {
    howto  = { text = "How it works", url = "https://example.com/how" }
    chrome = { text = "For Chrome", url = "https://example.com/chrome" }
  }
  link_text  = each.value.text
  final_urls = [each.value.url]
}
"#,
        );
        assert!(errors(&diags).is_empty(), "{:?}", errors(&diags));
        let rendered = files[0].body.to_string();
        assert!(rendered.contains(r#"link_text  = "How it works""#), "{rendered}");
        assert!(rendered.contains(r#"link_text  = "For Chrome""#), "{rendered}");
        assert!(
            rendered.contains(r#"final_urls = ["https://example.com/chrome"]"#),
            "{rendered}"
        );
        assert_eq!(
            resource_labels(&files[0]),
            vec![r#"sl["howto"]"#.to_string(), r#"sl["chrome"]"#.to_string()],
        );
    }

    #[test]
    fn a_missing_field_names_the_ones_the_entry_has() {
        let (_files, diags) = expand_str(
            "each_value_typo",
            r#"
resource "google_ads_sitelink_asset" "sl" {
  for_each = {
    howto = { text = "How it works", url = "https://example.com/how" }
  }
  link_text = each.value.tex
}
"#,
        );
        let errs = errors(&diags);
        assert!(
            errs.iter().any(|m| m.contains("has no field 'tex'")
                && m.contains("text")
                && m.contains("url")),
            "{errs:?}"
        );
    }

    #[test]
    fn a_field_lookup_on_a_scalar_entry_says_so() {
        // A list `for_each` binds each.value to a string; asking for a field of
        // it is a mistake worth naming precisely.
        let (_files, diags) = expand_str(
            "each_value_scalar",
            r#"
resource "google_ads_campaign_budget" "b" {
  for_each = ["a"]
  name = each.value.name
  amount_micros = 1000000
}
"#,
        );
        let errs = errors(&diags);
        assert!(
            errs.iter()
                .any(|m| m.contains("needs each.value to be an object") && m.contains("string")),
            "{errs:?}"
        );
    }

    #[test]
    fn an_instance_of_another_for_each_can_be_referenced_by_key() {
        // Previously you could fan out an asset or its attachment, never both
        // (DECISIONS.md deferred this with issue #86).
        let (files, diags) = expand_str(
            "keyed_ref",
            r#"
resource "google_ads_callout_asset" "co" {
  for_each      = ["fast", "free"]
  callout_text  = each.key
}

resource "google_ads_campaign_asset" "co_link" {
  for_each   = ["fast", "free"]
  campaign   = google_ads_campaign.c.id
  asset      = google_ads_callout_asset.co[each.key].id
  field_type = "CALLOUT"
}
"#,
        );
        assert!(errors(&diags).is_empty(), "{:?}", errors(&diags));
        let rendered = files[0].body.to_string();
        assert!(
            rendered.contains(r#"google_ads_callout_asset.co["fast"].id"#),
            "{rendered}"
        );
        assert!(
            rendered.contains(r#"google_ads_callout_asset.co["free"].id"#),
            "{rendered}"
        );
    }

    #[test]
    fn files_without_for_each_pass_through() {
        let (files, diags) = expand_str(
            "plain",
            r#"
resource "google_ads_campaign_budget" "b" {
  name = "B"
  amount_micros = 1000000
}
"#,
        );
        assert!(diags.is_empty());
        assert_eq!(resource_labels(&files[0]), vec!["b"]);
    }
}
