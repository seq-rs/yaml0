## Format-preserving edits

*Requires the `edit` feature.*

A serde round-trip is not intended to preserve a file's formatting or full content and context.
[`Deserialize`](serde::Deserialize) consumes the data part of the data format's expected type,
builds a value from a stream of typed events, but does not preserve comments, syntax (quoting, spacing),
order of keys, anchored values, etc. [`Serialize`](serde::Serialize) similarly does not output just what it's
intended to, which are the data points [`Deserialize`](serde::Deserialize) parses.
So `from_str` followed by `to_string` gives you the same *data* in a deterministic way,
but without regard for user-written comments or syntactical choices

The `edit` module works directly on the source text instead. The parser records a byte span
for every node as it walks, so a value can be replaced in place and everything around it stays as-is:

```
use yaml0::{path, set};

let src = "\
image: nginx:1.25   # bump on release
replicas: 2
";

let (out, _) = set(src, path!["image"], "nginx:1.26")?;

// The comment, its spacing, and every other line are untouched.
assert_eq!(out, "\
image: nginx:1.26   # bump on release
replicas: 2
");
# Ok::<(), yaml0::Error>(())
```

### Paths

[`path!`](crate::path) builds a path at compile time, where each argument is a key or an
index, and strings are never split, so a key containing dots needs no escaping.

[`parse_path`] reads the same path at runtime using a jq-like syntax, where a key containing
`.`, `[` or a quote must be within quotes.

```
use yaml0::{parse_path, path, Segment};

assert_eq!(
    parse_path(r#"metadata.annotations."kubernetes.io/ingress.class""#)?,
    path!["metadata", "annotations", "kubernetes.io/ingress.class"],
);
# Ok::<(), yaml0::Error>(())
```

A leading [`Segment::Doc`] selects a document in a `---` separated stream.

### Editing guarantees

Not every path is safe to overwrite or change. [`locate`] reports where a path lands and what
lives there, as a [`NodeKind`]: an ordinary value ([`NodeKind::Literal`]), a `*name` alias standing in for an
anchor ([`NodeKind::AliasRef`]), or a key that is not present at all because it arrives through `<<` ([`NodeKind::MergeInherited`]).
The [`Located::anchor`] field says whether the node also *defines* an anchor, in which
case editing it reaches every alias to it.

[`set`] refuses anything more complex than the bytes you named. By default it changes no type,
destroys no structure, and affects no other node.

Each [`SetOpts`] method gives up exactly one of those guarantees, and [`set_with`] applies them with the guarantee set you defined:

```
use yaml0::{path, set, set_with, SetOpts, SetOutcome};

let src = "base: &b 1\nuse: *b\nalso: *b\n";

// Refused: this would change what `use` and `also` read.
assert!(set(src, path!["base"], "2").is_err());

let (out, outcome) = set_with(src, path!["base"], "2", SetOpts::new().edit_anchors())?;
assert_eq!(out, "base: &b 2\nuse: *b\nalso: *b\n");
assert_eq!(outcome, SetOutcome::PropagatedAnchor { name: "b".into(), refs: 2 });
# Ok::<(), yaml0::Error>(())
```

The returned [`SetOutcome`] always reports the most impactful edit defined, even if it was refused.

### Multiple edits

[`set`] and [`locate`] parse the document each time. [`DocumentView`] parses once and
can keep querying without re-parsing. [`Document`] owns the buffer and performs edits in place,
so it can be used to perform *multiple edits*.

```
use yaml0::{path, Document};

let mut doc = Document::new("a: 1\nb: 2\n");
doc.set(path!["a"], "10")?;
doc.set(path!["b"], "20")?;
assert_eq!(doc.as_str(), "a: 10\nb: 20\n");
# Ok::<(), yaml0::Error>(())
```

### Limits

- Inserting non-existent keys is refused
- Reordering a sequence (array) is refused
- Removing a key is refused
- Block scalars keep their value, but not their *chomp* style (`|`)
- Editing values reachable through alias needs inlining the alias first (NOT SUPPORTED YET)
