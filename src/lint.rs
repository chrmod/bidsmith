use hcl_edit::Span;
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
}

fn lint_rsa(file: &ParsedFile, rsa: &Block, address: &str, diags: &mut Vec<Diag>) {
    lint_rsa_block(file, rsa, address, "headline", MIN_HEADLINES, diags);
    lint_rsa_block(file, rsa, address, "description", MIN_DESCRIPTIONS, diags);
}

fn lint_rsa_block(
    file: &ParsedFile,
    rsa: &Block,
    address: &str,
    label: &str,
    minimum: usize,
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

    if blocks.len() < minimum {
        diags.push(Diag::warning(
            file.src.clone(),
            span_of(rsa.ident.span()),
            format!(
                "responsive_search_ad in {address} has only {n} {label}{plural} — Google Ads requires at least {minimum}",
                n = blocks.len(),
                plural = if blocks.len() == 1 { "" } else { "s" },
            ),
        ));
    }

    for (idx, b) in blocks.iter().enumerate() {
        let Some(text_attr) = find_attr(&b.body, "text") else {
            continue;
        };
        let Some(text) = text_attr.value.as_str() else {
            continue;
        };
        if looks_like_phone(text) {
            diags.push(Diag::warning(
                file.src.clone(),
                span_of(text_attr.key.span()),
                format!(
                    "{label}[{idx}] in {address} looks like a phone number; Google Ads policy disallows phone numbers in ad copy (use call extensions)"
                ),
            ));
        }
    }
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
