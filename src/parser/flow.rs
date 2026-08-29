use std::borrow::Cow;

use crate::{BorrowedValue, Parser, Result, borrowed_value::apply_tag, patterns::resolve_scalar};

impl<'a> Parser<'a> {
    /// Parse a flow-style sequence
    ///
    /// Input: `[1, 2, 3]` → Output: `BorrowedValue::Seq([Int(1), Int(2), Int(3)])`
    ///
    /// Cursor enters at `[`. Skips whitespace/newlines/comments between
    /// tokens. Empty (`[]`) and trailing-comma (`[a,]`) forms are legal.
    pub(super) fn parse_flow_seq(&mut self) -> Result<BorrowedValue<'a>> {
        self.advance(); // consume '['

        let mut items = Vec::new();

        loop {
            self.skip_flow_whitespace();

            match self.peek() {
                Some(b']') => {
                    self.advance();
                    break;
                }
                None => return Err(self.err("unterminated flow sequence")),
                _ => {}
            }

            items.push(self.parse_flow_node()?);

            self.skip_flow_whitespace();

            match self.peek() {
                Some(b',') => {
                    self.advance();
                }
                Some(b']') => {
                    self.advance();
                    break;
                }
                None => return Err(self.err("unterminated flow sequence")),
                Some(_) => return Err(self.err("expected ',' or ']' in flow sequence")),
            }
        }

        Ok(BorrowedValue::Seq(items))
    }

    /// Parse a flow-style mapping
    ///
    /// Input: `{a: 1, b: 2}` → Output: `BorrowedValue::Map([(String("a"), Int(1)), (String("b"), Int(2))])`
    ///
    /// Cursor enters at `{`. Supports JSON-style `{"a":1}` and spaceless
    /// `{a:1}` per spec §7.5.3 (plain scalars in flow can't end with `:`).
    /// Pair shorthand (`{a, b}`) yields null values.
    pub(super) fn parse_flow_map(&mut self) -> Result<BorrowedValue<'a>> {
        self.advance(); // consume '{'
        let mut pairs = Vec::new();

        loop {
            self.skip_flow_whitespace();
            match self.peek() {
                Some(b'}') => {
                    self.advance();
                    break;
                }
                None => return Err(self.err("unterminated flow map")),
                _ => {}
            }

            let key = self.parse_flow_node()?;

            self.skip_flow_whitespace();

            let value = if self.peek() == Some(b':') {
                self.advance();
                self.skip_flow_whitespace();
                match self.peek() {
                    Some(b',' | b'}') | None => BorrowedValue::Null,
                    _ => self.parse_flow_node()?,
                }
            } else {
                BorrowedValue::Null // implicit null value, like {a, b}
            };

            pairs.push((key, value));

            self.skip_flow_whitespace();

            match self.peek() {
                Some(b',') => self.advance(),
                Some(b'}') => {
                    self.advance();
                    break;
                }
                None => return Err(self.err("unterminated flow map")),
                Some(_) => return Err(self.err("expected ',' or '}' in flow map")),
            }
        }
        Ok(BorrowedValue::Map(pairs))
    }

    /// Parse one node in flow context (scalar, nested flow, alias, optionally tagged/anchored)
    ///
    /// Input: `*ref` → Output: `BorrowedValue::String(...)` (alias resolved from anchors map)
    ///
    /// Unlike `parse_node`, this never recurses into block-style parsers —
    /// flow context is closed per spec. Tag/anchor prefixes are accepted in
    /// either order, just like block context.
    fn parse_flow_node(&mut self) -> Result<BorrowedValue<'a>> {
        let mut tag: Option<Cow<'a, str>> = None;
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

            self.skip_flow_whitespace();
        }

        let value = match self.peek() {
            Some(b'[') => self.parse_flow_seq()?,
            Some(b'{') => self.parse_flow_map()?,
            Some(b'"') => {
                BorrowedValue::String(self.parse_double_quoted(self.col.saturating_sub(1))?)
            }
            Some(b'\'') => {
                BorrowedValue::String(self.parse_single_quoted(self.col.saturating_sub(1))?)
            }
            Some(b'*') => self.parse_alias()?,
            Some(b',' | b']' | b'}') | None => BorrowedValue::Null,
            _ => {
                let s = self.parse_plain_scalar(true);
                resolve_scalar(s)
            }
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

    /// Parse a flow node; a `: ` on the same line means it was an implicit key
    pub(super) fn flow_node_or_map(&mut self, indent: usize) -> Result<BorrowedValue<'a>> {
        let start_line = self.line;
        let node = if self.peek() == Some(b'[') {
            self.parse_flow_seq()?
        } else {
            self.parse_flow_map()?
        };

        self.skip_spaces();
        if self.is_kv_colon() {
            self.reject_multiline_key(start_line)?;
            return self.continue_block_map(node, indent);
        }
        Ok(node)
    }

    pub(super) fn reject_multiline_key(&self, start_line: usize) -> Result<()> {
        if self.line != start_line {
            return Err(self.err("implicit key must not span multiple lines"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> BorrowedValue<'_> {
        Parser::new(src).parse_node(0).unwrap()
    }

    fn as_seq(v: BorrowedValue<'_>) -> Vec<BorrowedValue<'_>> {
        match v {
            BorrowedValue::Seq(items) => items,
            other => panic!("expected Seq, got {other:?}"),
        }
    }

    fn as_map(v: BorrowedValue<'_>) -> Vec<(BorrowedValue<'_>, BorrowedValue<'_>)> {
        match v {
            BorrowedValue::Map(pairs) => pairs,
            other => panic!("expected Map, got {other:?}"),
        }
    }

    // flow seq

    #[test]
    fn flow_seq_empty() {
        assert!(as_seq(parse("[]\n")).is_empty());
    }

    #[test]
    fn flow_seq_simple_strings() {
        let items = as_seq(parse("[a, b, c]\n"));
        let strs: Vec<&str> = items
            .iter()
            .map(|v| match v {
                BorrowedValue::String(s) => s.as_ref(),
                _ => panic!(),
            })
            .collect();
        assert_eq!(strs, vec!["a", "b", "c"]);
    }

    #[test]
    fn flow_seq_ints() {
        let items = as_seq(parse("[1, 2, 3]\n"));
        assert!(matches!(&items[0], BorrowedValue::Int(1)));
        assert!(matches!(&items[1], BorrowedValue::Int(2)));
        assert!(matches!(&items[2], BorrowedValue::Int(3)));
    }

    #[test]
    fn flow_seq_trailing_comma() {
        assert_eq!(as_seq(parse("[a, b,]\n")).len(), 2);
    }

    #[test]
    fn flow_seq_quoted_items() {
        let items = as_seq(parse("[\"a b\", 'c d']\n"));
        assert!(matches!(&items[0], BorrowedValue::String(s) if s == "a b"));
        assert!(matches!(&items[1], BorrowedValue::String(s) if s == "c d"));
    }

    #[test]
    fn flow_seq_nested() {
        let items = as_seq(parse("[[1, 2], [3]]\n"));
        let inner0 = match &items[0] {
            BorrowedValue::Seq(s) => s,
            _ => panic!(),
        };
        assert_eq!(inner0.len(), 2);
        let inner1 = match &items[1] {
            BorrowedValue::Seq(s) => s,
            _ => panic!(),
        };
        assert_eq!(inner1.len(), 1);
    }

    #[test]
    fn flow_seq_multiline() {
        let items = as_seq(parse("[\n  a,\n  b,\n]\n"));
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn flow_seq_unterminated_errors() {
        assert!(Parser::new("[a, b\n").parse_node(0).is_err());
    }

    #[test]
    fn flow_seq_with_comment() {
        let items = as_seq(parse("[a, # ignored\n  b]\n"));
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[0], BorrowedValue::String(s) if s == "a"));
        assert!(matches!(&items[1], BorrowedValue::String(s) if s == "b"));
    }

    #[test]
    fn flow_seq_with_alias() {
        let pairs = as_map(parse("defs: &x 7\nitems: [*x, *x]\n"));
        let items = match &pairs[1].1 {
            BorrowedValue::Seq(s) => s,
            _ => panic!(),
        };
        assert!(matches!(&items[0], BorrowedValue::Int(7)));
        assert!(matches!(&items[1], BorrowedValue::Int(7)));
    }

    // flow map

    #[test]
    fn flow_map_empty() {
        assert!(as_map(parse("{}\n")).is_empty());
    }

    #[test]
    fn flow_map_simple() {
        let pairs = as_map(parse("{a: 1, b: 2}\n"));
        assert!(matches!(&pairs[0].0, BorrowedValue::String(s) if s == "a"));
        assert!(matches!(&pairs[0].1, BorrowedValue::Int(1)));
        assert!(matches!(&pairs[1].0, BorrowedValue::String(s) if s == "b"));
        assert!(matches!(&pairs[1].1, BorrowedValue::Int(2)));
    }

    #[test]
    fn flow_map_quoted_keys() {
        let pairs = as_map(parse("{\"k 1\": 1}\n"));
        assert!(matches!(&pairs[0].0, BorrowedValue::String(s) if s == "k 1"));
        assert!(matches!(&pairs[0].1, BorrowedValue::Int(1)));
    }

    #[test]
    fn flow_map_json_style_quoted_key() {
        let pairs = as_map(parse("{\"a\":1}\n"));
        assert!(matches!(&pairs[0].0, BorrowedValue::String(s) if s == "a"));
        assert!(matches!(&pairs[0].1, BorrowedValue::Int(1)));
    }

    #[test]
    fn flow_map_colon_no_space() {
        let pairs = as_map(parse("{a:1, b:2}\n"));
        assert!(matches!(&pairs[0].0, BorrowedValue::String(s) if s == "a"));
        assert!(matches!(&pairs[0].1, BorrowedValue::Int(1)));
        assert!(matches!(&pairs[1].0, BorrowedValue::String(s) if s == "b"));
        assert!(matches!(&pairs[1].1, BorrowedValue::Int(2)));
    }

    #[test]
    fn flow_map_implicit_null_value() {
        let pairs = as_map(parse("{a:, b: 2}\n"));
        assert!(matches!(&pairs[0].1, BorrowedValue::Null));
        assert!(matches!(&pairs[1].1, BorrowedValue::Int(2)));
    }

    #[test]
    fn flow_pair_shorthand() {
        let pairs = as_map(parse("{a, b}\n"));
        assert_eq!(pairs.len(), 2);
        assert!(matches!(&pairs[0].1, BorrowedValue::Null));
        assert!(matches!(&pairs[1].1, BorrowedValue::Null));
    }

    #[test]
    fn flow_map_multiline() {
        let pairs = as_map(parse("{\n  a: 1,\n  b: 2,\n}\n"));
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn flow_map_with_comment() {
        let pairs = as_map(parse("{a: 1, # mid-map\n  b: 2}\n"));
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn flow_map_unterminated_errors() {
        assert!(Parser::new("{a: 1\n").parse_node(0).is_err());
    }

    // mixed flow/block

    #[test]
    fn flow_map_nested_in_seq() {
        let items = as_seq(parse("[{a: 1}, {b: 2}]\n"));
        assert_eq!(items.len(), 2);
        let first = match &items[0] {
            BorrowedValue::Map(p) => p,
            _ => panic!(),
        };
        assert_eq!(first.len(), 1);
    }

    #[test]
    fn flow_seq_as_block_map_value() {
        let pairs = as_map(parse("items: [a, b]\nname: x\n"));
        assert_eq!(pairs.len(), 2);
        let items = match &pairs[0].1 {
            BorrowedValue::Seq(s) => s,
            _ => panic!(),
        };
        assert_eq!(items.len(), 2);
        assert!(matches!(&pairs[1].1, BorrowedValue::String(s) if s == "x"));
    }
}
