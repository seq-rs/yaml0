use serde::Serialize;

use super::locate::{resolve, shift, spans};
use super::sink::SpanSink;
use crate::edit::apply::Applied;
use crate::edit::plan::shift_range;
use crate::edit::set::refused;
use crate::{Located, NodeKind, Result, Segment, SetOpts, SetOutcome};

/// A parsed view of a source string, for visiting many paths without
/// re-parsing.
///
/// Read-only and borrowed. To edit, use [`Document`].
///
/// Mostly used internally to be able to diff and traverse, but effective to query fields.
pub struct DocumentView<'a> {
    src: &'a str,
    sink: Box<SpanSink<'a>>,
    bom: usize,
}

impl<'a> DocumentView<'a> {
    /// Parse a YAML string into a path-searchable Document
    pub fn parse(src: &'a str) -> Result<Self> {
        let (sink, bom) = spans(src)?;
        Ok(Self { src, sink, bom })
    }

    /// Resolve a path on a parsed document
    ///
    /// Since the document is already parsed, a non-existent path returns [`None`], and not [`Err`].
    pub fn locate<'p>(&self, path: impl AsRef<[Segment<'p>]>) -> Option<Located<'a>> {
        resolve(&self.sink, path.as_ref(), false).map(|r| shift(r.located, self.bom))
    }

    /// Document count in the stream for [`Segment::Doc`]
    pub fn document_count(&self) -> usize {
        self.sink.roots().len()
    }

    /// Document source string
    pub fn src(&self) -> &'a str {
        self.src
    }
}

/// An owned buffer that edits in place.
///
/// Each edit re-parses rather than caching a span tree. A splice shifts every
/// span after it, so a cached tree would be stale the moment anything changed.
/// For many reads against one parse, take a [`DocumentView`] with
/// [`view`](Document::view).
///
/// ```
/// # use yaml0::{Document, path};
/// let mut doc = Document::new("services:\n  web:\n    image: nginx:1.25\n");
///
/// // Several reads share one parse.
/// {
///     let view = doc.view()?;
///     assert!(view.locate(path!["services", "web", "image"]).is_some());
/// }
///
/// // Then edit, without rebinding the source each time.
/// doc.set(path!["services", "web", "image"], "nginx:1.26")?;
/// assert_eq!(doc.as_str(), "services:\n  web:\n    image: nginx:1.26\n");
/// # Ok::<(), yaml0::Error>(())
/// ```
pub struct Document {
    pub(super) src: String,
}

impl Document {
    pub fn new(src: impl Into<String>) -> Self {
        Self { src: src.into() }
    }

    pub fn set<'p>(
        &mut self,
        path: impl AsRef<[Segment<'p>]>,
        value: &str,
    ) -> Result<SetOutcome<'static>> {
        self.set_with(path, value, SetOpts::new())
    }

    pub fn set_with<'p>(
        &mut self,
        path: impl AsRef<[Segment<'p>]>,
        value: &str,
        opts: SetOpts,
    ) -> Result<SetOutcome<'static>> {
        let (out, outcome) = crate::set_with(&self.src, path, value, opts)?;
        let outcome = outcome.into_owned();
        self.src = out;
        Ok(outcome)
    }

    /// Borrow a parsed view for several reads between edits.
    pub fn view(&self) -> Result<DocumentView<'_>> {
        DocumentView::parse(&self.src)
    }

    pub fn as_str(&self) -> &str {
        &self.src
    }

    pub fn into_string(self) -> String {
        self.src
    }

    /// Remove the mapping entry at `path`, both key and value.
    ///
    /// Whole lines are removed, so a multi-line value is removed with its key. A comment on the
    /// key's own line is not kept either. A comment above the key is kept.
    pub fn remove<'p>(&mut self, path: impl AsRef<[Segment<'p>]>) -> Result<()> {
        let (sink, bom) = spans(&self.src)?;
        let resolved = resolve(&sink, path.as_ref(), false)
            .ok_or_else(|| refused("no node at given path"))?;

        if matches!(resolved.located.kind, NodeKind::MergeInherited) {
            return Err(refused("key is inherited through '<<', not present on the document"));
        }

        if resolved.located.anchor.is_some() || resolved.enclosing_anchor.is_some() {
            return Err(refused("removing an anchored value would break its aliases"));
        }

        let Some(key_span) = resolved.key_span.clone() else {
            return Err(refused("only a mapping entry can be removed"));
        };

        let start = shift_range(key_span, bom).start;
        let end = resolved.located.span.end + bom;

        let line_start = self.src[..start].rfind('\n').map_or(0, |i| i + 1);
        if !self.src[line_start..start].bytes().all(|b| b == b' ') {
            return Err(refused("cannot remove an entry from a flow mapping"));
        }

        // take the trailing newline with it, so no blank line remains
        let line_end = self.src[end..]
            .find('\n')
            .map_or(self.src.len(), |i| end + i + 1);

        let mut out = String::with_capacity(self.src.len());
        out.push_str(&self.src[..line_start]);
        out.push_str(&self.src[line_end..]);
        self.src = out;
        Ok(())
    }

    pub fn apply<T: ?Sized + Serialize>(&mut self, before: &T, after: &T) -> Result<Applied> {
        self.apply_with(before, after, SetOpts::default())
    }

}
