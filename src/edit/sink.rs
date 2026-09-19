#![cfg_attr(not(feature = "edit"), allow(dead_code))]

use super::NodeKind;
use crate::parser::trim_trailing_whitespace_end;
use std::{borrow::Cow, ops::Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct SpanId(pub(super) u32);

pub(super) struct SpanNode<'a> {
    pub(super) span: Range<usize>,
    /// Value type: literal/alias of literal
    pub(super) kind: NodeKind<'a>,
    /// Set when the node carries `&name`, independently of `kind`
    pub(super) anchor: Option<Anchor<'a>>,
    pub(super) shape: Shape<'a>,
}

/// An anchor definition on a node
pub(super) struct Anchor<'a> {
    pub(super) name: &'a str,
    /// Then `&name` token, for structural edits
    #[allow(dead_code)]
    pub(super) token: Range<usize>,
}

pub(super) enum Shape<'a> {
    Leaf,
    Seq(Vec<SpanId>),
    Map(Vec<Entry<'a>>),
}

pub(super) struct Entry<'a> {
    /// Key extent, used to place a value into an empty mapping entry
    pub(super) key_span: Range<usize>,
    pub(super) key_text: Option<Cow<'a, str>>,
    pub(super) value: SpanId,
    pub(super) span: Range<usize>,
}

pub(super) struct Frame<'a> {
    pub(super) start: usize,
    pub(super) kind: NodeKind<'a>,
    pub(super) children: Vec<SpanId>,
    pub(super) entries: Vec<Entry<'a>>,
    pub(super) pending_key: Option<(Range<usize>, Option<Cow<'a, str>>)>,
    pub(super) delimited: bool,
}

impl<'a> Frame<'a> {
    fn new(start: usize, delimited: bool) -> Self {
        Self {
            start,
            kind: NodeKind::Literal,
            children: Vec::new(),
            entries: Vec::new(),
            pending_key: None,
            delimited,
        }
    }
}

#[derive(Default)]
pub(crate) struct SpanSink<'a> {
    nodes: Vec<SpanNode<'a>>,
    stack: Vec<Frame<'a>>,
    roots: Vec<SpanId>,
    last_closed: Option<SpanId>,
    anchors: Vec<(&'a str, SpanId)>,
}

impl<'a> SpanSink<'a> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(super) fn node(&self, id: SpanId) -> &SpanNode<'a> {
        &self.nodes[id.0 as usize]
    }

    pub(super) fn roots(&self) -> &[SpanId] {
        &self.roots
    }

    pub(super) fn anchor_before(&self, name: &str, before: usize) -> Option<SpanId> {
        self.anchors
            .iter()
            .rev()
            .find(|(n, id)| *n == name && self.node(*id).span.start < before)
            .map(|(_, id)| *id)
    }


    /// Alias sites in source order, returning anchor name and the `*name` token starts
    pub(super) fn alias_sites(&self) -> impl Iterator<Item = (&'a str, usize)> + '_ {
        self.nodes.iter().filter_map(|n| match n.kind {
            NodeKind::AliasRef(name) => Some((name, n.span.start)),
            _ => None,
        })
    }

    pub fn open(&mut self, start: usize, delimited: bool) {
        self.last_closed = None;
        self.stack.push(Frame::new(start, delimited));
    }

    pub fn close(&mut self, pos_hint: usize, src: &'a str) {
        let Some(frame) = self.stack.pop() else {
            return;
        };
        debug_assert!(frame.pending_key.is_none(), "map key without a value node");

        // Containers end at their last child: cursor has already skipped past trailing blank
        // and comment lines.
        let end = if frame.delimited {
            trimmed_end(src, frame.start, pos_hint)
        } else if let Some(e) = frame.entries.last() {
            e.span.end
        } else if let Some(&id) = frame.children.last() {
            self.node(id).span.end
        } else {
            trimmed_end(src, frame.start, pos_hint)
        };

        let shape = if !frame.entries.is_empty() {
            Shape::Map(frame.entries)
        } else if !frame.children.is_empty() {
            Shape::Seq(frame.children)
        } else {
            Shape::Leaf
        };

        let id = self.push(
            SpanNode {
                span: frame.start..end.max(frame.start),
                kind: frame.kind,
                anchor: None,
                shape,
            },
            src,
        );
        self.attach(id);
    }

    pub fn empty(&mut self, at: usize, src: &'a str) {
        let id = self.push(
            SpanNode {
                span: at..at,
                kind: NodeKind::Literal,
                anchor: None,
                shape: Shape::Leaf,
            },
            src,
        );
        self.attach(id);
    }

    pub fn key(&mut self, span: Range<usize>, text: Option<Cow<'a, str>>) {
        if let Some(f) = self.stack.last_mut() {
            f.pending_key = Some((span, text));
        }
    }

    pub fn promote_last_child_to_key(&mut self, text: Option<Cow<'a, str>>) {
        let Some(id) = self.stack.last_mut().and_then(|f| f.children.pop()) else {
            return;
        };

        let span = self.node(id).span.clone();
        if let Some(f) = self.stack.last_mut() {
            f.pending_key = Some((span, text));
        }
    }

    pub fn set_anchor(&mut self, name: &'a str, token: Range<usize>) {
        let Some(id) = self.last_closed else { return };
        self.nodes[id.0 as usize].anchor = Some(Anchor { name, token });
        self.anchors.push((name, id));
    }

    pub fn kind_open(&mut self, kind: NodeKind<'a>) {
        if let Some(f) = self.stack.last_mut() {
            f.kind = kind;
        }
    }

    pub fn finish(&self) {
        debug_assert!(self.stack.is_empty(), "unbalanced span frames");
    }

    fn push(&mut self, node: SpanNode<'a>, src: &str) -> SpanId {
        debug_assert!(node.span.start <= node.span.end && node.span.end <= src.len());
        debug_assert!(src.is_char_boundary(node.span.start) && src.is_char_boundary(node.span.end));

        let id = SpanId(self.nodes.len() as u32);
        self.nodes.push(node);
        self.last_closed = Some(id);
        id
    }

    fn attach(&mut self, id: SpanId) {
        let end = self.node(id).span.end;
        let Some(frame) = self.stack.last_mut() else {
            self.roots.push(id);
            return;
        };
        match frame.pending_key.take() {
            Some((key_span, key_text)) => {
                let span = key_span.start..end;
                frame.entries.push(Entry {
                    key_span,
                    key_text,
                    value: id,
                    span,
                })
            }
            None => frame.children.push(id),
        }
    }
}

/// Where a node's bytes really end.
///
/// Plain scalars stop the cursor on the whitespace before a comment or line
/// break; block scalars consume past their terminating break. Neither belongs
/// to the value, so a splice replaces the value and nothing else.
fn trimmed_end(src: &str, start: usize, pos_hint: usize) -> usize {
    let hint = pos_hint.max(start).min(src.len());
    let bytes = &src.as_bytes()[start..hint];

    let mut n = bytes.len();
    while n > 0 && matches!(bytes[n - 1], b'\n' | b'\r') {
        n -= 1;
    }
    start + trim_trailing_whitespace_end(&bytes[..n])
}
