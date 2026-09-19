//! Byte-span addressing over parsed YAML, for format-preserving edits.
//!
//! The parser records a span tree as it walks; [`locate`] resolves a path to
//! the source bytes it addresses and to what kind of node lives there.
#![cfg_attr(not(feature = "edit"), allow(dead_code))]

use std::ops::Range;

#[cfg(feature = "edit")]
mod apply;
#[cfg(feature = "edit")]
mod doc;
#[cfg(feature = "edit")]
mod edit_doc;
#[cfg(feature = "edit")]
mod locate;
#[cfg(feature = "edit")]
mod path;
#[cfg(feature = "edit")]
mod plan;
#[cfg(feature = "edit")]
mod set;
pub(crate) mod sink;

#[cfg(feature = "edit")]
pub use apply::{Applied, Change};
#[cfg(feature = "edit")]
pub use doc::{Document, DocumentView};
#[cfg(feature = "edit")]
pub use edit_doc::Edit;
#[cfg(feature = "edit")]
pub use locate::locate;
#[cfg(feature = "edit")]
pub use path::parse_path;
#[cfg(feature = "edit")]
pub use set::{SetOpts, SetOutcome, set, set_with};

/// A path segment representing one level of descent into a YAML stream.
///
/// [`Segment::Doc`] is only meaningful as the leading segment; absent, document 0.
/// Build paths with [`path!`](crate::path) or [`parse_path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment<'a> {
    /// Document index in a `---` separated (multi-doc) stream.
    Doc(usize),
    /// Mapping key, matched against the key's text rather than its source
    /// spelling, so `Key("a")` finds `"a": 1`.
    Key(&'a str),
    /// 0-based element index (sequence/array)
    Index(usize),
}

impl<'a> From<&'a str> for Segment<'a> {
    fn from(key: &'a str) -> Self {
        Self::Key(key)
    }
}

impl From<usize> for Segment<'_> {
    fn from(index: usize) -> Self {
        Self::Index(index)
    }
}

/// Describes what the located bytes are, helps decide if an edit is safe (breaks guarantees or not,
/// see [`SetOpts`])
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind<'a> {
    /// An ordinary value. Editing it changes nothing else.
    Literal,
    /// A `*name` token referencing an anchor's value.
    AliasRef(&'a str),
    /// The key is not present at this path. It arrives through `<<`.
    MergeInherited,
}

/// Holds information about the node a path resolves to in the stream
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located<'a> {
    /// Byte range into the source passed to [`locate`], ready to slice.
    pub span: Range<usize>,
    /// What the bytes are, which decides whether an edit is safe.
    pub kind: NodeKind<'a>,
    /// Anchor this node defines, if any, so editing it reaches every alias.
    pub anchor: Option<&'a str>,
}

#[cfg(all(test, feature = "edit"))]
mod tests {
    use crate::Parser;

    /// One entry per construct the parser can walk. Recording must not change
    /// what any of them parse to, and must leave no frame open.
    const CORPUS: &[&str] = &[
        "",
        "42\n",
        "just a scalar\n",
        "a: 1\nb: two\n",
        "a: 1   # trailing comment\n",
        "# leading comment\na: 1\n",
        "outer:\n  inner:\n    deep: 1\n",
        "- a\n- b\n",
        "- a\n\n- b\n",
        "list:\n  - one\n  - two\n",
        "list:\n- compact\n- seq\n",
        "items:\n  - k: v\n    j: w\n  - k: v2\n",
        "empty:\nafter: 1\n",
        "a:\n\n\nb: 1\n",
        "quoted: \"a b\"\nsingle: 'c d'\n",
        "escaped: \"tab\\there\"\n",
        "\"quoted key\": 1\n",
        "42: numeric key\n",
        "? explicit\n: value\n",
        "? [a, b]\n: value\n",
        "flow: [1, 2, 3]\n",
        "flow: {a: 1, b: 2}\n",
        "[a, b]: flow key\n",
        "block: |\n  line one\n  line two\n",
        "folded: >\n  line one\n  line two\n",
        "strip: |-\n  no trailing\n",
        "base: &b 1\nuse: *b\n",
        "base: &b\n  k: v\nuse: *b\n",
        "base: &b\n  k: v\nchild:\n  <<: *b\n  own: 1\n",
        "one: &a {x: 1}\ntwo: &c {y: 2}\nm:\n  <<: [*a, *c]\n",
        "tagged: !!str 42\ncustom: !mine thing\n",
        "anchored: !!int &n 7\n",
        "---\na: 1\n---\nb: 2\n",
        "---\na: 1\n...\n",
        "%YAML 1.2\n---\na: 1\n",
        "a: 1\r\nb: 2\r\n",
        "unicode: café\n",
        "unicode: café   # comment\n",
        "\u{FEFF}bom: 1\n",
        "multi: line one\n  continued\n",
        "deep:\n  - - nested\n    - seqs\n",
    ];

    /// The parse must be identical whether or not spans are being recorded.
    fn assert_span_neutral(src: &str) {
        let plain = Parser::new(src).parse_all();
        let spanned = Parser::with_spans(src).parse_all();

        match (plain, spanned) {
            (Ok(a), Ok(b)) => assert_eq!(a, b, "values diverged for {src:?}"),
            (Err(_), Err(_)) => {}
            (a, b) => panic!("outcome diverged for {src:?}: {a:?} vs {b:?}"),
        }
    }

    #[test]
    fn recording_does_not_change_parse_results() {
        for src in CORPUS {
            assert_span_neutral(src);
        }
    }

    /// Inputs that must fail. Recording must not swallow or invent an error,
    /// and must not leave the frame stack unbalanced on the way out.
    #[test]
    fn recording_does_not_change_errors() {
        for src in [
            "a: 1\n\tb: 2\n",
            "unknown: *nope\n",
            "&: 1\n",
            "[a, b\n",
            "{a: 1\n",
            "key\n  spanning: 1\n",
            "a: 1\nb: 2\n---\nc: 3\nbare\n",
        ] {
            assert_span_neutral(src);
        }
    }

    /// `finish` asserts the frame stack is empty; every corpus entry exercises
    /// it, so an unbalanced open/close fails the suite in debug builds.
    #[test]
    fn frames_balance_on_every_corpus_entry() {
        for src in CORPUS {
            let _ = Parser::with_spans(src).parse_all();
        }
    }
}
