use hcl_edit::expr::{Expression, ObjectKey};
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
        return ParsedFile {
            path: file.path.clone(),
            src: file.src.clone(),
            body: file.body.clone(),
            module: file.module.clone(),
        };
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
        path: file.path.clone(),
        src: file.src.clone(),
        body,
        module: file.module.clone(),
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
    match expr {
        Expression::Array(arr) => {
            for item in arr.iter_mut() {
                substitute_expr(item, key, value, file, diags);
            }
        }
        Expression::Object(obj) => {
            for (_, item) in obj.iter_mut() {
                substitute_expr(item.expr_mut(), key, value, file, diags);
            }
        }
        Expression::StringTemplate(template) => {
            for element in template.iter_mut() {
                if let hcl_edit::template::Element::Interpolation(interp) = element {
                    substitute_expr(&mut interp.expr, key, value, file, diags);
                }
            }
        }
        Expression::FuncCall(call) => {
            for arg in call.args.iter_mut() {
                substitute_expr(arg, key, value, file, diags);
            }
        }
        Expression::Parenthesis(p) => {
            substitute_expr(p.inner_mut(), key, value, file, diags);
        }
        _ => {}
    }
}

fn replace_preserving_decor(slot: &mut Expression, mut replacement: Expression) {
    *replacement.decor_mut() = slot.decor().clone();
    *slot = replacement;
}

enum EachRef {
    None,
    Key,
    Value,
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
        _ => EachRef::Unsupported(path.join(".")),
    }
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
  name = each.value.name
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
