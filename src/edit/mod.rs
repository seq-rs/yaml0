//! Byte-span addressing over parsed YAML, for format-preserving edits.
//!
//! The parser records a span tree as it walks; [`locate`] resolves a path to
//! the source bytes it addresses and to what kind of node lives there.
#![cfg_attr(not(feature = "edit"), allow(dead_code))]

use std::ops::Range;

#[cfg(feature = "edit")]
mod path;
pub(crate) mod sink;
#[cfg(feature = "edit")]
pub use path::parse_path;
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

