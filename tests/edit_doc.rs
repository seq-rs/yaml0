#![cfg(feature = "edit")]

use std::borrow::Cow;
use std::fs;

use yaml0::{
    DocumentView, Document, SetOpts, SetOutcome, locate, path, set, set_with,
};

const SRC: &str = "\
# stack
services:
  web:
    image: nginx:1.25
    ports:
      - \"80:80\"
  db:
    image: postgres:16.1
";

// ---- DocumentView ----

#[test]
fn one_parse_answers_many_paths() {
    let view = DocumentView::parse(SRC).unwrap();

    for p in [
        vec![yaml0::Segment::Key("services"), yaml0::Segment::Key("web"), yaml0::Segment::Key("image")],
        vec![yaml0::Segment::Key("services"), yaml0::Segment::Key("db"), yaml0::Segment::Key("image")],
        vec![yaml0::Segment::Key("services"), yaml0::Segment::Key("web"), yaml0::Segment::Key("ports"), yaml0::Segment::Index(0)],
    ] {
        assert_eq!(
            view.locate(&p),
            locate(SRC, &p).unwrap(),
            "view disagrees with the free function on {p:?}"
        );
    }
}

#[test]
fn a_missing_path_is_none_not_an_error() {
    let view = DocumentView::parse(SRC).unwrap();
    assert!(view.locate(path!["nope"]).is_none());
}

/// A parse failure belongs to `parse`, which is why `locate` returns `Option`
/// rather than `Result`.
#[test]
fn parse_errors_surface_at_parse() {
    assert!(DocumentView::parse("unknown: *nope\n").is_err());
    assert!(DocumentView::parse("a: 1\n\tb: 2\n").is_err());
}

#[test]
fn documents_counts_the_stream() {
    let stream = fs::read_to_string("tests/fixtures/kubectl_stream.yaml").unwrap();
    let view = DocumentView::parse(&stream).unwrap();
    assert_eq!(view.document_count(), 2);

    assert_eq!(DocumentView::parse(SRC).unwrap().document_count(), 1);
}

#[test]
fn src_round_trips() {
    assert_eq!(DocumentView::parse(SRC).unwrap().src(), SRC);
}

// ---- Document ----

#[test]
fn edits_chain_without_rebinding() {
    let mut doc = Document::new(SRC);

    doc.set(path!["services", "web", "image"], "nginx:1.26").unwrap();
    doc.set(path!["services", "db", "image"], "postgres:16.2").unwrap();
    doc.set(path!["services", "web", "ports", 0], "80:8080").unwrap();

    let out = doc.into_string();
    assert!(out.contains("image: nginx:1.26"));
    assert!(out.contains("image: postgres:16.2"));
    assert!(out.contains("- \"80:8080\""), "quoting lost");
    assert!(out.starts_with("# stack\n"), "comment lost");
}

#[test]
fn as_str_reflects_each_edit() {
    let mut doc = Document::new(SRC);
    assert!(doc.as_str().contains("nginx:1.25"));

    doc.set(path!["services", "web", "image"], "nginx:1.26").unwrap();
    assert!(doc.as_str().contains("nginx:1.26"));
    assert!(!doc.as_str().contains("nginx:1.25"));
}

/// The outcome owns its names, so it stays usable across later edits. Without
/// `into_owned` this would not compile.
#[test]
fn an_outcome_outlives_the_next_edit() {
    let mut doc = Document::new("base: &b 1\nuse: *b\nalso: *b\n");

    let first = doc.set_with(path!["base"], "2", SetOpts::new().edit_anchors()).unwrap();
    doc.set_with(path!["use"], "3", SetOpts::new().edit_refs()).unwrap();

    let SetOutcome::PropagatedAnchor { name, refs } = first else {
        panic!("expected PropagatedAnchor");
    };
    assert_eq!((name.as_ref(), refs), ("b", 2));
}

#[test]
fn a_refused_edit_leaves_the_buffer_alone() {
    let mut doc = Document::new(SRC);
    assert!(doc.set(path!["services"], "gone").is_err());
    assert_eq!(doc.as_str(), SRC, "buffer changed despite the refusal");
}

#[test]
fn a_view_can_be_taken_between_edits() {
    let mut doc = Document::new(SRC);

    let before = {
        let view = doc.view().unwrap();
        view.locate(path!["services", "web", "image"]).unwrap().span
    };
    doc.set(path!["services", "web", "image"], "nginx:1.26").unwrap();

    let view = doc.view().unwrap();
    let after = view.locate(path!["services", "web", "image"]).unwrap().span;
    assert_eq!(before.start, after.start);
    assert_eq!(&doc.as_str()[after], "nginx:1.26");
}

/// The handle is a wrapper, not a second implementation.
#[test]
fn the_handle_matches_the_free_functions() {
    let mut doc = Document::new(SRC);
    doc.set(path!["services", "web", "image"], "nginx:1.26").unwrap();
    doc.set(path!["services", "db", "image"], "postgres:16.2").unwrap();

    let (out, _) = set(SRC, path!["services", "web", "image"], "nginx:1.26").unwrap();
    let (out, _) = set(&out, path!["services", "db", "image"], "postgres:16.2").unwrap();

    assert_eq!(doc.as_str(), out);
}

// ---- SetOutcome ownership ----

/// The free functions borrow their names out of the source; only the handle
/// pays to own them.
#[test]
fn free_functions_do_not_allocate_outcome_names() {
    let src = "base: &b 1\nuse: *b\n";
    let (_, outcome) = set_with(src, path!["base"], "2", SetOpts::new().edit_anchors()).unwrap();

    let SetOutcome::PropagatedAnchor { name, .. } = outcome else {
        panic!("expected PropagatedAnchor");
    };
    assert!(matches!(name, Cow::Borrowed(_)), "outcome name was cloned");
}

#[test]
fn into_owned_detaches_every_variant() {
    let src = "base: &b 1\nuse: *b\n";

    let (_, o) = set_with(src, path!["base"], "2", SetOpts::new().edit_anchors()).unwrap();
    assert!(matches!(o.into_owned(), SetOutcome::PropagatedAnchor { .. }));

    let (_, o) = set_with(src, path!["use"], "2", SetOpts::new().edit_refs()).unwrap();
    assert!(matches!(o.into_owned(), SetOutcome::ReplacedAlias { .. }));

    let (_, o) = set("a: 1\n", path!["a"], "2").unwrap();
    assert_eq!(o.into_owned(), SetOutcome::Spliced);
}
