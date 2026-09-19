use std::borrow::Cow;

use serde::Serialize;

use crate::{BorrowedValue, Document, Result, Segment, SetOpts, SetOutcome, edit::set::refused};

/// Changes made by [`apply`](Document::apply), ordered as the edits themselves.
#[derive(Debug, Clone, Default)]
pub struct Applied {
    pub changes: Vec<Change>,
}

/// Change on the [`Document`] by [`apply`](Document::apply).
///
/// [`path`](Change::Set::path) is dotted, for diagnostics only. Keys containing dots are not
/// escaped, so it is not intended to be used as input or edit definition, only as a result after a
/// completed edit.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Change {
    Set {
        path: String,
        outcome: SetOutcome<'static>,
    },
    Removed {
        path: String,
    },
}

impl Document {
    /// Overwrite source with edited struct where the changes produced a diff
    ///
    /// `before` is the document prior to any edits, `after` is with the changes already made, this
    /// way they can be diffed when writing output.
    ///
    /// Needed, because e.g. a field that was not in the deserialized data may default to
    /// [`Option::None`], but it needs to be differentiated against fields that were there but had
    /// their values removed in the changes. Comparing `before` and `after` gives back the changes
    /// made between them and only apply the edits to the source at the edited spots.
    ///
    /// Only scalar values are rewritten, and a field cleared to `None` removes
    /// its key. Anything structural is refused before a single byte is written,
    /// so a half-applied document is not possible.
    ///
    /// # Errors
    ///
    /// Refuses when the difference adds a key, changes a sequence's length,
    /// changes a value's shape, or gives a value to a field that had none.
    pub fn apply_with<T: ?Sized + Serialize>(
        &mut self,
        before: &T,
        after: &T,
        opts: SetOpts,
    ) -> Result<Applied> {
        let (b, a) = (crate::to_value(before)?, crate::to_value(after)?);

        let (mut ops, mut problems) = (Vec::new(), Vec::new());
        collect(&b, &a, &mut Vec::new(), &mut ops, &mut problems);

        // Refuse and interrupt before writing anything, so half-applied document is impossible
        if !problems.is_empty() {
            return Err(refused(&format!(
                "apply cannot make structural changes: {}",
                problems.join("; ")
            )));
        }

        let mut changes = Vec::new();
        for (steps, op) in ops {
            let p: Vec<Segment<'_>> = steps.iter().map(Step::as_segment).collect();
            let path = show(&steps);
            match op {
                Op::Set(text) => {
                    let outcome = self.set_with(&p, &text, opts.clone())?;
                    changes.push(Change::Set { path, outcome });
                }
                Op::Remove => {
                    self.remove(&p)?;
                    changes.push(Change::Removed { path });
                }
            }
        }
        Ok(Applied { changes })
    }
}

/// A path collected before any edit, so it outlives the buffer
#[derive(Clone)]
enum Step {
    Key(String),
    Index(usize),
}

impl Step {
    fn as_segment(&self) -> Segment<'_> {
        match self {
            Self::Key(k) => Segment::Key(k.as_str()),
            Self::Index(i) => Segment::Index(*i),
        }
    }
}

/// An edit the walk decided on, to be carried out once the whole diff is known.
enum Op {
    Set(String),
    Remove,
}

/// Walk `before` against `after`, recording edits and refusals.
///
/// Both come from the same type, so their shapes match and every difference is
/// deliberate. Anything this cannot express as a scalar splice or a key removal
/// goes to `problems`, which aborts the apply before it writes.
fn collect(
    before: &BorrowedValue<'_>,
    after: &BorrowedValue<'_>,
    path: &mut Vec<Step>,
    out: &mut Vec<(Vec<Step>, Op)>,
    problems: &mut Vec<String>,
) {
    use BorrowedValue::*;

    // An untouched subtree costs one comparison, which is what keeps this cheap
    // against a struct whose every absent field is present as None.
    if before == after {
        return;
    }

    match (before, after) {
        (Map(b), Map(a)) => {
            for (k, bv) in b {
                let Some(key) = scalar_text(k) else { continue };
                path.push(Step::Key(key.clone().into_owned()));
                match find(a, key.as_ref()) {
                    // Gone from `after`: the field was cleared.
                    None => out.push((path.clone(), Op::Remove)),
                    Some(av) => collect(bv, av, path, out, problems),
                }
                path.pop();
            }
            for (k, _) in a {
                let Some(key) = scalar_text(k) else { continue };
                if find(b, key.as_ref()).is_none() {
                    path.push(Step::Key(key.into_owned()));
                    problems.push(format!("{}: adding a key", show(path)));
                    path.pop();
                }
            }
        }

        (Seq(b), Seq(a)) if b.len() == a.len() => {
            for (i, (bv, av)) in b.iter().zip(a).enumerate() {
                path.push(Step::Index(i));
                collect(bv, av, path, out, problems);
                path.pop();
            }
        }
        (Seq(b), Seq(a)) => problems.push(format!(
            "{}: sequence length {} -> {}",
            show(path),
            b.len(),
            a.len()
        )),

        // Nothing to splice over: the path is absent from the document, so this
        // would fail partway through the apply rather than up front.
        (Null, _) => problems.push(format!("{}: giving a value to an absent field", show(path))),

        // Clearing a field reads as removal. There is no way to ask for a
        // literal `null`, which is far rarer than deleting a key.
        (_, Null) => out.push((path.clone(), Op::Remove)),

        (_, a) => match scalar_text(a) {
            Some(text) => out.push((path.clone(), Op::Set(text.into_owned()))),
            None => problems.push(format!("{}: shape changed", show(path))),
        },
    }
}

/// A scalar's text form, for handing to `set_with`. `None` for collections.
fn scalar_text<'a>(v: &BorrowedValue<'a>) -> Option<Cow<'a, str>> {
    use BorrowedValue::*;
    match v {
        String(s) => Some(s.clone()),
        Bool(b) => Some(Cow::Borrowed(if *b { "true" } else { "false" })),
        Int(n) => Some(Cow::Owned(n.to_string())),
        Float(f) => Some(Cow::Owned(f.to_string())),
        _ => None,
    }
}

/// A dotted rendering of `path`, for diagnostics.
fn show(path: &[Step]) -> String {
    path.iter()
        .map(|s| match s {
            Step::Key(k) => k.clone(),
            Step::Index(n) => n.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// The value `key` maps to, matched on key text rather than source spelling.
fn find<'e, 'a>(
    entries: &'e [(BorrowedValue<'a>, BorrowedValue<'a>)],
    key: &str,
) -> Option<&'e BorrowedValue<'a>> {
    entries
        .iter()
        .find(|(k, _)| scalar_text(k).as_deref() == Some(key))
        .map(|(_, v)| v)
}
