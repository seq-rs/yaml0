use std::{borrow::Cow, ops::Range};

use super::locate::{Resolved, resolve, spans};
use super::plan::{plan_edit, shift_range};
use super::sink::{SpanId, SpanSink};
use crate::{
    BorrowedValue, Error, NodeKind, Result, Segment,
    emitter::emit_double_quoted,
    patterns::{
        has_ctrl_chars, has_leading_or_trailing_space, has_newline, resolve_scalar,
        starts_with_indicator,
    },
};

/// Relax [`set`]'s guarantees (allowed operations) by passing [`SetOpts`] to [`set_with`] instead.
///
/// By default an edit touches only the bytes the path names: it changes no
/// type, destroys no structure, and affects no other node. Each method below
/// lifts exactly one of those restrictions.
///
/// ```
/// # use yaml0::{SetOpts, path, set_with};
/// let src = "base: &b 1\nuse: *b\n";
/// let (out, _) = set_with(src, path!["use"], "2", SetOpts::new().edit_refs())?;
/// assert_eq!(out, "base: &b 1\nuse: 2\n");
/// # Ok::<(), yaml0::Error>(())
/// ```
#[derive(Default, Clone, Debug)]
pub struct SetOpts {
    /// Let a plain value change what it resolves to.
    ///
    /// Without this, replacing plain `hello` with `42` writes `'42'` so the
    /// field stays a string. Quoted values keep their quotes either way.
    pub(super) allow_type_change: bool,
    /// Allow a collection to be written over by a scalar.
    ///
    /// The mapping or sequence at the path is replaced wholesale.
    pub(super) replace_nodes: bool,
    /// Allow editing an anchor definition, or any value inside one.
    ///
    /// Every alias to that anchor reads the new value. The outcome reports
    /// how many.
    pub(super) edit_anchors: bool,
    /// Allow editing at an alias site, severing the link there.
    ///
    /// The `*name` token is replaced by the new value. The anchor and every
    /// other alias to it are left alone.
    pub(super) edit_refs: bool,
    /// Edit the anchor a reference points at, rather than the reference site.
    ///
    /// Implies [`SetOpts::edit_anchors`], since the bytes it changes belong to
    /// the anchor. The `*name` token stays where it is.
    pub(super) follow_refs: bool,
    /// Allow writing a value into a mapping entry that has none.
    ///
    /// `key:` becomes `key: value`. A sequence item has no key to write
    /// after, so an empty one is still refused.
    pub(super) insert_empty: bool,
    /// Write an explicit key that overrides one inherited through
    /// `<<`.
    pub(super) override_inherited: bool,
}

impl SetOpts {
    /// Options that give nothing up.
    pub fn new() -> Self {
        Self::default()
    }

    /// Let a plain value change what it resolves to.
    ///
    /// Without this, replacing plain `hello` with `42` writes `'42'` so the
    /// field stays a string. Quoted values keep their quotes either way.
    pub fn allow_type_change(mut self) -> Self {
        self.allow_type_change = true;
        self
    }

    /// Allow a collection to be written over by a scalar.
    ///
    /// The mapping or sequence at the path is replaced wholesale.
    pub fn replace_nodes(mut self) -> Self {
        self.replace_nodes = true;
        self
    }

    /// Allow editing an anchor definition, or any value inside one.
    ///
    /// Every alias to that anchor reads the new value. The outcome reports
    /// how many.
    pub fn edit_anchors(mut self) -> Self {
        self.edit_anchors = true;
        self
    }

    /// Allow editing at an alias site, severing the link there.
    ///
    /// The `*name` token is replaced by the new value. The anchor and every
    /// other alias to it are left alone.
    pub fn edit_refs(mut self) -> Self {
        self.edit_refs = true;
        self
    }

    /// Edit the anchor a reference points at, rather than the reference site.
    ///
    /// Implies [`SetOpts::edit_anchors`], since the bytes it changes belong to
    /// the anchor. The `*name` token stays where it is.
    pub fn follow_refs(mut self) -> Self {
        self.edit_anchors = true;
        self.follow_refs = true;
        self
    }

    /// Allow writing a value into a mapping entry that has none.
    ///
    /// `key:` becomes `key: value`. A sequence item has no key to write
    /// after, so an empty one is still refused.
    pub fn insert_empty(mut self) -> Self {
        self.insert_empty = true;
        self
    }

    /// Write an explicit key that overrides one inherited through
    /// `<<`.
    pub fn override_inherited(mut self) -> Self {
        self.override_inherited = true;
        self
    }
}

/// Change result from a [`set`] or [`set_with`] operation
///
/// When more than one applies, the largest impact change's outcome is reported:
/// `PropagatedAnchor` > `ReplacedAlias` > `ReplacedNode` > `Inserted` > `Spliced`
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SetOutcome<'a> {
    /// The addressed bytes were replaced, and nothing else.
    Spliced,
    /// An anchor definition changed, so `refs` alias sites now read the new
    /// value too.
    PropagatedAnchor { name: Cow<'a, str>, refs: usize },
    /// A mapping or sequence was overwritten by a scalar.
    ReplacedNode,
    /// An alias token was replaced with a value overwriting the reference
    ReplacedAlias { name: Cow<'a, str> },
    /// A new value was written into an entry without previous value present
    Inserted,
}

impl SetOutcome<'_> {
    pub fn into_owned(self) -> SetOutcome<'static> {
        match self {
            SetOutcome::Spliced => SetOutcome::Spliced,
            SetOutcome::PropagatedAnchor { name, refs } => SetOutcome::PropagatedAnchor {
                name: Cow::<'static, str>::Owned(name.into_owned()),
                refs,
            },
            SetOutcome::ReplacedNode => SetOutcome::ReplacedNode,
            SetOutcome::ReplacedAlias { name } => SetOutcome::ReplacedAlias {
                name: Cow::Owned(name.into_owned()),
            },
            SetOutcome::Inserted => SetOutcome::Inserted,
        }
    }
}

/// Replace the scalar at `path`, preserving everything around it.
///
/// Comments, key order, indentation and blank lines are unaffected. The quoting style of the source
/// survives, because otherwise `"3.9"` would turn into a float.
///
/// Check the returned [`SetOutcome`].
///
/// By default, it refuses anything wider than the bytes you named: an alias, an anchor definition
/// or a value inside one, a collection, an inherited key or an empty value. Use [`set_with`] with
/// [`SetOpts`] to change behavior.
///
/// # Example
///
/// ```
/// # use yaml0::{path, set};
/// let src = "# stack\nimage: nginx:1.25   # bump me\nreplicas: 2\n";
/// let (out, _) = set(src, path!["image"], "nginx:1.26")?;
/// assert_eq!(out, "# stack\nimage: nginx:1.26   # bump me\nreplicas: 2\n");
/// # Ok::<(), yaml0::Error>(())
/// ```
pub fn set<'a, 'p>(
    src: &'a str,
    path: impl AsRef<[Segment<'p>]>,
    value: &str,
) -> Result<(String, SetOutcome<'a>)> {
    set_with(src, path, value, SetOpts::default())
}

/// [`set`] with chosen options on how the edits will or will not be performed.
///
/// The returned [`SetOutcome`] reports the biggest impact outcome, whether or not it was allowed
/// by the options.
///
/// See [`SetOpts`] for the description of possible options.
///
/// # Example
///
/// ```
/// # use yaml0::{SetOpts, SetOutcome, path, set_with};
/// let src = "base: &b 1\nuse: *b\nalso: *b\n";
/// let (out, outcome) = set_with(src, path!["base"], "2", SetOpts::new().edit_anchors())?;
///
/// assert_eq!(out, "base: &b 2\nuse: *b\nalso: *b\n");
/// assert_eq!(outcome, SetOutcome::PropagatedAnchor { name: "b".into(), refs: 2 });
/// # Ok::<(), yaml0::Error>(())
/// ```
pub fn set_with<'a, 'p>(
    src: &'a str,
    path: impl AsRef<[Segment<'p>]>,
    value: &str,
    opts: SetOpts,
) -> Result<(String, SetOutcome<'a>)> {
    let (sink, bom) = spans(src)?;
    let resolved = resolve(&sink, path.as_ref(), opts.follow_refs)
        .ok_or_else(|| refused("no node at that path"))?;
    let span = shift_range(resolved.located.span.clone(), bom);

    if span.is_empty() {
        return insert_empty_value(&sink, src, &resolved, value, bom, &opts);
    }

    if matches!(resolved.located.kind, NodeKind::MergeInherited) && opts.override_inherited {
        return override_inherited(
            &sink,
            src,
            &resolved,
            path.as_ref(),
            value,
            span,
            bom,
            &opts,
        );
    }

    let plan = plan_edit(&sink, src, &resolved, span, bom, &opts)?;

    let mut out = String::with_capacity(src.len() - plan.span.len() + value.len() + 2);
    out.push_str(&src[..plan.span.start]);
    render_scalar(plan.model, value, &opts, &mut out);
    out.push_str(&src[plan.span.end..]);

    Ok((out, plan.outcome))
}

#[allow(clippy::too_many_arguments)]
fn override_inherited<'a>(
    sink: &SpanSink<'a>,
    src: &str,
    resolved: &Resolved<'a>,
    path: &[Segment<'_>],
    value: &str,
    merge_entry: Range<usize>,
    bom: usize,
    opts: &SetOpts,
) -> Result<(String, SetOutcome<'a>)> {
    let Some(Segment::Key(key)) = path.last() else {
        return Err(refused("only a key name can override an inherited one"));
    };

    if let Some((name, _)) = resolved.enclosing_anchor
        && !opts.edit_anchors
    {
        return Err(refused(&format!(
            "this map is inside the anchor '&{name}', SetOpts::edit_anchors allows editing it"
        )));
    }

    let line_start = src[..merge_entry.start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &src[line_start..merge_entry.start];
    if !indent.bytes().all(|b| b == b' ') {
        return Err(refused("cannot add an override inside a flow mapping"));
    }

    let line_end = src[merge_entry.end..]
        .find('\n')
        .map_or(src.len(), |i| merge_entry.end + i);

    let mut out = String::with_capacity(src.len() + key.len() + value.len() + indent.len() + 4);
    out.push_str(&src[..line_end]);
    out.push('\n');
    out.push_str(indent);
    render_new(key, &mut out);
    out.push_str(": ");

    match resolved.inherited {
        Some(id) => {
            let model = &src[shift_range(sink.node(id).span.clone(), bom)];
            render_scalar(model, value, opts, &mut out);
        }
        None => render_new(value, &mut out),
    }
    out.push_str(&src[line_end..]);

    let outcome = match resolved.enclosing_anchor {
        Some((name, def)) => SetOutcome::PropagatedAnchor {
            name: std::borrow::Cow::Borrowed(name),
            refs: count_refs(sink, def),
        },
        None => SetOutcome::Inserted,
    };

    Ok((out, outcome))
}

fn render_scalar(model: &str, value: &str, opts: &SetOpts, out: &mut String) {
    match model.as_bytes().first() {
        Some(b'"') => emit_double_quoted(value, out),
        Some(b'\'') => emit_quoted(value, out),
        _ if !opts.allow_type_change && retypes(model, value) => emit_quoted(value, out),
        _ => render_new(value, out),
    }
}

fn render_new(value: &str, out: &mut String) {
    if needs_quotes_strict(value) {
        emit_quoted(value, out);
    } else {
        out.push_str(value);
    }
}

fn retypes(model: &str, value: &str) -> bool {
    let was_string = matches!(resolve_scalar(model.into()), BorrowedValue::String(_));
    let is_string = matches!(resolve_scalar(value.into()), BorrowedValue::String(_));
    was_string != is_string
}

fn insert_empty_value<'a>(
    sink: &SpanSink<'a>,
    src: &str,
    resolved: &Resolved<'a>,
    value: &str,
    bom: usize,
    opts: &SetOpts,
) -> Result<(String, SetOutcome<'a>)> {
    if !opts.insert_empty {
        return Err(refused(
            "current value is empty, SetOpts::insert_empty allows creating it",
        ));
    }

    let Some(key_span) = resolved.key_span.clone() else {
        return Err(refused("only an empty mapping value can be filled in"));
    };

    if let Some((name, _)) = resolved.enclosing_anchor
        && !opts.edit_anchors
    {
        return Err(refused(&format!(
            "this value is inside the anchor '&{name}', SetOpts::edit_anchors allows editing it",
        )));
    }

    let out = insert_after_key(src, shift_range(key_span, bom), value);

    let outcome = match resolved.enclosing_anchor {
        Some((name, def)) => SetOutcome::PropagatedAnchor {
            name: std::borrow::Cow::Borrowed(name),
            refs: count_refs(sink, def),
        },
        None => SetOutcome::Inserted,
    };

    Ok((out, outcome))
}

fn insert_after_key(src: &str, key_span: Range<usize>, value: &str) -> String {
    let line_end = src[key_span.end..]
        .find('\n')
        .map_or(src.len(), |i| key_span.end + i);
    let colon = src[key_span.end..line_end]
        .find(':')
        .map(|i| key_span.end + i);

    let mut out = String::with_capacity(src.len() + value.len() + 2);
    match colon {
        Some(c) => {
            let mut after = c + 1;
            while src.as_bytes().get(after) == Some(&b' ') {
                after += 1;
            }
            out.push_str(&src[..after]);
            if after == c + 1 {
                out.push(' ');
            }

            render_new(value, &mut out);
            out.push_str(&src[after..]);
        }
        None => {
            out.push_str(&src[..key_span.end]);
            out.push_str(": ");
            render_new(value, &mut out);
            out.push_str(&src[key_span.end..]);
        }
    }
    out
}

pub(super) fn count_refs(sink: &SpanSink<'_>, def: SpanId) -> usize {
    sink.alias_sites()
        .filter(|&(name, at)| sink.anchor_before(name, at) == Some(def))
        .count()
}

pub(super) fn refused(msg: &str) -> Error {
    Error {
        msg: msg.to_string(),
        line: None,
        col: None,
    }
}

fn needs_quotes_strict(s: &str) -> bool {
    s.is_empty()
        || has_ctrl_chars(s)
        || has_newline(s)
        || has_leading_or_trailing_space(s)
        || starts_with_indicator(s)
        || s.contains(": ")
        || s.ends_with(':')
        || s.contains(" #")
}

fn emit_quoted(value: &str, out: &mut String) {
    if has_ctrl_chars(value) || has_newline(value) {
        emit_double_quoted(value, out);
    } else {
        emit_single_quoted(value, out);
    }
}

fn emit_single_quoted(value: &str, out: &mut String) {
    out.push('\'');
    for c in value.chars() {
        if c == '\'' {
            out.push('\''); // YAML escapes a quote by doubling it
        }
        out.push(c);
    }
    out.push('\'');
}
