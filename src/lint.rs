use hcl_edit::Span;
use hcl_edit::expr::Expression;
use hcl_edit::structure::{Attribute, Block, Body, Structure};

use crate::diagnostics::Diag;
use crate::parser::ParsedFile;

const STATUS_AWARE: &[&str] = &[
    "google_ads_campaign",
    "google_ads_ad_group",
    "google_ads_ad_group_ad",
    "google_ads_ad_group_criterion",
    "google_ads_campaign_criterion",
];

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

pub fn lint_files(files: &[ParsedFile]) -> Vec<Diag> {
    let mut diags = Vec::new();
    for f in files {
        lint_file(f, &mut diags);
    }
    diags
}

fn lint_file(file: &ParsedFile, diags: &mut Vec<Diag>) {
    for s in file.body.iter() {
        let Structure::Block(b) = s else { continue };
        if b.ident.as_str() != "resource" || b.labels.len() != 2 {
            continue;
        }
        lint_resource(file, b, diags);
    }
}

fn lint_resource(file: &ParsedFile, block: &Block, diags: &mut Vec<Diag>) {
    let ty = block.labels[0].as_str();
    let name = block.labels[1].as_str();
    let address = format!("{ty}.{name}");

    if STATUS_AWARE.contains(&ty) && find_attr(&block.body, "status").is_none() {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(block.ident.span()),
            format!(
                "{address} has no 'status' — defaults to ENABLED on apply; set it explicitly"
            ),
        ));
    }

    if ty == "google_ads_ad_group_ad" {
        if let Some(ad_block) = find_block(&block.body, "ad") {
            if let Some(rsa_block) = find_block(&ad_block.body, "responsive_search_ad") {
                lint_rsa(file, rsa_block, &address, diags);
            }
        }
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

fn lint_rsa(file: &ParsedFile, rsa: &Block, address: &str, diags: &mut Vec<Diag>) {
    lint_rsa_block(
        file,
        rsa,
        address,
        "headline",
        MIN_HEADLINES,
        MAX_HEADLINE_LEN,
        diags,
    );
    lint_rsa_block(
        file,
        rsa,
        address,
        "description",
        MIN_DESCRIPTIONS,
        MAX_DESCRIPTION_LEN,
        diags,
    );
    lint_rsa_paths(file, rsa, address, diags);
}

fn lint_rsa_paths(file: &ParsedFile, rsa: &Block, address: &str, diags: &mut Vec<Diag>) {
    for name in ["path1", "path2"] {
        let Some(attr) = find_attr(&rsa.body, name) else {
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

fn lint_rsa_block(
    file: &ParsedFile,
    rsa: &Block,
    address: &str,
    label: &str,
    minimum: usize,
    max_len: usize,
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
            .map(|attr| collect_rsa_list_items(&attr.value))
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
) -> Vec<(Option<std::ops::Range<usize>>, Option<String>)> {
    let Expression::Array(arr) = expr else {
        return Vec::new();
    };
    arr.iter()
        .map(|item| match item {
            Expression::String(s) => (item.span(), Some(s.as_str().to_string())),
            Expression::Object(obj) => {
                let mut text = None;
                for (key, value) in obj.iter() {
                    let Some(ident) = key.as_ident() else { continue };
                    if ident.as_str() == "text" {
                        if let Expression::String(s) = value.expr() {
                            text = Some(s.as_str().to_string());
                        }
                    }
                }
                (item.span(), text)
            }
            _ => (item.span(), None),
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
