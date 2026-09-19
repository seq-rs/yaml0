use std::ops::Range;

use super::sink::{Entry, Shape, SpanId, SpanSink};
use super::{Located, NodeKind, Segment};
use crate::edit::doc::DocumentView;
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

/// Resolve a path to the source bytes it addresses.
///
/// Returns `None` when the path doesn't exist. A path running *through* an
/// alias stops at the alias rather than following it. Spans index `src` as
/// given, so they can be spliced directly.
///
/// # Example
///
/// ```
/// # use yaml0::{Segment, NodeKind, locate};
/// let src = "image: nginx:1.0\n";
/// let found = locate(src, &[Segment::Key("image")])?.unwrap();
/// assert_eq!(&src[found.span], "nginx:1.0");
/// assert_eq!(found.kind, NodeKind::Literal);
/// # Ok::<(), yaml0::Error>(())
/// ```
pub fn locate<'a, 'p>(
    src: &'a str,
    path: impl AsRef<[Segment<'p>]>,
) -> Result<Option<Located<'a>>> {
    Ok(DocumentView::parse(src)?.locate(path))
}

pub(super) fn shift<'a>(located: Located<'a>, bom: usize) -> Located<'a> {
    Located {
        span: located.span.start + bom..located.span.end + bom,
        kind: located.kind,
        anchor: located.anchor, //FLAG
    }
}

fn descend<'a>(
    sink: &SpanSink<'a>,
    root: SpanId,
    path: &[Segment<'_>],
    follow: bool,
) -> Option<Resolved<'a>> {
    let mut id = root;
    let mut enclosing_anchor = None;
    let mut key_span = None;

    for seg in path {
        if let NodeKind::AliasRef(name) = sink.node(id).kind {
            if !follow {
                let node = sink.node(id);
                return Some(Resolved {
                    located: Located {
                        span: node.span.clone(),
                        kind: NodeKind::AliasRef(name),
                        anchor: node.anchor.as_ref().map(|a| a.name),
                    },
                    node: Some(id),
                    enclosing_anchor,
                    key_span,
                    truncated: true,
                    inherited: None,
                });
            }
            id = follow_alias(sink, id)?;
        }

        let node = sink.node(id);

        // Anything below an anchor definition reaches that anchor's aliases
        if let Some(a) = &node.anchor {
            enclosing_anchor = Some((a.name, id));
        }

        id = match (seg, &node.shape) {
            (Segment::Index(n), Shape::Seq(items)) => {
                key_span = None;
                *items.get(*n)?
            }
            (Segment::Key(k), Shape::Map(entries)) => match entry(entries, k) {
                Some(e) => {
                    key_span = Some(e.key_span.clone());
                    e.value
                }
                None => return merge_lookup(sink, entries, k, enclosing_anchor),
            },
            _ => return None,
        };
    }

    if follow {
        id = follow_alias(sink, id)?;
    }

    let node = sink.node(id);
    Some(Resolved {
        located: Located {
            span: node.span.clone(),
            kind: node.kind,
            anchor: node.anchor.as_ref().map(|a| a.name),
        },
        node: Some(id),
        enclosing_anchor,
        key_span,
        truncated: false,
        inherited: None,
    })
}

fn entry<'e, 'a>(entries: &'e [Entry<'a>], key: &str) -> Option<&'e Entry<'a>> {
    entries.iter().find(|e| e.key_text.as_deref() == Some(key))
}

fn lookup(entries: &[Entry<'_>], key: &str) -> Option<SpanId> {
    entry(entries, key).map(|e| e.value)
}

fn merge_lookup<'a>(
    sink: &SpanSink<'a>,
    entries: &[Entry<'_>],
    key: &str,
    enclosing_anchor: Option<(&'a str, SpanId)>,
) -> Option<Resolved<'a>> {
    for e in entries
        .iter()
        .filter(|e| e.key_text.as_deref() == Some("<<"))
    {
        for source in merge_sources(sink, e.value) {
            if let Some(inherited) = find_key(sink, source, key) {
                return Some(Resolved {
                    located: Located {
                        span: e.span.clone(),
                        kind: NodeKind::MergeInherited,
                        anchor: None,
                    },
                    node: None,
                    enclosing_anchor,
                    key_span: None,
                    truncated: false,
                    inherited: Some(inherited),
                });
            }
        }
    }
    None
}

fn merge_sources(sink: &SpanSink<'_>, value: SpanId) -> Vec<SpanId> {
    let node = sink.node(value);
    match (&node.kind, &node.shape) {
        (NodeKind::AliasRef(name), _) => sink
            .anchor_before(name, node.span.start)
            .into_iter()
            .collect(),
        (_, Shape::Seq(items)) => items
            .iter()
            .filter_map(|&i| {
                let item = sink.node(i);
                match item.kind {
                    NodeKind::AliasRef(n) => sink.anchor_before(n, item.span.start),
                    _ => None,
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn find_key(sink: &SpanSink<'_>, id: SpanId, key: &str) -> Option<SpanId> {
    let Shape::Map(entries) = &sink.node(id).shape else {
        return None;
    };

    if let Some(v) = lookup(entries, key) {
        return Some(v);
    }

    entries
        .iter()
        .filter(|e| e.key_text.as_deref() == Some("<<"))
        .flat_map(|e| merge_sources(sink, e.value))
        .find_map(|s| find_key(sink, s, key))
}

/// Resolve alias chain to the node it actually names
///
/// Each hop lands on an anchor defined earlier in the source, so it always terminates
fn follow_alias(sink: &SpanSink<'_>, mut id: SpanId) -> Option<SpanId> {
    while let NodeKind::AliasRef(name) = sink.node(id).kind {
        id = sink.anchor_before(name, sink.node(id).span.start)?;
    }
    Some(id)
}
