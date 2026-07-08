use hcl_edit::Span;
use hcl_edit::expr::Expression;
use hcl_edit::structure::{Attribute, Block, Body, Structure};

use crate::diagnostics::Diag;
use crate::parser::ParsedFile;
use crate::schema::{Bindings, InputBindings};

const MIN_HEADLINES: usize = 3;
const MIN_DESCRIPTIONS: usize = 2;
const MAX_HEADLINE_LEN: usize = 30;
const MAX_DESCRIPTION_LEN: usize = 90;
const MAX_PATH_LEN: usize = 15;

const SUSPICIOUS_LANGUAGE_CONSTANTS: &[(&str, &str, &str)] = &[
    (
        "languageConstants/1045",
        "Afar",
        "did you mean languageConstants/1030 (Polish)?",
    ),
];

pub fn lint_files(files: &[ParsedFile], inputs: &InputBindings) -> Vec<Diag> {
    // Resolve list-valued `local`/`var` refs so `headlines = local.set` is counted, not seen as zero; build diags belong to `validate`.
    let (bindings, _) = Bindings::build(files, inputs);
    let mut diags = Vec::new();
    for f in files {
        lint_file(f, &bindings, &mut diags);
    }
    diags
}

fn lint_file(file: &ParsedFile, bindings: &Bindings, diags: &mut Vec<Diag>) {
    for s in file.body.iter() {
        let Structure::Block(b) = s else { continue };
        match b.ident.as_str() {
            "resource" if b.labels.len() == 2 => lint_resource(file, b, bindings, diags),
            // Lint the template's RSA once at its declaration; referencing ad_group_ads carry no `ad` block.
            "ad_template" if b.labels.len() == 1 => {
                if let Some(rsa) = find_block(&b.body, "responsive_search_ad") {
                    let address = format!("ad_template.{}", b.labels[0].as_str());
                    lint_rsa(file, rsa, &address, bindings, diags);
                }
            }
            _ => {}
        }
    }
}

fn lint_resource(file: &ParsedFile, block: &Block, bindings: &Bindings, diags: &mut Vec<Diag>) {
    let ty = block.labels[0].as_str();
    let name = block.labels[1].as_str();
    let address = format!("{ty}.{name}");

    if ty == "google_ads_ad_group_ad" {
        if let Some(ad_block) = find_block(&block.body, "ad") {
            if let Some(rsa_block) = find_block(&ad_block.body, "responsive_search_ad") {
                lint_rsa(file, rsa_block, &address, bindings, diags);
            }
            if let Some(video_block) = find_block(&ad_block.body, "video_responsive_ad") {
                diags.push(Diag::warning(
                    file.src.clone(),
                    span_of(video_block.ident.span()),
                    format!(
                        "{address} declares a video ad: bidsmith records the creative but `plan`/`apply` do not create or update video ad creatives on the live account — build/attach the video ad in the Google Ads UI, then adopt the campaign scaffold with `bidsmith refresh`"
                    ),
                ));
            }
        }
        // path1/path2 set at the resource level override a template's RSA paths; lint them too.
        lint_rsa_paths(file, &block.body, &address, diags);
    }

    if ty == "google_ads_youtube_video_asset" {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "{address} references a YouTube video by id: bidsmith cannot upload video files — the video must already be published on your YouTube channel (upload via YouTube Studio or the YouTube Data API). `plan`/`apply` do not create or link video assets on the live account yet; attach the creative in the Google Ads UI and adopt the scaffold with `bidsmith refresh`"
            ),
        ));
    }

    if ty == "google_ads_campaign_criterion" {
        if let Some(lang_block) = find_block(&block.body, "language") {
            lint_language(file, lang_block, &address, diags);
        }
    }
}

fn lint_language(file: &ParsedFile, block: &Block, address: &str, diags: &mut Vec<Diag>) {
    let Some(constant_attr) = find_attr(&block.body, "language_constant") else {
        return;
    };
    let Some(value) = constant_attr.value.as_str() else {
        return;
    };
    for (constant, name, hint) in SUSPICIOUS_LANGUAGE_CONSTANTS {
        if value == *constant {
            diags.push(Diag::warning(
                file.src.clone(),
                span_of(constant_attr.value.span()),
                format!(
                    "language_constant '{value}' in {address} is {name}, a rarely-targeted language — {hint}"
                ),
            ));
        }
    }
}

fn lint_rsa(file: &ParsedFile, rsa: &Block, address: &str, bindings: &Bindings, diags: &mut Vec<Diag>) {
    lint_rsa_block(
        file,
        rsa,
        address,
        "headline",
        MIN_HEADLINES,
        MAX_HEADLINE_LEN,
        bindings,
        diags,
    );
    lint_rsa_block(
        file,
        rsa,
        address,
        "description",
        MIN_DESCRIPTIONS,
        MAX_DESCRIPTION_LEN,
        bindings,
        diags,
    );
    lint_rsa_paths(file, &rsa.body, address, diags);
}

fn lint_rsa_paths(file: &ParsedFile, body: &Body, address: &str, diags: &mut Vec<Diag>) {
    for name in ["path1", "path2"] {
        let Some(attr) = find_attr(body, name) else {
            continue;
        };
        let Some(value) = attr.value.as_str() else {
            continue;
        };
        if value.chars().count() > MAX_PATH_LEN {
            diags.push(Diag::warning(
                file.src.clone(),
                span_of(attr.value.span()),
                format!(
                    "{name} in {address} is {len} characters; Google Ads truncates at {MAX_PATH_LEN}",
                    len = value.chars().count(),
                ),
            ));
        }
        if !value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            diags.push(Diag::warning(
                file.src.clone(),
                span_of(attr.value.span()),
                format!(
                    "{name} in {address} contains characters outside [a-z0-9-]; Google Ads display URLs only render lowercase letters, digits, and hyphens"
                ),
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn lint_rsa_block(
    file: &ParsedFile,
    rsa: &Block,
    address: &str,
    label: &str,
    minimum: usize,
    max_len: usize,
    bindings: &Bindings,
    diags: &mut Vec<Diag>,
) {
    let blocks: Vec<&Block> = rsa
        .body
        .iter()
        .filter_map(|s| match s {
            Structure::Block(b) if b.ident.as_str() == label => Some(b),
            _ => None,
        })
        .collect();

    let list_attr_name = match label {
        "headline" => "headlines",
        "description" => "descriptions",
        _ => "",
    };
    let list_items: Vec<(Option<std::ops::Range<usize>>, Option<String>)> =
        find_attr(&rsa.body, list_attr_name)
            .map(|attr| collect_rsa_list_items(&attr.value, bindings, &file.module))
            .unwrap_or_default();

    let total = blocks.len() + list_items.len();
    if total < minimum {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(rsa.ident.span()),
            format!(
                "responsive_search_ad in {address} has only {total} {label}{plural} — Google Ads requires at least {minimum}",
                plural = if total == 1 { "" } else { "s" },
            ),
        ));
    }

    let mut idx = 0;
    for b in blocks.iter() {
        let Some(text_attr) = find_attr(&b.body, "text") else {
            idx += 1;
            continue;
        };
        let Some(text) = text_attr.value.as_str() else {
            idx += 1;
            continue;
        };
        lint_rsa_text(
            file,
            address,
            label,
            idx,
            text,
            max_len,
            span_of(text_attr.key.span()),
            span_of(text_attr.value.span()),
            diags,
        );
        idx += 1;
    }
    for (span, text) in &list_items {
        let Some(text) = text else {
            idx += 1;
            continue;
        };
        let s = span_of(span.clone());
        lint_rsa_text(
            file,
            address,
            label,
            idx,
            text,
            max_len,
            s.clone(),
            s,
            diags,
        );
        idx += 1;
    }
}

fn lint_rsa_text(
    file: &ParsedFile,
    address: &str,
    label: &str,
    idx: usize,
    text: &str,
    max_len: usize,
    key_span: std::ops::Range<usize>,
    value_span: std::ops::Range<usize>,
    diags: &mut Vec<Diag>,
) {
    if looks_like_phone(text) {
        diags.push(Diag::warning(
            file.src.clone(),
            key_span,
            format!(
                "{label}[{idx}] in {address} looks like a phone number; Google Ads policy disallows phone numbers in ad copy (use call extensions)"
            ),
        ));
    }
    let char_count = text.chars().count();
    if char_count > max_len {
        diags.push(Diag::warning(
            file.src.clone(),
            value_span,
            format!(
                "{label}[{idx}] in {address} is {char_count} characters; Google Ads rejects {label}s over {max_len}"
            ),
        ));
    }
}

fn collect_rsa_list_items(
    expr: &Expression,
    bindings: &Bindings,
    module: &str,
) -> Vec<(Option<std::ops::Range<usize>>, Option<String>)> {
    let resolved = bindings.resolve_value(module, expr);
    // A referenced list's item spans point into the (maybe other-file) declaration, so fall back to the use-site span.
    let from_ref = !std::ptr::eq(resolved, expr);
    let fallback = expr.span();
    let Expression::Array(arr) = resolved else {
        return Vec::new();
    };
    arr.iter()
        .map(|item| {
            let span = if from_ref { fallback.clone() } else { item.span() };
            let text = match bindings.resolve_value(module, item) {
                Expression::String(s) => Some(s.as_str().to_string()),
                Expression::Object(obj) => {
                    let mut text = None;
                    for (key, value) in obj.iter() {
                        let Some(ident) = key.as_ident() else { continue };
                        if ident.as_str() == "text" {
                            if let Expression::String(s) = bindings.resolve_value(module, value.expr())
                            {
                                text = Some(s.as_str().to_string());
                            }
                        }
                    }
                    text
                }
                _ => None,
            };
            (span, text)
        })
        .collect()
}

fn looks_like_phone(s: &str) -> bool {
    let mut run = 0usize;
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            run += 1;
            if run >= 7 {
                return true;
            }
        } else if matches!(ch, ' ' | '-' | '.' | '(' | ')' | '+' | '\u{a0}') {
            continue;
        } else {
            run = 0;
        }
    }
    false
}

fn find_attr<'a>(body: &'a Body, name: &str) -> Option<&'a Attribute> {
    body.iter().find_map(|s| match s {
        Structure::Attribute(a) if a.key.as_str() == name => Some(a),
        _ => None,
    })
}

fn find_block<'a>(body: &'a Body, name: &str) -> Option<&'a Block> {
    body.iter().find_map(|s| match s {
        Structure::Block(b) if b.ident.as_str() == name => Some(b),
        _ => None,
    })
}

fn span_of(s: Option<std::ops::Range<usize>>) -> std::ops::Range<usize> {
    s.unwrap_or(0..0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_file;
    use std::io::Write;

    fn lint_str(name: &str, content: &str) -> Vec<String> {
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("bidsmith-lint-test-{name}.bid"));
        {
            let mut f = std::fs::File::create(&tmp).expect("create tmp");
            f.write_all(content.as_bytes()).expect("write tmp");
        }
        let pf = parse_file(&tmp).expect("parse");
        lint_files(std::slice::from_ref(&pf), &InputBindings::default())
            .iter()
            .map(|d| d.message.clone())
            .collect()
    }

    fn rsa(headlines: &str, descriptions: &str) -> String {
        format!(
            r#"
resource "google_ads_ad_group_ad" "rsa" {{
  ad_group = google_ads_ad_group.g.id

  ad {{
    final_urls = ["https://example.com"]
    responsive_search_ad {{
      headlines    = {headlines}
      descriptions = {descriptions}
    }}
  }}
}}
"#
        )
    }

    #[test]
    fn list_local_meets_rsa_minimums_no_warning() {
        let content = format!(
            r#"
locals {{
  headlines    = ["One Headline", "Two Headline", "Three Headline"]
  descriptions = ["First description", "Second description"]
}}
{}"#,
            rsa("local.headlines", "local.descriptions")
        );
        let msgs = lint_str("list_local_ok", &content);
        assert!(
            !msgs.iter().any(|m| m.contains("Google Ads requires at least")),
            "unexpected min-count warning: {msgs:?}"
        );
    }

    #[test]
    fn list_local_below_minimum_still_warns() {
        let content = format!(
            r#"
locals {{
  headlines    = ["Only One Headline"]
  descriptions = ["First description", "Second description"]
}}
{}"#,
            rsa("local.headlines", "local.descriptions")
        );
        let msgs = lint_str("list_local_short", &content);
        assert!(
            msgs.iter()
                .any(|m| m.contains("has only 1 headline") && m.contains("at least 3")),
            "expected min-count warning: {msgs:?}"
        );
    }

    #[test]
    fn thin_ad_template_warns_at_declaration() {
        let msgs = lint_str(
            "thin_template",
            r#"
ad_template "thin" {
  final_urls = ["https://example.com"]
  responsive_search_ad {
    headlines    = ["Only One"]
    descriptions = ["Just one description here"]
  }
}
"#,
        );
        assert!(
            msgs.iter().any(|m| m.contains("ad_template.thin")
                && m.contains("has only 1 headline")),
            "expected template min-headline warning: {msgs:?}"
        );
    }

    #[test]
    fn top_level_path_override_is_linted() {
        let msgs = lint_str(
            "path_override",
            r#"
resource "google_ads_ad_group_ad" "rsa" {
  ad_group = google_ads_ad_group.g.id
  template = ad_template.shared
  path1    = "Get_Started"
}
"#,
        );
        assert!(
            msgs.iter().any(|m| m.contains("path1 in google_ads_ad_group_ad.rsa")
                && m.contains("outside [a-z0-9-]")),
            "expected path-override charset warning: {msgs:?}"
        );
    }

    #[test]
    fn over_length_headline_in_referenced_list_warns() {
        let content = format!(
            r#"
locals {{
  headlines    = ["This headline is far too long for Google Ads to accept it", "Short Two", "Short Three"]
  descriptions = ["First description", "Second description"]
}}
{}"#,
            rsa("local.headlines", "local.descriptions")
        );
        let msgs = lint_str("list_local_long", &content);
        assert!(
            msgs.iter().any(|m| m.contains("characters; Google Ads rejects headlines over 30")),
            "expected length warning: {msgs:?}"
        );
    }
}
