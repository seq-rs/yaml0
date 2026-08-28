use crate::{Parser, Result, BorrowedValue};

impl<'a> Parser<'a> {
    /// Parse a block-style sequence at the given indent
    ///
    /// Input:
    /// ```yaml
    /// - a
    /// - b
    /// ```
    ///
    /// Output: `BorrowedValue::Seq([String("a"), String("b")])`
    ///
    /// Stops at EOF or a line whose indent doesn't match `indent` (or whose
    /// first non-blank char isn't a sequence dash).
    pub(super) fn parse_block_seq(&mut self, indent: usize) -> Result<BorrowedValue<'a>> {
        let mut items = Vec::new();

        loop {
            self.skip_blank_and_comment_lines();

            if self.at_eof() {
                // Stop when reaching EOF
                break;
            }

            // Normalize: if not already at dash, advance past indent
            if self.peek() != Some(b'-') {
                let cur = self.current_indent()?;
                // If indentation is not the same as the current line, we stop
                if cur != indent {
                    // Confirm current indentation
                    break;
                }

                // If the next line doesn't have a dash with the same indentation, we stop
                if !self.is_seq_dash_at(self.pos + cur) {
                    break;
                }

                for _ in 0..indent {
                    self.advance();
                }
            }

            if self.peek() != Some(b'-') || !self.at_seq_dash() {
                // Need start with dash
                break;
            }

            self.advance(); // eat `-`
            self.skip_spaces();

            items.push(self.parse_node(indent + 1)?);
        }
        Ok(BorrowedValue::Seq(items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    macro_rules! assert_seq {
        ($yaml:expr, $expected:expr) => {
            let mut p = Parser::new($yaml);
            let v = p.parse_block_seq(0).unwrap();
            match v {
                BorrowedValue::Seq(items) => {
                    let strs: Vec<&str> = items
                        .iter()
                        .map(|i| match i {
                            BorrowedValue::String(s) => s.as_ref(),
                            _ => panic!("expected string, got {:?}", i),
                        })
                        .collect();
                    assert_eq!(strs, $expected);
                }
                _ => panic!("expected Seq"),
            }
        };
    }

    #[test]
    fn seq_two_items() {
        assert_seq!("- a\n- b\n", vec!["a", "b"]);
    }

    #[test]
    fn seq_one_item() {
        assert_seq!("- only\n", vec!["only"]);
    }

    #[test]
    fn seq_no_trailing_newline() {
        assert_seq!("- a\n- b", vec!["a", "b"]);
    }

    #[test]
    fn seq_with_blank_lines() {
        assert_seq!("- a\n\n- b\n", vec!["a", "b"]);
    }

    #[test]
    fn seq_with_comment_lines() {
        assert_seq!("- a\n# comment\n- b\n", vec!["a", "b"]);
    }

    #[test]
    fn seq_quoted_values() {
        assert_seq!(
            r#"- "foo"
- 'bar'
"#,
            vec!["foo", "bar"]
        );
    }

    #[test]
    fn seq_with_spaces_in_items() {
        assert_seq!("- hello world\n- foo bar\n", vec!["hello world", "foo bar"]);
    }

    #[test]
    fn seq_dashes_in_value() {
        // "-foo" is a plain scalar; the dash is part of the value because no space follows
        // here it would NOT be parsed as a nested seq item
        // but the outer seq should still get "value"
        assert_seq!("- -value\n- other\n", vec!["-value", "other"]);
    }

    #[test]
    fn seq_stops_at_lesser_indent() {
        // when reading at indent 2, a "- " at indent 0 ends our scope
        let mut p = Parser::new("  - a\n  - b\nother: x\n");
        let v = p.parse_block_seq(2).unwrap();
        match v {
            BorrowedValue::Seq(items) => assert_eq!(items.len(), 2),
            _ => panic!(),
        }
        // cursor should be at 'o' of "other"
        assert_eq!(p.peek(), Some(b'o'));
    }

    #[test]
    fn seq_empty_at_eof() {
        // if nothing follows, we get an empty seq
        let mut p = Parser::new("");
        let v = p.parse_block_seq(0).unwrap();
        assert!(matches!(v, BorrowedValue::Seq(items) if items.is_empty()));
    }

    #[test]
    fn seq_item_folds_continuation() {
        assert_seq!("- one\n  two\n", vec!["one two"]);
    }

    #[test]
    fn seq_fold_stops_at_next_item() {
        assert_seq!("- one\n- two\n", vec!["one", "two"]);
    }

    #[test]
    fn seq_fold_empty_line_becomes_newline() {
        assert_seq!("- one\n\n  two\n", vec!["one\ntwo"]);
    }

    #[test]
    fn seq_fold_strips_extra_indent() {
        assert_seq!("- one\n      two\n", vec!["one two"]);
    }
}
