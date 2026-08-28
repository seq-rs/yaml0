use crate::Result;
use std::borrow::Cow;

use crate::Parser;
use crate::parser::trim_trailing_whitespace_end;

impl<'a> Parser<'a> {
    pub(super) fn is_colon_terminator(&self) -> bool {
        matches!(
            self.peek_at(self.pos + 1),
            None | Some(b' ' | b'\t' | b'\n' | b'\r')
        )
    }

    pub(super) fn is_comment_start(&self) -> bool {
        self.pos == 0 || matches!(self.src.as_bytes()[self.pos - 1], b' ' | b'\t')
    }

    pub(super) fn fold_plain_continuation(
        &mut self,
        first: Cow<'a, str>,
        min_indent: usize,
    ) -> Result<Cow<'a, str>> {
        let mut out: Option<String> = None;
        loop {
            if self.peek() == Some(b':') {
                return Err(self.err("mapping values are not allowed in this context"));
            }
            if !self.at_line_end() {
                break;
            }
            let Some((breaks, content)) = self.peek_plain_continuation(min_indent) else {
                break;
            };

            let out = out.get_or_insert_with(|| first.to_string());

            while self.pos < content {
                self.advance();
            }

            if breaks == 1 {
                out.push(' ')
            } else {
                for _ in 1..breaks {
                    out.push('\n')
                }
            }
            out.push_str(&self.parse_plain_scalar(false));
        }
        Ok(out.map_or(first, Cow::Owned))
    }

    fn peek_plain_continuation(&self, min_indent: usize) -> Option<(usize, usize)> {
        let bytes = self.src.as_bytes();
        let mut i = self.pos;
        let mut breaks = 0;

        loop {
            match bytes.get(i) {
                // one line break, CRLF counts as one
                Some(b'\r') => {
                    i += 1;
                    if bytes.get(i) == Some(&b'\n') {
                        i += 1;
                    }
                }
                Some(b'\n') => i += 1,
                // EOF, or cursor wasn't on a break after all
                _ => return None,
            }

            breaks += 1;

            // `s-indent(n)`: spaces only
            let line_start = i;
            while bytes.get(i) == Some(&b' ') {
                i += 1;
            }

            let indent = i - line_start;

            // `s-separate-in-line?`: tabs are legal separation past the indent
            let mut content = i;
            while matches!(bytes.get(content), Some(b' ' | b'\t')) {
                content += 1;
            }

            match bytes.get(content) {
                // trailing whitespace then EOF: no continuation
                None => return None,
                // empty line: keep counting breaks
                Some(b'\n' | b'\r') => {
                    i = content;
                    continue;
                }
                // a comment line ends the scalar
                Some(b'#') => return None,
                _ => {}
            }

            if indent < min_indent {
                return None;
            }

            if indent == 0
                && (self.is_doc_marker_at(content, b"---")
                    || self.is_doc_marker_at(content, b"..."))
            {
                return None;
            }

            return Some((breaks, content));
        }
    }

    /// Read an unquoted plain scalar from the current cursor
    ///
    /// Input: `hello world\n` → Output: `Cow::Borrowed("hello world")`
    ///
    /// Stops at EOF, line break, `:` followed by whitespace, `#` after
    /// whitespace, or (in flow context) `,`/`]`/`}`. Trailing whitespace is
    /// trimmed. Always borrowed — plain scalars never need allocation.
    pub(super) fn parse_plain_scalar(&mut self, in_flow: bool) -> Cow<'a, str> {
        let start = self.pos;
        loop {
            match self.peek() {
                // EOF
                None => break,
                Some(b'\n' | b'\r') => break,
                Some(b':') if in_flow => break,
                Some(b':') if self.is_colon_terminator() => break,
                Some(b'#') if self.is_comment_start() => break,
                Some(b',' | b']' | b'}') if in_flow => break,
                _ => self.advance(),
            }
        }

        let end = trim_trailing_whitespace_end(&self.src.as_bytes()[start..self.pos]) + start;
        Cow::Borrowed(&self.src[start..end])
    }

    /// Read a double-quoted scalar; borrowed if no escapes or folds
    ///
    /// Input: `"hello"` → Output: `Cow::Borrowed("hello")`
    /// Input: `"a\nb"`  → Output: `Cow::Owned("a\nb")` (escape forces allocation)
    ///
    /// Hot path stays borrowed; the moment we see `\` or a line break we
    /// hand off to `parse_double_quoted_owned`. `parent_indent` is the
    /// owning node's indent, used for indent-aware folding of continuation
    /// lines per YAML §6.5.
    pub(super) fn parse_double_quoted(&mut self, parent_indent: usize) -> Result<Cow<'a, str>> {
        debug_assert_eq!(self.peek(), Some(b'"'));
        self.advance();
        let content_start = self.pos;

        loop {
            match self.peek() {
                None => return Err(self.err("unterminated double-quoted string")),
                Some(b'"') => {
                    let end = self.pos;
                    self.advance();
                    return Ok(Cow::Borrowed(&self.src[content_start..end]));
                }
                Some(b'\\') => {
                    return self.parse_double_quoted_owned(content_start, parent_indent);
                }
                Some(b'\n' | b'\r') => {
                    return self.parse_double_quoted_owned(content_start, parent_indent);
                }
                _ => self.advance(),
            }
        }
    }

    /// Owned-mode continuation when a double-quoted scalar needs allocation
    pub(super) fn parse_double_quoted_owned(
        &mut self,
        content_start: usize,
        parent_indent: usize,
    ) -> Result<Cow<'a, str>> {
        let mut out = String::new();
        let mut last_copy_end = content_start;

        loop {
            match self.peek() {
                None => return Err(self.err("unterminated double-quoted string")),
                Some(b'"') => {
                    out.push_str(&self.src[last_copy_end..self.pos]);
                    self.advance();
                    return Ok(Cow::Owned(out));
                }
                Some(b'\\') => {
                    out.push_str(&self.src[last_copy_end..self.pos]);
                    self.advance();
                    self.decode_double_quoted_escape(&mut out, parent_indent)?;
                    last_copy_end = self.pos;
                }
                Some(b'\n' | b'\r') => {
                    let content_end =
                        trim_trailing_whitespace_end(&self.src.as_bytes()[last_copy_end..self.pos])
                            + last_copy_end;
                    out.push_str(&self.src[last_copy_end..content_end]);
                    self.consume_line_fold(&mut out, parent_indent);
                    last_copy_end = self.pos;
                }
                _ => self.advance(),
            }
        }
    }

    /// Read a single-quoted scalar; borrowed unless `''` escape or line fold
    ///
    /// Input: `'hello'`   → Output: `Cow::Borrowed("hello")`
    /// Input: `'it''s'`   → Output: `Cow::Owned("it's")` (escape forces allocation)
    ///
    /// Single-quoted scalars have no backslash escapes; only `''` represents
    /// a literal `'`. Multi-line continuation folds per YAML §6.5.
    pub(super) fn parse_single_quoted(&mut self, parent_indent: usize) -> Result<Cow<'a, str>> {
        debug_assert_eq!(self.peek(), Some(b'\''));
        self.advance();
        let content_start = self.pos;

        loop {
            match self.peek() {
                None => return Err(self.err("unterminated single-quoted string")),
                Some(b'\'') => {
                    if self.peek_at(self.pos + 1) == Some(b'\'') {
                        // '' escape - switch to owned
                        return self.parse_single_quoted_owned(content_start, parent_indent);
                    }
                    let end = self.pos;
                    self.advance(); // closing '
                    return Ok(Cow::Borrowed(&self.src[content_start..end]));
                }
                Some(b'\n' | b'\r') => {
                    return self.parse_single_quoted_owned(content_start, parent_indent);
                }
                _ => self.advance(),
            }
        }
    }

    /// Owned-mode continuation when a single-quoted scalar needs allocation
    pub(super) fn parse_single_quoted_owned(
        &mut self,
        content_start: usize,
        parent_indent: usize,
    ) -> Result<Cow<'a, str>> {
        let mut out = String::new();
        let mut last_copy_end = content_start;

        loop {
            match self.peek() {
                None => return Err(self.err("unterminated single-quoted string")),
                Some(b'\'') => {
                    if self.peek_at(self.pos + 1) == Some(b'\'') {
                        out.push_str(&self.src[last_copy_end..self.pos]);
                        out.push('\'');
                        self.advance(); //first '
                        self.advance(); //second '
                        last_copy_end = self.pos;
                    } else {
                        out.push_str(&self.src[last_copy_end..self.pos]);
                        self.advance();
                        return Ok(Cow::Owned(out));
                    }
                }
                Some(b'\n' | b'\r') => {
                    let content_end =
                        trim_trailing_whitespace_end(&self.src.as_bytes()[last_copy_end..self.pos])
                            + last_copy_end;
                    out.push_str(&self.src[last_copy_end..content_end]);
                    self.consume_line_fold(&mut out, parent_indent);
                    last_copy_end = self.pos;
                }
                _ => self.advance(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! assert_plain_scalar {
        ($yaml:literal, $expected:literal, $flow:literal) => {
            let mut parser = Parser::new($yaml);
            let res = parser.parse_plain_scalar($flow);
            assert_eq!(res, $expected);
        };
    }

    #[test]
    fn plain_scalar_simple() {
        assert_plain_scalar!("foo\n", "foo", false);
    }

    #[test]
    fn plain_scalar_with_space() {
        assert_plain_scalar!("hello world\n", "hello world", false);
    }

    #[test]
    fn plain_stops_at_colon_space() {
        assert_plain_scalar!("key: val", "key", false);
    }

    #[test]
    fn plain_keeps_bare_colon() {
        assert_plain_scalar!("http://foo\n", "http://foo", false);
    }

    #[test]
    fn plain_stops_at_space_hash() {
        assert_plain_scalar!("foo # bar\n", "foo", false);
    }

    #[test]
    fn plain_keeps_bare_hash() {
        assert_plain_scalar!("foo#bar\n", "foo#bar", false);
    }

    #[test]
    fn plain_trims_trailing_ws() {
        assert_plain_scalar!("foo    \n", "foo", false);
    }

    #[test]
    fn plain_flow_stops_at_comma() {
        assert_plain_scalar!("a,b", "a", true);
    }

    #[test]
    fn plain_flow_stops_at_bracket() {
        assert_plain_scalar!("a]b", "a", true);
    }

    macro_rules! assert_dq {
        ($yaml:expr, $expected:expr) => {
            let mut p = Parser::new($yaml);
            assert_eq!(p.parse_double_quoted(0).unwrap(), $expected);
        };
        ($yaml:expr, $expected:expr, indent=$ind:literal) => {
            let mut p = Parser::new($yaml);
            assert_eq!(p.parse_double_quoted($ind).unwrap(), $expected);
        };
    }

    macro_rules! assert_sq {
        ($yaml:expr, $expected:expr) => {
            let mut p = Parser::new($yaml);
            assert_eq!(p.parse_single_quoted(0).unwrap(), $expected);
        };
        ($yaml:expr, $expected:expr, indent=$ind:literal) => {
            let mut p = Parser::new($yaml);
            assert_eq!(p.parse_single_quoted($ind).unwrap(), $expected);
        };
    }

    // double-quoted borrow mode (no esc or fold)

    #[test]
    fn dq_simple() {
        assert_dq!(r#""foo""#, "foo");
    }

    #[test]
    fn dq_empty() {
        assert_dq!(r#""""#, "");
    }

    #[test]
    fn dq_with_space() {
        assert_dq!(r#""hello world""#, "hello world");
    }

    #[test]
    fn dq_borrowed_when_no_escape() {
        let mut p = Parser::new(r#""foo""#);
        let cow = p.parse_double_quoted(0).unwrap();
        assert!(
            matches!(cow, Cow::Borrowed(_)),
            "expected borrowed, got owned"
        );
    }

    #[test]
    fn dq_cursor_lands_past_close() {
        let mut p = Parser::new(r#""x" y"#);
        p.parse_double_quoted(0).unwrap();
        assert_eq!(p.peek(), Some(b' '));
    }

    // double-quoted w/ simple escapes

    #[test]
    fn dq_escape_newline() {
        assert_dq!(r#""a\nb""#, "a\nb");
    }

    #[test]
    fn dq_escape_tab() {
        assert_dq!(r#""a\tb""#, "a\tb");
    }

    #[test]
    fn dq_escape_cr() {
        assert_dq!(r#""a\rb""#, "a\rb");
    }

    #[test]
    fn dq_escape_null() {
        assert_dq!(r#""a\0b""#, "a\0b");
    }

    #[test]
    fn dq_escape_backslash() {
        assert_dq!(r#""a\\b""#, "a\\b");
    }

    #[test]
    fn dq_escape_quote() {
        assert_dq!(r#""a\"b""#, "a\"b");
    }

    #[test]
    fn dq_escape_slash() {
        assert_dq!(r#""a\/b""#, "a/b");
    }

    #[test]
    fn dq_escape_space() {
        assert_dq!(r#""a\ b""#, "a b");
    }

    #[test]
    fn dq_escape_bell_backspace_vt_ff_esc() {
        assert_dq!(r#""\a\b\v\f\e""#, "\x07\x08\x0B\x0C\x1B");
    }

    // double-quoted special unicode escapes

    #[test]
    fn dq_escape_next_line() {
        assert_dq!(r#""\N""#, "\u{85}");
    }

    #[test]
    fn dq_escape_nbsp() {
        assert_dq!(r#""\_""#, "\u{A0}");
    }

    #[test]
    fn dq_escape_line_separator() {
        assert_dq!(r#""\L""#, "\u{2028}");
    }

    #[test]
    fn dq_escape_paragraph_separator() {
        assert_dq!(r#""\P""#, "\u{2029}");
    }

    // double-quoted hex escapes

    #[test]
    fn dq_escape_x_hex() {
        assert_dq!(r#""\x41""#, "A");
    }

    #[test]
    fn dq_escape_u_hex_bmp() {
        assert_dq!(r#""\u00E9""#, "é");
    }

    #[test]
    fn dq_escape_big_u_supplementary() {
        // U+1F600 GRINNING FACE
        assert_dq!(r#""\U0001F600""#, "😀");
    }

    // double-quoted owned mode marker

    #[test]
    fn dq_owned_when_escape() {
        let mut p = Parser::new(r#""a\nb""#);
        let cow = p.parse_double_quoted(0).unwrap();
        assert!(matches!(cow, Cow::Owned(_)), "expected owned, got borrowed");
    }

    // double-quoted w/ folds

    #[test]
    fn dq_single_break_folds_to_space() {
        assert_dq!("\"a\nb\"", "a b");
    }

    #[test]
    fn dq_empty_line_becomes_one_newline() {
        // a + blank line + b → a\nb (N empty lines = N-1 newlines... wait, spec: N empty → N newlines)
        // "a\n\nb": one \n + one empty line. fold: zero "in-between" empty → space; but
        // actually we have 1 break + 1 empty line. The "consume_line_fold" sees:
        //   consume first \n; loop iter 1: peek is \n, empty_lines += 1, consume; loop iter 2: peek 'b', break.
        //   empty_lines == 1 → push 1 newline.
        assert_dq!("\"a\n\nb\"", "a\nb");
    }

    #[test]
    fn dq_two_empty_lines_become_two_newlines() {
        // a + 2 empty lines + b → a\n\nb
        assert_dq!("\"a\n\n\nb\"", "a\n\nb");
    }

    #[test]
    fn dq_crlf_fold() {
        assert_dq!("\"a\r\nb\"", "a b");
    }

    #[test]
    fn dq_more_indented_preserves_break() {
        // parent_indent=0, continuation has leading whitespace → preserve break as \n
        assert_dq!("\"a\n  b\"", "a\n  b");
    }

    #[test]
    fn dq_indent_aware_strip() {
        // parent_indent=4: 4 spaces of leading ws are indent (stripped),
        // anything more is content
        assert_dq!("\"a\n    b\"", "a b", indent = 4);
        assert_dq!("\"a\n      b\"", "a\n  b", indent = 4);
    }

    #[test]
    fn dq_trailing_ws_before_break_stripped() {
        // trailing spaces before \n are stripped before fold-to-space
        assert_dq!("\"a   \nb\"", "a b");
    }

    // double-quoted w/ line continuation

    #[test]
    fn dq_line_continuation_swallows_break() {
        // \<newline> + indent → swallow both, emit nothing
        assert_dq!("\"a\\\nb\"", "ab");
    }

    #[test]
    fn dq_line_continuation_strips_indent() {
        assert_dq!("\"a\\\n  b\"", "ab");
    }

    // double-quoted w/ UTF-8 in content

    #[test]
    fn dq_utf8_passthrough() {
        assert_dq!(r#""café""#, "café");
    }

    #[test]
    fn dq_utf8_with_escape_mixed() {
        assert_dq!(r#""\tcafé\n""#, "\tcafé\n");
    }

    // double-quoted error paths

    #[test]
    fn dq_err_unterminated() {
        let mut p = Parser::new(r#""foo"#);
        assert!(p.parse_double_quoted(0).is_err());
    }

    #[test]
    fn dq_err_unknown_escape() {
        let mut p = Parser::new(r#""\q""#);
        assert!(p.parse_double_quoted(0).is_err());
    }

    #[test]
    fn dq_err_truncated_hex() {
        let mut p = Parser::new(r#""\x4""#);
        assert!(p.parse_double_quoted(0).is_err());
    }

    #[test]
    fn dq_err_invalid_hex_digit() {
        let mut p = Parser::new(r#""\xZZ""#);
        assert!(p.parse_double_quoted(0).is_err());
    }

    #[test]
    fn dq_err_invalid_codepoint() {
        // \uD800 is a high surrogate — invalid as a standalone Unicode scalar
        let mut p = Parser::new(r#""\uD800""#);
        assert!(p.parse_double_quoted(0).is_err());
    }

    // single-quoted borrow mode

    #[test]
    fn sq_simple() {
        assert_sq!(r#"'foo'"#, "foo");
    }

    #[test]
    fn sq_empty() {
        assert_sq!(r#"''"#, "");
    }

    #[test]
    fn sq_borrowed_when_no_escape() {
        let mut p = Parser::new(r#"'foo'"#);
        let cow = p.parse_single_quoted(0).unwrap();
        assert!(matches!(cow, Cow::Borrowed(_)));
    }

    #[test]
    fn sq_backslash_is_literal() {
        // single-quoted has no backslash escapes; \n is literal two chars
        assert_sq!(r#"'a\nb'"#, "a\\nb");
    }

    // single-quoted '' escape

    #[test]
    fn sq_escape_quote() {
        assert_sq!(r#"'it''s'"#, "it's");
    }

    #[test]
    fn sq_escape_quote_multiple() {
        assert_sq!(r#"'a''b''c'"#, "a'b'c");
    }

    #[test]
    fn sq_owned_when_escape() {
        let mut p = Parser::new(r#"'it''s'"#);
        let cow = p.parse_single_quoted(0).unwrap();
        assert!(matches!(cow, Cow::Owned(_)));
    }

    // single-quoted w/ folds

    #[test]
    fn sq_single_break_folds_to_space() {
        assert_sq!("'a\nb'", "a b");
    }

    #[test]
    fn sq_trailing_ws_before_break_stripped() {
        assert_sq!("'a   \nb'", "a b");
    }

    #[test]
    fn sq_more_indented_preserves_break() {
        assert_sq!("'a\n  b'", "a\n  b");
    }

    #[test]
    fn sq_crlf_fold() {
        assert_sq!("'a\r\nb'", "a b");
    }

    // single-quoted errors

    #[test]
    fn sq_err_unterminated() {
        let mut p = Parser::new(r#"'foo"#);
        assert!(p.parse_single_quoted(0).is_err());
    }

    // --- plain continuation lookahead (§7.3.3 s-ns-plain-next-line) ---

    macro_rules! assert_cont {
        ($src:literal, $min:literal, $expected:expr) => {
            let p = Parser::new($src);
            assert_eq!(p.peek_plain_continuation($min), $expected);
        };
    }

    #[test]
    fn cont_single_break() {
        assert_cont!("\n  two", 1, Some((1, 3)));
    }

    #[test]
    fn cont_counts_empty_lines() {
        assert_cont!("\n\n  two", 1, Some((2, 4)));
        assert_cont!("\n\n\n  two", 1, Some((3, 5)));
    }

    #[test]
    fn cont_crlf_counts_as_one_break() {
        assert_cont!("\r\n  two", 1, Some((1, 4)));
    }

    #[test]
    fn cont_indent_equal_to_min_qualifies() {
        // s-indent(n) demands at least n, not more than n
        assert_cont!("\n  two", 2, Some((1, 3)));
    }

    #[test]
    fn cont_indent_below_min_stops() {
        assert_cont!("\n  two", 3, None);
    }

    #[test]
    fn cont_dedented_sibling_stops() {
        assert_cont!("\nb: 2", 1, None);
    }

    #[test]
    fn cont_comment_line_stops() {
        assert_cont!("\n  # c", 1, None);
    }

    #[test]
    fn cont_eof_stops() {
        assert_cont!("\n", 0, None);
        assert_cont!("", 0, None);
        // trailing blank lines are not consumed as a continuation
        assert_cont!("\n   \n  \n", 0, None);
    }

    #[test]
    fn cont_requires_cursor_on_a_break() {
        assert_cont!("two", 0, None);
    }

    #[test]
    fn cont_tab_past_indent_is_separation() {
        // s-separate-in-line: tabs are legal once s-indent(n) is satisfied
        assert_cont!("\n \ttwo", 1, Some((1, 3)));
    }

    #[test]
    fn cont_tab_inside_indent_stops() {
        // indent counts spaces only; current_indent reports the tab downstream
        assert_cont!("\n\ttwo", 1, None);
    }

    #[test]
    fn cont_doc_markers_stop() {
        assert_cont!("\n---\ntwo", 0, None);
        assert_cont!("\n...\ntwo", 0, None);
    }

    #[test]
    fn cont_doc_marker_prefix_is_content() {
        // `---x` is not a marker
        assert_cont!("\n---x\n", 0, Some((1, 1)));
    }

    // --- fold allocation behaviour ---

    #[test]
    fn fold_single_line_stays_borrowed() {
        let mut p = Parser::new("one\n");
        let first = p.parse_plain_scalar(false);
        let folded = p.fold_plain_continuation(first, 0).unwrap();
        assert_eq!(folded, "one");
        assert!(matches!(folded, Cow::Borrowed(_)));
    }

    #[test]
    fn fold_continuation_allocates() {
        let mut p = Parser::new("one\n  two\n");
        let first = p.parse_plain_scalar(false);
        let folded = p.fold_plain_continuation(first, 1).unwrap();
        assert_eq!(folded, "one two");
        assert!(matches!(folded, Cow::Owned(_)));
    }
}
