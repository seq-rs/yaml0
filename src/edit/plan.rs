use std::ops::Range;

use super::locate::Resolved;
use super::set::{SetOpts, count_refs, refused};
use super::sink::{Shape, SpanId, SpanSink};
use crate::{Result, SetOutcome};

pub(super) struct Plan<'a> {
    pub span: Range<usize>,
    pub model: &'a str,
    pub outcome: SetOutcome<'a>,
}

pub(super) fn plan_edit<'a>(
    sink: &SpanSink<'a>,
    src: &'a str,
    resolved: &Resolved<'a>,
    span: Range<usize>,
    bom: usize,
    opts: &SetOpts,
) -> Result<Plan<'a>> {
    let is_leaf = |id: SpanId| matches!(sink.node(id).shape, Shape::Leaf);

    let propagation = match (resolved.located.anchor, resolved.node) {
        (Some(name), Some(id)) => Some((name, id)),
        _ => resolved.enclosing_anchor,
    };

    if let Some((name, _)) = propagation
        && !opts.edit_anchors
    {
        return Err(refused(&format!(
            "this edit reaches the anchor '&{name}', SetOpts::edit_anchors allows it"
        )));
    }

    let propagated = |sink: &SpanSink<'a>| {
        propagation.map(|(name, def)| SetOutcome::PropagatedAnchor {
            name: std::borrow::Cow::Borrowed(name),
            refs: count_refs(sink, def),
        })
    };

    match resolved.located.kind {
        super::NodeKind::MergeInherited => Err(refused(
            "key is inherited through '<<', SetOpts::override_inherited writes an explicit key that overrides it",
        )),
        super::NodeKind::AliasRef(name) => {
            if resolved.truncated {
                return Err(refused(&format!(
                    "path continues past the alias '*{name}', expansion is unsupported"
                )));
            }

            if !opts.edit_refs {
                return Err(refused(&format!(
                    "'*{name}' is an alias. SetOpts::edit_refs allows severing the link here."
                )));
            }

            let def = sink
                .anchor_before(name, resolved.located.span.start)
                .ok_or_else(|| refused(&format!("unknown anchor '{name}'")))?;

            if !is_leaf(def) && !opts.replace_nodes {
                return Err(refused(
                    "alias points to a collection, SetOpts::replace_nodes allows replacing it",
                ));
            }

            let model_span = shift_range(sink.node(def).span.clone(), bom);
            Ok(Plan {
                span,
                model: &src[model_span],
                // Widest effect applies
                // Severing the link is also an anchor edit when this node carries one.
                outcome: propagated(sink).unwrap_or(SetOutcome::ReplacedAlias {
                    name: std::borrow::Cow::Borrowed(name),
                }),
            })
        }

        super::NodeKind::Literal => {
            let id = resolved.node.expect("a literal is a node");
            let collection = !is_leaf(id);

            if collection && !opts.replace_nodes {
                return Err(refused(
                    "path addresses a collection, SetOpts::replace_nodes allows replacing it",
                ));
            }

            let outcome = propagated(sink).unwrap_or(if collection {
                SetOutcome::ReplacedNode
            } else {
                SetOutcome::Spliced
            });

            let model = &src[span.clone()];
            Ok(Plan {
                span,
                model,
                outcome,
            })
        }
    }
}

pub(super) fn shift_range(r: Range<usize>, by: usize) -> Range<usize> {
    r.start + by..r.end + by
}
