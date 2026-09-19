use crate::{Error, Result, Segment};

/// Parse a path expression into segments
///
/// Keys are dot-separated, indices are bracketed, and a key containing '.',
/// '[' or a quote is written quoted. A leading '#N' selects a document.
///
/// ```
/// # use yaml0::{parse_path, Segment};
/// assert_eq!(
///     parse_path(r#"services.web.ports[0]"#)?,
///     vec![
///         Segment::Key("services"),
///         Segment::Key("web"),
///         Segment::Key("ports"),
///         Segment::Index(0),
///     ]
/// );
/// # Ok::<(), yaml0::Error>(())
/// ```
pub fn parse_path(path: &str) -> Result<Vec<Segment<'_>>> {
    let mut expect_separator = false;
    let bytes = path.as_bytes();
    let mut segments = Vec::new();

    let mut i = 0;

    if path.is_empty() {
        return Ok(segments);
    }

    if bytes[0] == b'#' {
        i += 1;
        let n = read_digits(path, &mut i, "a document number after '#'")?;
        segments.push(Segment::Doc(n));
        if i < bytes.len() {
            expect(path, &mut i, b'.')?;
            if i == bytes.len() {
                return Err(err(path, i, "trailing '.' in path"));
            }
        }
    }

    while i < bytes.len() {
        match bytes[i] {
            // separator of two keys or components
            b'.' => {
                if !expect_separator {
                    return Err(err(path, i, "empty path segment"));
                }
                i += 1;
                if i == bytes.len() {
                    return Err(err(path, i, "trailing '.' in path"));
                }
                expect_separator = false;
            }
            // indices chain onto previous element, no separator needed
            b'[' => {
                i += 1;
                let n = read_digits(path, &mut i, "an index")?;
                expect(path, &mut i, b']')?;
                segments.push(Segment::Index(n));
                expect_separator = true;
            }
            q @ (b'"' | b'\'') if !expect_separator => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != q {
                    i += 1;
                }
                if i == bytes.len() {
                    return Err(err(path, start, "unterminated quoted path segment"));
                }
                segments.push(Segment::Key(&path[start..i]));
                i += 1; // closing quote
                expect_separator = true;
            }
            _ if !expect_separator => {
                let start = i;
                while i < bytes.len() && !matches!(bytes[i], b'.' | b'[' | b']' | b'"' | b'\'') {
                    i += 1;
                }

                if i == start {
                    return Err(err(path, start, "empty path segment"));
                }

                segments.push(Segment::Key(&path[start..i]));
                expect_separator = true;
            }
            b']' => return Err(err(path, i, "unmatched ']'")),
            _ => return Err(err(path, i, "expected '.' between segments")),
        }
    }

    Ok(segments)
}

fn read_digits(path: &str, i: &mut usize, what: &str) -> Result<usize> {
    let start = *i;

    while let Some(d) = path.as_bytes().get(*i)
        && d.is_ascii_digit()
    {
        *i += 1;
    }

    path[start..*i]
        .parse::<usize>()
        .map_err(|_| err(path, start, &format!("expected {what}")))
}

fn expect(path: &str, i: &mut usize, b: u8) -> Result<()> {
    if path.as_bytes().get(*i) == Some(&b) {
        *i += 1;
        Ok(())
    } else {
        Err(err(
            path,
            *i,
            &format!("expected {} at position {i}", b as char),
        ))
    }
}

fn err(path: &str, at: usize, msg: &str) -> Error {
    Error {
        msg: format!("{msg} in path {path:?}"),
        line: None,
        col: Some(at),
    }
}
