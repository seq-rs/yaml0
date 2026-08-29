mod block_map;
mod block_scalar;
mod block_seq;
mod cursor;
mod escape;
mod flow;
mod scalar;

use std::{borrow::Cow, collections::HashMap};

use crate::{
    BorrowedValue, Result,
    borrowed_value::{apply_tag, resolve_merge_keys},
    patterns::resolve_scalar,
};

pub(super) enum ScalarToken<'a> {
    Plain(Cow<'a, str>),
    Quoted(Cow<'a, str>),
}

impl<'a> ScalarToken<'a> {
    pub(super) fn into_value(self) -> BorrowedValue<'a> {
        match self {
            Self::Plain(s) => resolve_scalar(s),
            Self::Quoted(s) => BorrowedValue::String(s),
        }
    }
}

/// YAML parser cursor over a borrowed source string.
///
/// Most callers should use [`from_str`](crate::from_str) /
/// [`to_string`](crate::to_string) and never instantiate `Parser` directly.
/// Reach for `Parser` when you need:
///
/// - **Multi-document streams** — [`Parser::parse_all`] returns
///   `Vec<BorrowedValue>` for inputs containing `---`-separated documents.
/// - **Manual inspection** — call [`Parser::parse`] and walk the
///   [`BorrowedValue`](crate::BorrowedValue) tree yourself before deserializing.
/// - **Zero-copy borrows** — keep the resulting `BorrowedValue` alive and use
///   [`from_value`](crate::from_value) so struct fields can be `&str`
///   slices of the input.
pub struct Parser<'a> {
    pub(super) src: &'a str,
    pub(super) pos: usize,
    /// 1-based line for errors
    pub(super) line: usize,
    /// 1-based column for errors
    pub(super) col: usize,
    pub(super) anchors: HashMap<&'a str, crate::BorrowedValue<'a>>,
}

impl<'a> Parser<'a> {
    pub fn new(src: &'a str) -> Self {
        // Strip leading UTF-8 BOM (U+FEFF, EF BB BF) per YAML 1.2 §5.2
        let src = src.strip_prefix('\u{FEFF}').unwrap_or(src);
        Self {
            src,
            pos: 0,
            line: 1,
            col: 1,
            anchors: HashMap::new(),
        }
    }

    /// Parse all documents in the YAML stream
    ///
    /// Input:
    /// ```yaml
    /// ---
    /// kind: Pod
    /// ---
    /// kind: Service
    /// ```
    ///
    /// Output: `vec![Map[kind: Pod], Map[kind: Service]]`
    ///
    /// Handles `---` start markers, `...` end markers, and `%`-prefix
    /// directives. Anchors are reset between documents per spec §9.1.
    pub fn parse_all(&mut self) -> Result<Vec<BorrowedValue<'a>>> {
        let mut docs = Vec::new();
        let mut allow_bare = true;

        loop {
            self.skip_blank_and_comment_lines();
            self.skip_directives();

            if self.at_eof() {
                break;
            }

            if self.at_doc_marker(b"---") {
                self.consume_doc_marker(3);
            } else if !allow_bare {
                let indent = self.current_indent()?;

                // point at content, not col 1
                for _ in 0..indent {
                    self.advance();
                }

                return Err(self.err("unexpected content; expected '---' to start a new document"));
            }

            // Spec-mandated reset of anchor scope at document boundaries
            self.anchors.clear();

            docs.push(resolve_merge_keys(self.parse_node(0)?));

            self.skip_blank_and_comment_lines();
            allow_bare = self.at_doc_marker(b"...");
            if allow_bare {
                self.consume_doc_marker(3);
            }
        }
        Ok(docs)
    }

    /// Parse a single-document YAML stream
    ///
    /// Input: `"a: 1\nb: 2\n"` → Output: `BorrowedValue::Map([("a", 1), ("b", 2)])`
    ///
    /// Errors if the stream contains more than one document. Returns
    /// `BorrowedValue::Null` on empty input.
    pub fn parse(&mut self) -> Result<BorrowedValue<'a>> {
        let mut docs = self.parse_all()?;
        match docs.len() {
            0 => Ok(BorrowedValue::Null),
            1 => Ok(docs.pop().unwrap()),
            n => Err(self.err(format!("expected single document, got {n}"))),
        }
    }

    /// Consume `%YAML`/`%TAG` directive lines (and any in-between blanks)
    fn skip_directives(&mut self) {
        while self.peek() == Some(b'%') {
            self.skip_to_next_line();
            self.skip_blank_and_comment_lines();
        }
    }

    /// Consume a doc marker (`---` or `...`) plus its trailing line break
    fn consume_doc_marker(&mut self, len: usize) {
        for _ in 0..len {
            self.advance();
        }
        self.skip_spaces();
        self.consume_one_line_break();
    }

    pub(super) fn is_doc_marker_at(&self, pos: usize, marker: &[u8]) -> bool {
        let b = self.src.as_bytes();
        b.len() >= pos + marker.len()
            && &b[pos..pos + marker.len()] == marker
            && matches!(
                b.get(pos + marker.len()),
                None | Some(b' ' | b'\t' | b'\n' | b'\r')
            )
    }

    /// Parse a single YAML node (scalar, seq, map, tagged, or empty)
    ///
    /// Input:
    /// ```yaml
    /// !mytag
    /// - a
    /// - b
    /// ```
    ///
    /// Output: `BorrowedValue::Tagged("!mytag", BorrowedValue::Seq([String("a"), String("b")]))`
    ///
    /// `min_indent` is the minimum indent the node must have to belong to the
    /// caller; content at lesser indent yields `BorrowedValue::Null`. Accepts cursor
    /// at line-start (skips blanks + checks indent) or mid-line (uses
    /// `col - 1` as effective indent).
    pub fn parse_node(&mut self, min_indent: usize) -> Result<BorrowedValue<'a>> {
        // Catch doc markers at cursor (covers entries from parse_all where
        // consume_doc_marker has just advanced past the line break)
        if self.at_doc_marker(b"---") || self.at_doc_marker(b"...") {
            return Ok(BorrowedValue::Null);
        }

        let indent = if self.at_line_end() {
            self.skip_blank_and_comment_lines();
            if self.at_eof() {
                return Ok(BorrowedValue::Null);
            }

            // Document boundary terminates a node — parse_all handles the marker
            if self.at_doc_marker(b"---") || self.at_doc_marker(b"...") {
                return Ok(BorrowedValue::Null);
            }

            let indent = self.current_indent()?;
            if indent < min_indent {
                return Ok(BorrowedValue::Null);
            }

            for _ in 0..indent {
                self.advance();
            }
            indent

            // we are at start of value already otherwise, no need to indent/skip
        } else {
            self.col.saturating_sub(1)
        };

        // tags and anchors prefix the node, for now just markers
        let mut tag: Option<Cow<'_, str>> = None;
        let mut anchor: Option<&'a str> = None;

        loop {
            let mut consumed = false;

            if tag.is_none()
                && let Some(t) = self.try_consume_tag()?
            {
                tag = Some(t);
                consumed = true;
            }

            if anchor.is_none()
                && let Some(a) = self.try_consume_anchor()?
            {
                anchor = Some(a);
                consumed = true;
            }

            if !consumed {
                break;
            }
        }

        let value = if (tag.is_some() || anchor.is_some()) && self.at_line_end() {
            self.parse_node(min_indent)?
        } else {
            self.dispatch(indent, min_indent)?
        };

        let value = match tag {
            Some(t) => apply_tag(t, value),
            None => value,
        };

        if let Some(name) = anchor {
            self.anchors.insert(name, value.clone());
        }

        Ok(value)
    }

    /// Route the cursor to the right node parser based on the next byte
    fn dispatch(&mut self, indent: usize, min_indent: usize) -> Result<BorrowedValue<'a>> {
        match self.peek() {
            None => Ok(BorrowedValue::Null),
            Some(b'[') | Some(b'{') => self.flow_node_or_map(indent),
            Some(b'|') => self.parse_block_scalar(),
            Some(b'>') => self.parse_block_scalar(),
            Some(b'*') => self.parse_alias(),
            Some(b'-') if self.at_seq_dash() => self.parse_block_seq(indent),
            _ => self.parse_scalar_or_map(indent, min_indent),
        }
    }

    /// Consume an anchor prefix (`&name`) if present
    ///
    /// Input:
    /// ```yaml
    /// &id 42
    /// ```
    ///
    /// Output: `Some("id")`, cursor positioned at `42`. Returns `None` (no
    /// advance) if the next byte isn't `&`. The name's lifetime is borrowed
    /// from source.
    fn try_consume_anchor(&mut self) -> Result<Option<&'a str>> {
        if self.peek() != Some(b'&') {
            return Ok(None);
        }

        self.advance();

        let start = self.pos;

        while let Some(b) = self.peek() {
            if matches!(
                b,
                b' ' | b'\t' | b'\n' | b'\r' | b',' | b'[' | b']' | b'{' | b'}'
            ) {
                break;
            }
            self.advance();
        }

        if self.pos == start {
            return Err(self.err("empty anchor name"));
        }

        let name = &self.src[start..self.pos];

        self.skip_spaces();

        Ok(Some(name))
    }

    /// Resolve a `*name` alias to a clone of the anchored value
    ///
    /// Input (with anchor `id` previously registered as `Int(42)`):
    /// ```yaml
    /// *id
    /// ```
    ///
    /// Output: `BorrowedValue::Int(42)` (cloned from the anchors map). Errors if
    /// the anchor name is unknown or empty.
    fn parse_alias(&mut self) -> Result<BorrowedValue<'a>> {
        self.advance(); // consume '*'

        let start = self.pos;
        while let Some(b) = self.peek() {
            if matches!(
                b,
                b' ' | b'\t' | b'\n' | b'\r' | b',' | b'[' | b']' | b'{' | b'}'
            ) {
                break;
            }
            self.advance();
        }

        if self.pos == start {
            return Err(self.err("empty alias name"));
        }

        let name = &self.src[start..self.pos];

        match self.anchors.get(name) {
            Some(v) => Ok(v.clone()),
            None => Err(self.err(format!("unknown anchor: '{name}'"))),
        }
    }

    /// Consume a YAML tag prefix if present
    ///
    /// Input:
    /// ```yaml
    /// !!str foo
    /// ```
    ///
    /// Output: `Some("!!str")`, cursor positioned at `foo`. Returns `None`
    /// (no advance) if the next byte isn't `!`. Recognizes `!`, `!!name`,
    /// and `!<verbatim>` forms.
    fn try_consume_tag(&mut self) -> Result<Option<Cow<'a, str>>> {
        if self.peek() != Some(b'!') {
            return Ok(None);
        }
        let start = self.pos;
        self.advance(); // consume first '!'

        if self.peek() == Some(b'<') {
            // verbatim form !<...>
            self.advance();
            while let Some(b) = self.peek() {
                self.advance();
                if b == b'>' {
                    break;
                }
            }
        } else {
            // local, secondary or bare tag, reading till whitespace/eof/flow indicator
            while let Some(b) = self.peek() {
                if matches!(
                    b,
                    b' ' | b'\t' | b'\n' | b'\r' | b',' | b'[' | b']' | b'{' | b'}'
                ) {
                    break;
                }
                self.advance();
            }
        }

        let tag = Cow::Borrowed(&self.src[start..self.pos]);
        // Whitespace between tag and node content
        self.skip_spaces();
        Ok(Some(tag))
    }

    /// Whether the next character is a `-` at e.g. start of a sequence
    fn at_seq_dash(&self) -> bool {
        self.is_seq_dash_at(self.pos)
    }

    /// Whether the character at position `pos` is a `-` at e.g. the start of a sequence
    fn is_seq_dash_at(&self, pos: usize) -> bool {
        self.is_block_indicator_at(pos, b'-')
    }

    fn is_block_indicator_at(&self, pos: usize, ind: u8) -> bool {
        self.peek_at(pos) == Some(ind)
            && matches!(
                self.peek_at(pos + 1),
                None | Some(b' ' | b'\t' | b'\n' | b'\r')
            )
    }

    fn at_explicit_key(&self) -> bool {
        self.is_block_indicator_at(self.pos, b'?')
    }

    fn finish_value_token(
        &mut self,
        tok: ScalarToken<'a>,
        min_indent: usize,
    ) -> Result<BorrowedValue<'a>> {
        Ok(match tok {
            ScalarToken::Plain(s) => resolve_scalar(self.fold_plain_continuation(s, min_indent)?),
            ScalarToken::Quoted(s) => BorrowedValue::String(s),
        })
    }

    /// Parse one scalar token (plain, double-quoted, or single-quoted)
    ///
    /// Input: `"42"` → Output: `BorrowedValue::String("42")` (quoted stays string)
    /// Input: `42`   → Output: `BorrowedValue::Int(42)` (plain resolves via `resolve_scalar`)
    fn parse_scalar_token(&mut self) -> Result<BorrowedValue<'a>> {
        Ok(self.read_scalar_token()?.into_value())
    }

    /// Parse a scalar token, either double-quoted, single-quoted or plain
    ///
    /// `"42"` => ScalarToken::Quoted(42)
    /// `'42'` => ScalarToken::Quoted(42)
    /// `42` => ScalarToken::Plain(42)
    fn read_scalar_token(&mut self) -> Result<ScalarToken<'a>> {
        match self.peek() {
            Some(b'"') => Ok(ScalarToken::Quoted(
                self.parse_double_quoted(self.col.saturating_sub(1))?,
            )),
            Some(b'\'') => Ok(ScalarToken::Quoted(
                self.parse_single_quoted(self.col.saturating_sub(1))?,
            )),
            _ => Ok(ScalarToken::Plain(self.parse_plain_scalar(false))),
        }
    }
}

#[inline(always)]
fn line_end(b: u8) -> bool {
    matches!(b, b'\n' | b'\r')
}

#[inline(always)]
fn whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t')
}

#[inline(always)]
fn space(b: u8) -> bool {
    matches!(b, b' ')
}

#[inline(always)]
fn tab(b: u8) -> bool {
    matches!(b, b'\t')
}

fn trim_trailing_whitespace_end(bytes: &[u8]) -> usize {
    let mut n = bytes.len();
    while n > 0 && matches!(bytes[n - 1], b' ' | b'\t') {
        n -= 1
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> BorrowedValue<'_> {
        Parser::new(src).parse().unwrap()
    }

    #[test]
    fn standard_str_tag_drops_wrapper() {
        let v = parse("!!str 42\n");
        assert!(matches!(v, BorrowedValue::String(s) if s == "42"));
    }

    #[test]
    fn standard_int_tag_coerces_quoted() {
        let v = parse("!!int \"5\"\n");
        assert!(matches!(v, BorrowedValue::Int(5)));
    }

    #[test]
    fn standard_int_tag_accepts_hex() {
        let v = parse("!!int 0xff\n");
        assert!(matches!(v, BorrowedValue::Int(255)));
    }

    #[test]
    fn standard_float_tag_promotes_int() {
        let v = parse("!!float 7\n");
        assert!(matches!(v, BorrowedValue::Float(f) if f == 7.0));
    }

    #[test]
    fn standard_null_tag_drops_inner() {
        let v = parse("!!null whatever\n");
        assert!(matches!(v, BorrowedValue::Null));
    }

    #[test]
    fn standard_bool_tag_rejects_yaml1_1_spelling() {
        // YES resolves to String("YES"); !!bool no-ops on it
        let v = parse("!!bool YES\n");
        assert!(matches!(v, BorrowedValue::String(s) if s == "YES"));
    }

    #[test]
    fn custom_tag_wraps() {
        let v = parse("!myapp/Thing foo\n");
        match v {
            BorrowedValue::Tagged(tag, inner) => {
                assert_eq!(tag, "!myapp/Thing");
                assert!(matches!(*inner, BorrowedValue::String(s) if s == "foo"));
            }
            other => panic!("expected Tagged, got {other:?}"),
        }
    }

    #[test]
    fn verbatim_tag_wraps() {
        let v = parse("!<tag:example.com,2026:x> 5\n");
        match v {
            BorrowedValue::Tagged(tag, inner) => {
                assert_eq!(tag, "!<tag:example.com,2026:x>");
                assert!(matches!(*inner, BorrowedValue::Int(5)));
            }
            other => panic!("expected Tagged, got {other:?}"),
        }
    }

    #[test]
    fn local_tag_short_form() {
        let v = parse("!local foo\n");
        match v {
            BorrowedValue::Tagged(tag, inner) => {
                assert_eq!(tag, "!local");
                assert!(matches!(*inner, BorrowedValue::String(s) if s == "foo"));
            }
            other => panic!("expected Tagged, got {other:?}"),
        }
    }

    #[test]
    fn custom_tag_on_block_seq() {
        let v = parse("!mytag\n- a\n- b\n");
        match v {
            BorrowedValue::Tagged(tag, inner) => {
                assert_eq!(tag, "!mytag");
                match *inner {
                    BorrowedValue::Seq(items) => {
                        assert_eq!(items.len(), 2);
                        assert!(matches!(&items[0], BorrowedValue::String(s) if s == "a"));
                        assert!(matches!(&items[1], BorrowedValue::String(s) if s == "b"));
                    }
                    other => panic!("expected Seq, got {other:?}"),
                }
            }
            other => panic!("expected Tagged, got {other:?}"),
        }
    }

    #[test]
    fn custom_tag_on_block_map() {
        let v = parse("!mytag\nk: v\n");
        match v {
            BorrowedValue::Tagged(tag, inner) => {
                assert_eq!(tag, "!mytag");
                match *inner {
                    BorrowedValue::Map(pairs) => {
                        assert_eq!(pairs.len(), 1);
                        assert!(matches!(&pairs[0].0, BorrowedValue::String(s) if s == "k"));
                        assert!(matches!(&pairs[0].1, BorrowedValue::String(s) if s == "v"));
                    }
                    other => panic!("expected Map, got {other:?}"),
                }
            }
            other => panic!("expected Tagged, got {other:?}"),
        }
    }

    #[test]
    fn no_tag_parses_plain() {
        let v = parse("42\n");
        assert!(matches!(v, BorrowedValue::Int(42)));
    }

    // anchors & aliases

    fn map_pairs(v: BorrowedValue<'_>) -> Vec<(BorrowedValue<'_>, BorrowedValue<'_>)> {
        match v {
            BorrowedValue::Map(p) => p,
            other => panic!("expected Map, got {other:?}"),
        }
    }

    #[test]
    fn anchor_then_alias_scalar() {
        let pairs = map_pairs(parse("a: &id 42\nb: *id\n"));
        assert_eq!(pairs.len(), 2);
        assert!(matches!(&pairs[0].1, BorrowedValue::Int(42)));
        assert!(matches!(&pairs[1].1, BorrowedValue::Int(42)));
    }

    #[test]
    fn alias_on_block_seq() {
        let src = "list: &l\n  - a\n  - b\ncopy: *l\n";
        let pairs = map_pairs(parse(src));
        assert_eq!(pairs.len(), 2);
        let seq_eq = |v: &BorrowedValue<'_>| match v {
            BorrowedValue::Seq(items) => {
                items.len() == 2
                    && matches!(&items[0], BorrowedValue::String(s) if s == "a")
                    && matches!(&items[1], BorrowedValue::String(s) if s == "b")
            }
            _ => false,
        };
        assert!(seq_eq(&pairs[0].1), "list");
        assert!(seq_eq(&pairs[1].1), "copy");
    }

    #[test]
    fn alias_on_block_map() {
        let src = "base: &b\n  name: foo\n  port: 80\noverride: *b\n";
        let pairs = map_pairs(parse(src));
        assert_eq!(pairs.len(), 2);
        assert_eq!(&pairs[0].1, &pairs[1].1);
    }

    #[test]
    fn unknown_alias_errors() {
        let result = Parser::new("a: *missing\n").parse_node(0);
        assert!(result.is_err());
    }

    #[test]
    fn empty_anchor_errors() {
        let result = Parser::new("a: & 42\n").parse_node(0);
        assert!(result.is_err());
    }

    #[test]
    fn anchor_then_tag_order() {
        let pairs = map_pairs(parse("a: &id !!str 42\nb: *id\n"));
        assert!(matches!(&pairs[0].1, BorrowedValue::String(s) if s == "42"));
        assert!(matches!(&pairs[1].1, BorrowedValue::String(s) if s == "42"));
    }

    #[test]
    fn tag_then_anchor_order() {
        let pairs = map_pairs(parse("a: !!str &id 42\nb: *id\n"));
        assert!(matches!(&pairs[0].1, BorrowedValue::String(s) if s == "42"));
        assert!(matches!(&pairs[1].1, BorrowedValue::String(s) if s == "42"));
    }

    #[test]
    fn anchor_custom_tag_preserved() {
        let pairs = map_pairs(parse("a: &id !myapp/T foo\nb: *id\n"));
        for (_, v) in &pairs {
            match v {
                BorrowedValue::Tagged(tag, inner) => {
                    assert_eq!(tag, "!myapp/T");
                    assert!(matches!(inner.as_ref(), BorrowedValue::String(s) if s == "foo"));
                }
                other => panic!("expected Tagged, got {other:?}"),
            }
        }
    }

    #[test]
    fn reanchor_latest_wins() {
        let src = "first: &x 1\nsecond: &x 2\nthird: *x\n";
        let pairs = map_pairs(parse(src));
        assert!(matches!(&pairs[2].1, BorrowedValue::Int(2)));
    }

    #[test]
    fn alias_value_is_independent_clone() {
        // Structurally equal via clone (no shared mutable state to assert
        // independence on; PartialEq is the meaningful check).
        let pairs = map_pairs(parse("a: &id foo\nb: *id\n"));
        assert_eq!(&pairs[0].1, &pairs[1].1);
    }

    #[test]
    fn anchor_on_own_line() {
        // anchor terminates with newline, value follows on the next line
        let src = "a: &id\n  nested: 1\nb: *id\n";
        let pairs = map_pairs(parse(src));
        assert_eq!(pairs.len(), 2);
        assert_eq!(&pairs[0].1, &pairs[1].1);
        assert!(matches!(&pairs[0].1, BorrowedValue::Map(_)));
    }

    // multi-document streams

    fn parse_all(src: &str) -> Vec<BorrowedValue<'_>> {
        Parser::new(src).parse_all().unwrap()
    }
    fn parse_one(src: &str) -> BorrowedValue<'_> {
        Parser::new(src).parse().unwrap()
    }

    #[test]
    fn stream_single_implicit_doc() {
        let docs = parse_all("a: 1\nb: 2\n");
        assert_eq!(docs.len(), 1);
        assert!(matches!(&docs[0], BorrowedValue::Map(p) if p.len() == 2));
    }

    #[test]
    fn stream_single_explicit_doc() {
        let docs = parse_all("---\na: 1\n");
        assert_eq!(docs.len(), 1);
        assert!(matches!(&docs[0], BorrowedValue::Map(p) if p.len() == 1));
    }

    #[test]
    fn stream_two_explicit_docs() {
        let docs = parse_all("---\na: 1\n---\nb: 2\n");
        assert_eq!(docs.len(), 2);
        assert!(
            matches!(&docs[0], BorrowedValue::Map(p) if matches!(&p[0].0, BorrowedValue::String(s) if s == "a"))
        );
        assert!(
            matches!(&docs[1], BorrowedValue::Map(p) if matches!(&p[0].0, BorrowedValue::String(s) if s == "b"))
        );
    }

    #[test]
    fn stream_kubectl_style() {
        let src = "\
---
apiVersion: v1
kind: Pod
---
apiVersion: v1
kind: Service
";
        let docs = parse_all(src);
        assert_eq!(docs.len(), 2);
        for d in &docs {
            assert!(matches!(d, BorrowedValue::Map(p) if p.len() == 2));
        }
    }

    #[test]
    fn stream_with_end_markers() {
        let docs = parse_all("---\na: 1\n...\n---\nb: 2\n...\n");
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn stream_empty() {
        assert!(parse_all("").is_empty());
    }

    #[test]
    fn stream_only_markers() {
        let docs = parse_all("---\n---\nb: 2\n");
        assert_eq!(docs.len(), 2);
        assert!(matches!(&docs[0], BorrowedValue::Null));
        assert!(matches!(&docs[1], BorrowedValue::Map(_)));
    }

    #[test]
    fn stream_directives_skipped() {
        let docs = parse_all("%YAML 1.2\n---\na: 1\n");
        assert_eq!(docs.len(), 1);
    }

    #[test]
    fn stream_multiple_directives() {
        let docs = parse_all("%YAML 1.2\n%TAG !foo! tag:example.com,2026:\n---\na: 1\n");
        assert_eq!(docs.len(), 1);
    }

    #[test]
    fn parse_single_succeeds_on_one() {
        let v = parse_one("a: 1\n");
        assert!(matches!(v, BorrowedValue::Map(p) if p.len() == 1));
    }

    #[test]
    fn parse_single_errors_on_multi() {
        let result = Parser::new("---\na: 1\n---\nb: 2\n").parse();
        assert!(result.is_err());
    }

    #[test]
    fn parse_single_on_empty() {
        assert!(matches!(parse_one(""), BorrowedValue::Null));
    }

    #[test]
    fn anchors_reset_between_docs() {
        // doc 1 defines &x; doc 2 references *x → should error
        let result = Parser::new("---\nbase: &x 1\n---\nuse: *x\n").parse_all();
        assert!(result.is_err());
    }

    #[test]
    fn triple_dash_in_scalar_is_not_marker() {
        // quoted scalar — never sees at_doc_marker check
        let docs = parse_all("key: '---'\n");
        assert_eq!(docs.len(), 1);
        let pairs = match &docs[0] {
            BorrowedValue::Map(p) => p,
            _ => panic!(),
        };
        assert!(matches!(&pairs[0].1, BorrowedValue::String(s) if s == "---"));
    }

    #[test]
    fn seq_dash_not_confused_with_marker() {
        let docs = parse_all("- item\n- item2\n");
        assert_eq!(docs.len(), 1);
        assert!(matches!(&docs[0], BorrowedValue::Seq(items) if items.len() == 2));
    }

    // merge keys (<<: *base)

    fn keys_of(v: &BorrowedValue<'_>) -> Vec<String> {
        match v {
            BorrowedValue::Map(pairs) => pairs
                .iter()
                .map(|(k, _)| match k {
                    BorrowedValue::String(s) => s.to_string(),
                    other => format!("{other:?}"),
                })
                .collect(),
            _ => panic!("expected Map"),
        }
    }

    fn get<'a, 'b>(v: &'a BorrowedValue<'b>, key: &str) -> &'a BorrowedValue<'b> {
        match v {
            BorrowedValue::Map(pairs) => pairs
                .iter()
                .find_map(|(k, val)| match k {
                    BorrowedValue::String(s) if s == key => Some(val),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing key {key}")),
            _ => panic!("expected Map"),
        }
    }

    #[test]
    fn merge_simple_splice() {
        let src = "\
defaults: &d
  port: 80
  host: localhost
service:
  <<: *d
  port: 443
";
        let pairs = map_pairs(parse(src));
        let service = &pairs[1].1;
        assert!(matches!(get(service, "port"), BorrowedValue::Int(443)));
        assert!(matches!(get(service, "host"), BorrowedValue::String(s) if s == "localhost"));
        assert!(!keys_of(service).contains(&"<<".to_string()));
    }

    #[test]
    fn merge_parent_wins_over_source() {
        let src = "\
d: &d
  shared: from_default
target:
  <<: *d
  shared: overridden
";
        let pairs = map_pairs(parse(src));
        let target = &pairs[1].1;
        assert!(matches!(get(target, "shared"), BorrowedValue::String(s) if s == "overridden"));
    }

    #[test]
    fn merge_seq_of_aliases_left_wins() {
        let src = "\
a: &a
  k: from_a
b: &b
  k: from_b
  extra: from_b
target:
  <<: [*a, *b]
";
        let pairs = map_pairs(parse(src));
        let target = &pairs[2].1;
        // left wins: a's k beats b's k
        assert!(matches!(get(target, "k"), BorrowedValue::String(s) if s == "from_a"));
        // b's extra still merges in since it's not in a
        assert!(matches!(get(target, "extra"), BorrowedValue::String(s) if s == "from_b"));
    }

    #[test]
    fn merge_no_merge_key_untouched() {
        let pairs = map_pairs(parse("a: 1\nb: 2\n"));
        assert_eq!(pairs.len(), 2);
        assert!(
            !pairs
                .iter()
                .any(|(k, _)| matches!(k, BorrowedValue::String(s) if s == "<<"))
        );
    }

    #[test]
    fn merge_non_map_value_dropped() {
        // <<: 42 is invalid; should be silently dropped
        let pairs = map_pairs(parse("k: v\n'<<': 42\n"));
        let keys: Vec<String> = pairs
            .iter()
            .filter_map(|(k, _)| match k {
                BorrowedValue::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        assert!(!keys.contains(&"<<".to_string()));
        assert!(keys.contains(&"k".to_string()));
    }

    #[test]
    fn merge_inside_seq() {
        let src = "\
d: &d
  shared: yes
items:
  - <<: *d
    own: a
  - <<: *d
    own: b
";
        let pairs = map_pairs(parse(src));
        let items = match &pairs[1].1 {
            BorrowedValue::Seq(s) => s,
            _ => panic!(),
        };
        for (i, item) in items.iter().enumerate() {
            assert!(matches!(get(item, "shared"), BorrowedValue::String(s) if s == "yes"));
            let own = match get(item, "own") {
                BorrowedValue::String(s) => s.as_ref(),
                _ => panic!(),
            };
            assert_eq!(own, if i == 0 { "a" } else { "b" });
        }
    }

    #[test]
    fn merge_nested_resolves_bottom_up() {
        // outer's *inner clone has its own << that must resolve before
        // outer splices inner in
        let src = "\
other: &other
  z: from_other
inner: &inner
  k: v
  <<: *other
outer:
  <<: *inner
";
        let pairs = map_pairs(parse(src));
        let outer = &pairs[2].1;
        assert!(matches!(get(outer, "k"), BorrowedValue::String(s) if s == "v"));
        assert!(matches!(get(outer, "z"), BorrowedValue::String(s) if s == "from_other"));
        assert!(!keys_of(outer).contains(&"<<".to_string()));
    }

    #[test]
    fn merge_inside_tagged_value() {
        // tag preserved, merge resolved inside
        let src = "\
d: &d
  x: 1
wrapped: !mytag
  <<: *d
  y: 2
";
        let pairs = map_pairs(parse(src));
        let wrapped = &pairs[1].1;
        match wrapped {
            BorrowedValue::Tagged(tag, inner) => {
                assert_eq!(tag, "!mytag");
                assert!(matches!(get(inner, "x"), BorrowedValue::Int(1)));
                assert!(matches!(get(inner, "y"), BorrowedValue::Int(2)));
            }
            other => panic!("expected Tagged, got {other:?}"),
        }
    }

    // BOM handling (UTF-8 BOM at stream start, U+FEFF)

    #[test]
    fn bom_stripped_simple() {
        let pairs = map_pairs(parse("\u{FEFF}a: 1\n"));
        assert!(matches!(&pairs[0].0, BorrowedValue::String(s) if s == "a"));
        assert!(matches!(&pairs[0].1, BorrowedValue::Int(1)));
    }

    #[test]
    fn bom_empty_stream() {
        let v = Parser::new("\u{FEFF}").parse().unwrap();
        assert!(matches!(v, BorrowedValue::Null));
    }

    #[test]
    fn bom_with_doc_marker() {
        let docs = parse_all("\u{FEFF}---\nkind: Pod\n");
        assert_eq!(docs.len(), 1);
    }

    #[test]
    fn bom_in_middle_preserved() {
        // only LEADING BOM is stripped; in-content BOM stays as content
        let pairs = map_pairs(parse("key: \"a\u{FEFF}b\"\n"));
        let v = match &pairs[0].1 {
            BorrowedValue::String(s) => s.as_ref(),
            _ => panic!(),
        };
        assert_eq!(v, "a\u{FEFF}b");
    }

    #[test]
    fn merge_compose_style() {
        // Realistic docker-compose anchor pattern
        let src = "\
x-defaults: &defaults
  restart: always
  logging:
    driver: json-file
services:
  web:
    <<: *defaults
    image: nginx
  api:
    <<: *defaults
    image: api:latest
    restart: on-failure
";
        let pairs = map_pairs(parse(src));
        let services = &pairs[1].1;
        let web = get(services, "web");
        let api = get(services, "api");
        assert!(matches!(get(web, "image"), BorrowedValue::String(s) if s == "nginx"));
        assert!(matches!(get(web, "restart"), BorrowedValue::String(s) if s == "always"));
        assert!(matches!(get(api, "restart"), BorrowedValue::String(s) if s == "on-failure")); // override
    }

    // --- multi-line plain scalars at stream level ---

    #[test]
    fn top_level_scalar_folds() {
        let v = parse("one\ntwo\n");
        assert!(matches!(v, BorrowedValue::String(s) if s == "one two"));
    }

    #[test]
    fn fold_stops_at_start_marker() {
        let docs = Parser::new("one\n---\ntwo\n").parse_all().unwrap();
        assert_eq!(docs.len(), 2);
        assert!(matches!(&docs[0], BorrowedValue::String(s) if s == "one"));
        assert!(matches!(&docs[1], BorrowedValue::String(s) if s == "two"));
    }

    #[test]
    fn fold_stops_at_end_marker() {
        let docs = Parser::new("one\n...\ntwo\n").parse_all().unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn quoted_value_does_not_fold() {
        // a quoted scalar ends at its closing quote; the next line is orphaned
        let err = Parser::new("a: \"one\"\n  two\n").parse().unwrap_err();
        assert_eq!((err.line, err.col), (Some(2), Some(3)));
    }

    // --- document boundaries (§9.2 l-yaml-stream) ---

    #[test]
    fn second_bare_document_errors() {
        let err = Parser::new("{a: 1}\n{b: 2}\n").parse_all().unwrap_err();
        assert_eq!((err.line, err.col), (Some(2), Some(1)));
        assert!(err.msg.contains("---"), "got {}", err.msg);
    }

    #[test]
    fn orphan_error_points_at_content_not_column_one() {
        let err = Parser::new("a: \"one\"\n    two\n")
            .parse_all()
            .unwrap_err();
        assert_eq!((err.line, err.col), (Some(2), Some(5)));
    }

    #[test]
    fn bare_single_document() {
        assert_eq!(Parser::new("a: 1\n").parse_all().unwrap().len(), 1);
    }

    #[test]
    fn leading_marker_single_document() {
        assert_eq!(Parser::new("---\na: 1\n").parse_all().unwrap().len(), 1);
    }

    #[test]
    fn explicit_multi_document() {
        let docs = Parser::new("---\na: 1\n---\nb: 2\n").parse_all().unwrap();
        assert_eq!(docs.len(), 2);
    }

    #[test]
    fn bare_document_after_end_marker() {
        // [211] permits a bare document after a `...` suffix. libyaml and
        // go-yaml reject this; the grammar does not.
        assert_eq!(
            Parser::new("a: 1\n...\nb: 2\n").parse_all().unwrap().len(),
            2
        );
    }

    #[test]
    fn explicit_document_after_end_marker() {
        assert_eq!(
            Parser::new("a: 1\n...\n---\nb: 2\n")
                .parse_all()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn directive_then_marker() {
        assert_eq!(
            Parser::new("%YAML 1.2\n---\na: 1\n")
                .parse_all()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn trailing_blanks_and_comments_end_the_stream() {
        assert_eq!(
            Parser::new("a: 1\n\n# done\n").parse_all().unwrap().len(),
            1
        );
    }

    #[test]
    fn empty_input_is_null() {
        assert!(matches!(parse(""), BorrowedValue::Null));
    }
}
