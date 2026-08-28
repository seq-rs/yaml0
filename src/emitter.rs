use crate::BorrowedValue;
use crate::Result;
use crate::patterns::has_ctrl_chars;
use crate::patterns::has_newline;
use crate::patterns::needs_quotes;
use std::fmt::Write;

pub fn emit(v: &BorrowedValue<'_>) -> Result<String> {
    let mut out = String::new();
    emit_node(v, 0, &mut out)?;
    if out.starts_with('\n') {
        out.remove(0);
    }

    if !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(out)
}

fn emit_node(v: &BorrowedValue<'_>, indent: usize, out: &mut String) -> Result<()> {
    match v {
        BorrowedValue::Null => emit_null(out),
        BorrowedValue::Bool(b) => emit_bool(*b, out),
        BorrowedValue::Int(n) => emit_int(n, out),
        BorrowedValue::Float(f) => emit_float(f, out),
        BorrowedValue::String(s) => emit_scalar(s, indent, out),
        BorrowedValue::Tagged(tag, v) => {
            out.push_str(tag);
            if is_inline(v) {
                out.push(' ');
            }
            emit_node(v, indent, out)?;
        }
        BorrowedValue::Seq(items) => emit_block_seq(items, indent, out)?,
        BorrowedValue::Map(pairs) => emit_block_map(pairs, indent, out)?,
    }
    Ok(())
}

fn emit_null(out: &mut String) {
    out.push_str("null")
}

fn emit_bool(b: bool, out: &mut String) {
    out.push_str(if b { "true" } else { "false" });
}

fn emit_int(n: &i64, out: &mut String) {
    write!(out, "{n}").unwrap();
}

fn emit_float(f: &f64, out: &mut String) {
    if f.is_nan() {
        out.push_str(".nan");
        return;
    }

    if f.is_infinite() {
        out.push_str(if f.is_sign_positive() {
            ".inf"
        } else {
            "-.inf"
        });
        return;
    }

    let s = format!("{}", f);
    out.push_str(&s);
    if !s.contains('.') && !s.contains('e') {
        out.push_str(".0");
    }
}

fn emit_scalar(s: &str, indent: usize, out: &mut String) {
    if !s.is_empty() && has_newline(s) && !has_ctrl_chars(s) {
        emit_block_scalar(s, indent, out);
    } else if needs_quotes(s) {
        emit_quoted_scalar(s, out);
    } else {
        out.push_str(s);
    }
}

fn emit_block_scalar(s: &str, indent: usize, out: &mut String) {
    out.push('|');
    if !s.ends_with('\n') {
        out.push('-');
    }
    let child = " ".repeat(indent + 2);
    for line in s.split('\n') {
        out.push('\n');
        if !line.is_empty() {
            out.push_str(&child);
            out.push_str(line);
        }
    }
}

fn emit_quoted_scalar(s: &str, out: &mut String) {
    if !has_ctrl_chars(s) && !s.contains('\'') {
        out.push('\'');
        out.push_str(s);
        out.push('\'');
    } else {
        emit_double_quoted(s, out);
    }
}

fn emit_double_quoted(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                write!(out, "\\x{:02x}", c as u32).unwrap()
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn emit_block_seq(items: &[BorrowedValue<'_>], indent: usize, out: &mut String) -> Result<()> {
    if items.is_empty() {
        out.push_str("[]");
        return Ok(());
    }
    for item in items {
        out.push('\n');
        push_indent(out, indent);
        push_block_prefix(out);
        match item {
            BorrowedValue::Map(pairs) if !pairs.is_empty() => {
                emit_kv(&pairs[0], indent + 2, out)?;
                for kv in &pairs[1..] {
                    out.push('\n');
                    push_indent(out, indent + 2);
                    emit_kv(kv, indent + 2, out)?;
                }
            }
            _ => emit_node(item, indent + 2, out)?,
        }
    }
    Ok(())
}

fn emit_block_map(
    pairs: &[(BorrowedValue<'_>, BorrowedValue<'_>)],
    indent: usize,
    out: &mut String,
) -> Result<()> {
    if pairs.is_empty() {
        out.push('{');
        out.push('}');
        return Ok(());
    }
    for kv in pairs {
        out.push('\n');
        push_indent(out, indent);
        emit_kv(kv, indent, out)?;
    }
    Ok(())
}

fn emit_kv(
    kv: &(BorrowedValue<'_>, BorrowedValue<'_>),
    indent: usize,
    out: &mut String,
) -> Result<()> {
    let (k, v) = kv;
    if key_needs_explicit(k) {
        out.push('?');

        if !emits_leading_break(k) {
            out.push(' ');
        }
        emit_node(k, indent + 2, out)?;
        out.push('\n');
        push_indent(out, indent);
    } else {
        emit_node(k, indent, out)?;
    }
    out.push(':');
    if is_inline(v) {
        out.push(' ');
    }
    emit_node(v, indent + 2, out)?;
    Ok(())
}

fn push_indent(out: &mut String, indent: usize) {
    out.push_str(&" ".repeat(indent))
}

fn push_block_prefix(out: &mut String) {
    out.push('-');
    out.push(' ');
}

fn is_inline(v: &BorrowedValue<'_>) -> bool {
    match v {
        BorrowedValue::Seq(items) if !items.is_empty() => false,
        BorrowedValue::Map(items) if !items.is_empty() => false,
        BorrowedValue::Tagged(_, value) => is_inline(value),
        _ => true,
    }
}

fn key_needs_explicit(k: &BorrowedValue<'_>) -> bool {
    match k {
        BorrowedValue::String(s) => has_newline(s),
        BorrowedValue::Tagged(_, inner) => key_needs_explicit(inner),
        // flow nodes are legal implicit keys, but the parser does not read
        // them in key position yet
        BorrowedValue::Seq(_) | BorrowedValue::Map(_) => true,
        _ => false,
    }
}

/// True when [`emit_node`] opens its output with a line break
fn emits_leading_break(v: &BorrowedValue<'_>) -> bool {
    match v {
        BorrowedValue::Seq(items) => !items.is_empty(),
        BorrowedValue::Map(pairs) => !pairs.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Parser;
    use std::borrow::Cow;

    fn s(x: &str) -> BorrowedValue<'_> {
        BorrowedValue::String(Cow::Borrowed(x))
    }

    /// Emit, reparse, and assert the value survives; returns the emitted text
    fn roundtrip(v: &BorrowedValue<'_>) -> String {
        let out = emit(v).unwrap();
        match Parser::new(&out).parse() {
            Ok(back) => assert_eq!(&back, v, "roundtrip mismatch for {out:?}"),
            Err(e) => panic!("emitted YAML does not reparse: {out:?} -> {e}"),
        }
        out
    }

    #[test]
    fn simple_key_stays_implicit() {
        let v = BorrowedValue::Map(vec![(s("a"), BorrowedValue::Int(1))]);
        assert_eq!(roundtrip(&v), "a: 1\n");
    }

    #[test]
    fn seq_key_uses_explicit_form() {
        let v = BorrowedValue::Map(vec![(
            BorrowedValue::Seq(vec![BorrowedValue::Int(1), BorrowedValue::Int(2)]),
            s("v"),
        )]);
        assert_eq!(roundtrip(&v), "?\n  - 1\n  - 2\n: v\n");
    }

    #[test]
    fn map_key_uses_explicit_form() {
        let v = BorrowedValue::Map(vec![(
            BorrowedValue::Map(vec![(s("a"), BorrowedValue::Int(1))]),
            BorrowedValue::Int(9),
        )]);
        assert_eq!(roundtrip(&v), "?\n  a: 1\n: 9\n");
    }

    #[test]
    fn multiline_string_key_uses_explicit_form() {
        let v = BorrowedValue::Map(vec![(s("one\ntwo"), BorrowedValue::Int(1))]);
        assert_eq!(roundtrip(&v), "? |-\n    one\n    two\n: 1\n");
    }

    #[test]
    fn empty_container_key_uses_explicit_form() {
        // `[]: 1` is valid YAML, but the parser does not read flow nodes in
        // key position, so containers always take the explicit form
        let seq = BorrowedValue::Map(vec![(BorrowedValue::Seq(vec![]), BorrowedValue::Int(1))]);
        assert_eq!(roundtrip(&seq), "? []\n: 1\n");
        let map = BorrowedValue::Map(vec![(BorrowedValue::Map(vec![]), BorrowedValue::Int(1))]);
        assert_eq!(roundtrip(&map), "? {}\n: 1\n");
    }

    #[test]
    fn explicit_key_with_container_value() {
        let v = BorrowedValue::Map(vec![(
            BorrowedValue::Seq(vec![BorrowedValue::Int(1)]),
            BorrowedValue::Seq(vec![BorrowedValue::Int(2)]),
        )]);
        assert_eq!(roundtrip(&v), "?\n  - 1\n:\n  - 2\n");
    }

    #[test]
    fn explicit_key_inside_seq_item() {
        let v = BorrowedValue::Seq(vec![BorrowedValue::Map(vec![(
            BorrowedValue::Seq(vec![BorrowedValue::Int(1)]),
            BorrowedValue::Int(2),
        )])]);
        assert_eq!(roundtrip(&v), "- ?\n    - 1\n  : 2\n");
    }

    #[test]
    fn tagged_container_key_keeps_the_indicator_separated() {
        // the tag renders before the key body, so `?` still needs its space
        let v = BorrowedValue::Map(vec![(
            BorrowedValue::Tagged(
                Cow::Borrowed("!k"),
                Box::new(BorrowedValue::Seq(vec![BorrowedValue::Int(1)])),
            ),
            BorrowedValue::Int(2),
        )]);
        assert_eq!(roundtrip(&v), "? !k\n  - 1\n: 2\n");
    }
}
