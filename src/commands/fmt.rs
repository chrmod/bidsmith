use std::path::{Path, PathBuf};
use std::process::ExitCode;

use hcl_edit::expr::{Expression, Traversal, TraversalOperator};
use hcl_edit::structure::{Block, Body, Structure};

use crate::parser::parse_file;

pub fn run(target: &str, check: bool) -> ExitCode {
    let target = Path::new(target);
    if !target.exists() {
        eprintln!("No such file or directory: {}", target.display());
        return ExitCode::from(1);
    }

    let files: Vec<PathBuf> = if target.is_file() {
        vec![target.to_path_buf()]
    } else {
        match collect_bid_files(target) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
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
        let canonical = format_body(&parsed.body);
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

fn collect_bid_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    walk(dir, &mut out).map_err(|e| format!("failed to walk {}: {e}", dir.display()))?;
    out.sort();
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == "node_modules" || name_str == "target" {
            continue;
        }
        let path = entry.path();
        let ft = entry.file_type()?;
        if ft.is_dir() {
            walk(&path, out)?;
        } else if ft.is_file() && path.extension().and_then(|e| e.to_str()) == Some("bid") {
            out.push(path);
        }
    }
    Ok(())
}

pub fn format_body(body: &Body) -> String {
    let mut out = String::new();
    emit_body(body, 0, &mut out);
    out
}

fn emit_body(body: &Body, indent: usize, out: &mut String) {
    let structures: Vec<&Structure> = body.iter().collect();
    for (i, s) in structures.iter().enumerate() {
        if i > 0 {
            let prev = structures[i - 1];
            if needs_blank_line(prev, s) {
                out.push('\n');
            }
        }
        emit_structure(s, indent, out);
    }
}

fn needs_blank_line(prev: &Structure, next: &Structure) -> bool {
    !matches!((prev, next), (Structure::Attribute(_), Structure::Attribute(_)))
}

fn emit_structure(s: &Structure, indent: usize, out: &mut String) {
    match s {
        Structure::Attribute(a) => {
            write_indent(out, indent);
            out.push_str(a.key.as_str());
            out.push_str(" = ");
            emit_expr(&a.value, indent, out);
            out.push('\n');
        }
        Structure::Block(b) => emit_block(b, indent, out),
    }
}

fn emit_block(b: &Block, indent: usize, out: &mut String) {
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
    out.push_str(" {\n");
    emit_body(&b.body, indent + 1, out);
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
