use std::ops::Range;

use super::sink::{Entry, Shape, SpanId, SpanSink};
use super::{Located, NodeKind, Segment};
use crate::{Parser, Result};

pub(super) struct Resolved<'a> {
    pub(super) located: Located<'a>,
    pub(super) node: Option<SpanId>,
    /// An anchor definition passed through on the way down. Editing anything beneath it reaches
    /// that anchor's aliases too.
    pub(super) enclosing_anchor: Option<(&'a str, SpanId)>,
    /// Key extent of the entry that owns this value, when it has one.
    pub(super) key_span: Option<Range<usize>>,
    /// Descent stopped at an alias with path left over
    pub(super) truncated: bool,
    /// For a `MergeInherited` result, the node the key resolves to in the merge source. Its text is
    /// the model for an override.
    pub(super) inherited: Option<SpanId>,
}

pub(super) fn spans(src: &str) -> Result<(Box<SpanSink<'_>>, usize)> {
    let mut parser = Parser::with_spans(src);
    parser.parse_all()?;
    Ok((
        parser.take_sink(),
        src.len() - src.strip_prefix('\u{FEFF}').unwrap_or(src).len(),
    ))
}

pub(super) fn resolve<'a>(
    sink: &SpanSink<'a>,
    path: &[Segment<'_>],
    follow: bool,
) -> Option<Resolved<'a>> {
    let (doc, rest) = match path.split_first() {
        Some((Segment::Doc(n), rest)) => (*n, rest),
        _ => (0, path),
    };
    let &root = sink.roots().get(doc)?;
    descend(sink, root, rest, follow)
}

