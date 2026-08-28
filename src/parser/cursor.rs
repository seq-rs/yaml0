use crate::{
    Error, Parser,
    parser::{line_end, space, tab, whitespace},
};

impl<'a> Parser<'a> {
    /// Construct an `Error` annotated with the cursor's current line/col.
    /// Use this for any parser-side error so users see useful positions.
    pub(super) fn err(&self, msg: impl Into<String>) -> Error {
        Error {
            msg: msg.into(),
            line: Some(self.line),
            col: Some(self.col),
        }
    }

    pub(super) fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }

    pub(super) fn peek_at(&self, pos: usize) -> Option<u8> {
        self.src.as_bytes().get(pos).copied()
    }

    pub(super) fn advance(&mut self) {
        if let Some(b) = self.peek() {
            self.pos += 1;
            if line_end(b) {
                self.line += 1;
                self.col = 1;
            } else {
                self.col += 1;
            }
        }
    }

    pub(super) fn at_eof(&self) -> bool {
        self.peek().is_none()
    }

    pub(super) fn skip_spaces(&mut self) {
        while let Some(b) = self.peek() {
            if whitespace(b) {
                self.advance()
            } else {
                break;
            }
        }
    }

    pub(super) fn at_line_end(&self) -> bool {
        self.peek().is_none_or(line_end)
    }

    pub(super) fn consume_newline(&mut self) {
        if self.peek() == Some(b'\r') {
            self.advance();
        }
        if self.peek() == Some(b'\n') {
            self.advance();
        }
    }

    pub(super) fn skip_blank_and_comment_lines(&mut self) {
        loop {
            if self.at_eof() {
                return;
            }

            if self.current_line_is_blank() || self.current_line_is_comment() {
                self.skip_to_next_line();
            } else {
                return;
            }
        }
    }

    pub(super) fn current_line_is_blank(&self) -> bool {
        let mut i = self.pos;
        while let Some(b) = self.src.as_bytes().get(i).copied() {
            if line_end(b) {
                return true;
            }
            if !whitespace(b) {
                return false;
            }
            i += 1;
        }
        true
    }

    pub(super) fn current_line_is_comment(&self) -> bool {
        let mut i = self.pos;
        while let Some(b) = self.src.as_bytes().get(i).copied() {
            if !whitespace(b) {
                return matches!(b, b'#');
            }
            i += 1;
        }
        false
    }

    pub(super) fn skip_to_next_line(&mut self) {
        while let Some(b) = self.peek() {
            if line_end(b) {
                break;
            }
            self.advance();
        }
        self.consume_newline();
    }

    pub(super) fn current_indent(&self) -> crate::Result<usize> {
        let mut i = self.pos;
        while let Some(b) = self.peek_at(i) {
            // stop at non-whitespace character
            match b {
                b if tab(b) => {
                    return Err(Error {
                        msg: "parsing failed: illegal indent whitespace using \\t".into(),
                        line: Some(self.line),
                        col: Some(self.col),
                    });
                }
                b if space(b) => {
                    i += 1;
                }
                _ => break,
            };
        }
        Ok(i - self.pos)
    }

    pub(super) fn consume_line_fold(&mut self, out: &mut String, parent_indent: usize) {
        self.consume_one_line_break();

        let mut empty_lines = 0;
        let mut next_more_indented = false;

        loop {
            let mut stripped = 0;
            while stripped < parent_indent && matches!(self.peek(), Some(b' ' | b'\t')) {
                self.advance();
                stripped += 1;
            }

            match self.peek() {
                Some(b'\n' | b'\r') => {
                    empty_lines += 1;
                    self.consume_one_line_break();
                }
                Some(b' ' | b'\t') => {
                    next_more_indented = true;
                    break;
                }
                _ => break,
            }
        }
        if empty_lines == 0 {
            if next_more_indented {
                out.push('\n');
            } else {
                out.push(' ');
            }
        } else {
            for _ in 0..empty_lines {
                out.push('\n');
            }
        }
    }

    pub(super) fn consume_one_line_break(&mut self) {
        match self.peek() {
            Some(b'\r') => {
                self.advance();
                if self.peek() == Some(b'\n') {
                    self.advance();
                }
            }
            Some(b'\n') => self.advance(),
            _ => {}
        }
    }

    /// True when cursor is at column 1 and starts with `marker` followed
    /// by whitespace, newline, or EOF — the spec rule for `---` and `...`
    pub(super) fn at_doc_marker(&self, marker: &[u8]) -> bool {
        self.col == 1 && self.is_doc_marker_at(self.pos, marker)
    }

    pub(super) fn skip_flow_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b' '|b'\t'|b'\n'|b'\r' => self.advance(),
                b'#' => {
                    // # we see is always preceded by skipped whitespace or start-of-line
                    // both are valid comment-start contexts.
                    while let Some(b) = self.peek() {
                        if matches!(b, b'\n' | b'\r') {
                            break;
                        }
                        self.advance();
                    }
                }
                _ => break,
            }
        }
    }

}

#[cfg(test)]
mod tests {

    #[test]
    fn advance_tracks_line_col() {
        let mut parser = super::Parser::new("ab\ncd");
        parser.advance();
        parser.advance();
        parser.advance();
        parser.advance();
        assert_eq!((parser.line, parser.col), (2, 2))
    }

    #[test]
    fn skip_spaces_stops_at_newline() {
        let mut parser = super::Parser::new("    \nx");
        parser.skip_spaces();
        assert!(parser.at_line_end());
        assert_eq!(parser.peek(), Some(b'\n'));
    }

    #[test]
    fn skip_blank_skips_comments_and_blanks() {
        let mut parser = super::Parser::new("\n# x\n\n  y");
        parser.skip_blank_and_comment_lines();
        assert_eq!(parser.peek(), Some(b' '));
        parser.advance();
        assert_eq!(parser.peek(), Some(b' '));
        parser.advance();
        assert_eq!(parser.peek(), Some(b'y'));
    }

    #[test]
    fn current_indent_counts_leading_spaces() -> crate::Result<()> {
        let parser = super::Parser::new("    foo");
        let indent = parser.current_indent()?;
        assert_eq!(indent, 4);
        Ok(())
    }

    #[test]
    fn tab_in_indent_errors() {
        let parser = super::Parser::new("\tfoo");
        let err = parser.current_indent().unwrap_err();
        assert_eq!(err.line, Some(1));
        assert_eq!(err.col, Some(1));
    }
}
