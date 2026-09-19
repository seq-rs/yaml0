#![cfg(feature = "edit")]

use std::ops::Range;
use yaml0::{Located, NodeKind, Segment, locate};

fn span(src: &str, needle: &str) -> Range<usize> {
    let start = src.find(needle).unwrap_or_else(|| panic!("{needle:?} not in source"));
    start..start + needle.len()
}

fn at(src: &str, path: &[Segment<'_>]) -> Located<'static> {
    let found = locate(src, path).expect("parse failed");
    let found = found.unwrap_or_else(|| panic!("no node at {path:?}"));
    // Spans are plain offsets; detach from the borrow for easier assertions.
    Located {
        span: found.span,
        kind: kind_owned(found.kind),
        anchor: found.anchor.map(leak),
    }
}

fn kind_owned(k: NodeKind<'_>) -> NodeKind<'static> {
    match k {
        NodeKind::Literal => NodeKind::Literal,
        NodeKind::MergeInherited => NodeKind::MergeInherited,
        NodeKind::AliasRef(n) => NodeKind::AliasRef(leak(n)),
    }
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

// ---- scalars ----

#[test]
fn plain_literal() {
    let src = "key: value\n";
    let l = at(src, &[Segment::Key("key")]);
    assert_eq!(l.span, span(src, "value"));
    assert_eq!(l.kind, NodeKind::Literal);
}

#[test]
fn quoted_literal_span_includes_the_quotes() {
    let src = "key: \"value\"\n";
    assert_eq!(at(src, &[Segment::Key("key")]).span, span(src, "\"value\""));
}

#[test]
fn trailing_comment_is_not_part_of_the_span() {
    let src = "key: value    # explanation\n";
    assert_eq!(at(src, &[Segment::Key("key")]).span, span(src, "value"));
}

#[test]
fn multibyte_scalar_with_trailing_comment() {
    let src = "café: naïve  # comment\n";
    let l = at(src, &[Segment::Key("café")]);
    assert_eq!(l.span, span(src, "naïve"));
    assert_eq!(&src[l.span], "naïve");
}

#[test]
fn crlf_offsets_are_byte_accurate() {
    let src = "a: 1\r\nb: 2\r\n";
    let l = at(src, &[Segment::Key("b")]);
    assert_eq!(&src[l.span.clone()], "2");
    assert_eq!(l.span, span(src, "2"));
}

#[test]
fn tagged_value_span_excludes_the_tag() {
    let src = "n: !!int 42\n";
    assert_eq!(at(src, &[Segment::Key("n")]).span, span(src, "42"));
}

#[test]
fn block_scalar_extent() {
    let src = "script: |\n  one\n  two\nnext: 1\n";
    let l = at(src, &[Segment::Key("script")]);
    assert_eq!(&src[l.span], "|\n  one\n  two");
}

#[test]
fn empty_value_is_a_zero_width_insertion_point() {
    let src = "key:\nnext: 1\n";
    let l = at(src, &[Segment::Key("key")]);
    assert!(l.span.is_empty(), "expected zero-width, got {:?}", l.span);
    assert!(l.span.start <= src.find("next").unwrap());
}

// ---- structure ----

#[test]
fn nested_map_path() {
    let src = "outer:\n  inner:\n    deep: found\n";
    assert_eq!(
        at(src, &[Segment::Key("outer"), Segment::Key("inner"), Segment::Key("deep")]).span,
        span(src, "found")
    );
}

#[test]
fn seq_index_path() {
    let src = "list:\n  - one\n  - two\n";
    assert_eq!(at(src, &[Segment::Key("list"), Segment::Index(1)]).span, span(src, "two"));
}

#[test]
fn seq_of_maps() {
    let src = "items:\n  - name: a\n  - name: b\n";
    assert_eq!(
        at(src, &[Segment::Key("items"), Segment::Index(1), Segment::Key("name")]).span,
        span(src, "b")
    );
}

#[test]
fn compact_seq_form() {
    let src = "list:\n- one\n- two\n";
    assert_eq!(at(src, &[Segment::Key("list"), Segment::Index(0)]).span, span(src, "one"));
}

#[test]
fn container_extent_stops_at_last_child() {
    let src = "outer:\n  a: 1\n  b: 2\n\n# trailing comment\nnext: 3\n";
    let l = at(src, &[Segment::Key("outer")]);
    assert_eq!(&src[l.span], "a: 1\n  b: 2");
}

/// A flow container ends at its own closing delimiter, not at its last child
/// the way a block container does.
#[test]
fn flow_container_span_includes_its_delimiters() {
    let src = "ports: [80, 443]\n";
    assert_eq!(at(src, &[Segment::Key("ports")]).span, span(src, "[80, 443]"));

    let src = "opts: {a: 1, b: 2}\n";
    assert_eq!(at(src, &[Segment::Key("opts")]).span, span(src, "{a: 1, b: 2}"));
}

#[test]
fn flow_seq_items_are_addressable() {
    let src = "ports: [80, 443]\n";
    assert_eq!(at(src, &[Segment::Key("ports"), Segment::Index(1)]).span, span(src, "443"));
    assert!(
        locate(src, &[Segment::Key("ports"), Segment::Index(2)]).unwrap().is_none(),
        "index past the end"
    );
}

#[test]
fn flow_map_keys_are_addressable() {
    let src = "opts: {a: 1, b: 2}\n";
    assert_eq!(at(src, &[Segment::Key("opts"), Segment::Key("b")]).span, span(src, "2"));
    assert!(locate(src, &[Segment::Key("opts"), Segment::Key("z")]).unwrap().is_none());
}

#[test]
fn nested_flow_containers() {
    let src = "grid: [[1, 2], [3]]\n";
    let p = [Segment::Key("grid"), Segment::Index(0), Segment::Index(1)];
    assert_eq!(at(src, &p).span, span(src, "2"));

    let src = "items: [{a: 1}, {b: 2}]\n";
    let p = [Segment::Key("items"), Segment::Index(1), Segment::Key("b")];
    assert_eq!(at(src, &p).span, span(src, "2"));
}

#[test]
fn flow_items_keep_their_quoting() {
    let src = "ports: [\"80:80\", '443']\n";
    assert_eq!(
        at(src, &[Segment::Key("ports"), Segment::Index(0)]).span,
        span(src, "\"80:80\"")
    );
    assert_eq!(
        at(src, &[Segment::Key("ports"), Segment::Index(1)]).span,
        span(src, "'443'")
    );
}

#[test]
fn flow_trailing_comma() {
    let src = "ports: [80,]\n";
    assert_eq!(at(src, &[Segment::Key("ports"), Segment::Index(0)]).span, span(src, "80"));
    assert!(locate(src, &[Segment::Key("ports"), Segment::Index(1)]).unwrap().is_none());
}

/// `{a, b}` and `{a: }` both give the key a null value, which is a zero-width
/// node rather than no node at all.
#[test]
fn flow_implicit_nulls_are_zero_width() {
    for src in ["opts: {a, b}\n", "opts: {a: , b: 2}\n"] {
        let l = at(src, &[Segment::Key("opts"), Segment::Key("a")]);
        assert!(l.span.is_empty(), "{src:?} gave {:?}", l.span);
    }
}

#[test]
fn an_alias_inside_a_flow_seq_keeps_its_kind() {
    let src = "base: &b 1\nrefs: [*b, 2]\n";
    let l = at(src, &[Segment::Key("refs"), Segment::Index(0)]);
    assert_eq!(l.kind, NodeKind::AliasRef("b"));
    assert_eq!(&src[l.span], "*b");
}

#[test]
fn an_anchor_inside_a_flow_seq_is_registered() {
    let src = "items: [&r 2]\nuse: *r\n";
    let l = at(src, &[Segment::Key("items"), Segment::Index(0)]);
    assert_eq!(l.anchor, Some("r"), "anchor inside flow not recorded");
    // and it resolves from outside the container
    assert_eq!(at(src, &[Segment::Key("use")]).kind, NodeKind::AliasRef("r"));
}

// ---- key matching ----

#[test]
fn quoted_key_matches_by_its_text() {
    let src = "\"my key\": value\n";
    assert_eq!(at(src, &[Segment::Key("my key")]).span, span(src, "value"));
}

#[test]
fn numeric_key_matches_by_its_text() {
    let src = "80: http\n";
    assert_eq!(at(src, &[Segment::Key("80")]).span, span(src, "http"));
}

#[test]
fn explicit_key_matches_by_its_text() {
    let src = "? explicit\n: value\n";
    assert_eq!(at(src, &[Segment::Key("explicit")]).span, span(src, "value"));
}

#[test]
fn collection_key_is_not_addressable() {
    let src = "[a, b]: value\n";
    assert!(locate(src, &[Segment::Key("[a, b]")]).unwrap().is_none());
}

// ---- anchors, aliases, merges ----

#[test]
fn anchor_def_spans_the_value_not_the_token() {
    let src = "base: &b 42\n";
    let l = at(src, &[Segment::Key("base")]);
    assert_eq!(l.span, span(src, "42"));
    assert_eq!(l.anchor, Some("b"));
    assert_eq!(l.kind, NodeKind::Literal, "the value itself is an ordinary scalar");
}

#[test]
fn anchor_def_on_a_collection() {
    let src = "base: &b\n  k: v\nother: 1\n";
    let l = at(src, &[Segment::Key("base")]);
    assert_eq!(&src[l.span], "k: v");
    assert_eq!(l.anchor, Some("b"));
}

#[test]
fn descending_into_an_anchored_map_still_works() {
    let src = "base: &b\n  k: v\n";
    let l = at(src, &[Segment::Key("base"), Segment::Key("k")]);
    assert_eq!(l.span, span(src, "v"));
    assert_eq!(l.kind, NodeKind::Literal);
}

#[test]
fn alias_ref_spans_the_token() {
    let src = "base: &b 42\nuse: *b\n";
    let l = at(src, &[Segment::Key("use")]);
    assert_eq!(l.span, span(src, "*b"));
    assert_eq!(l.kind, NodeKind::AliasRef("b"));
}

#[test]
fn path_through_an_alias_stops_at_the_alias() {
    let src = "base: &b\n  k: v\nuse: *b\n";
    let l = at(src, &[Segment::Key("use"), Segment::Key("k")]);
    assert_eq!(l.span, span(src, "*b"));
    assert_eq!(l.kind, NodeKind::AliasRef("b"));
}

#[test]
fn merge_inherited_key_reports_the_merge_entry() {
    let src = "base: &b\n  image: nginx\napp:\n  <<: *b\n  port: 80\n";
    let l = at(src, &[Segment::Key("app"), Segment::Key("image")]);
    assert_eq!(l.kind, NodeKind::MergeInherited);
    assert_eq!(&src[l.span], "<<: *b");
}

#[test]
fn own_key_wins_over_an_inherited_one() {
    let src = "base: &b\n  port: 1\napp:\n  <<: *b\n  port: 80\n";
    let l = at(src, &[Segment::Key("app"), Segment::Key("port")]);
    assert_eq!(l.span, span(src, "80"));
    assert_eq!(l.kind, NodeKind::Literal);
}

#[test]
fn merge_from_a_seq_of_aliases() {
    let src = "a: &a\n  x: 1\nb: &b\n  y: 2\nm:\n  <<:\n    - *a\n    - *b\n";
    assert_eq!(at(src, &[Segment::Key("m"), Segment::Key("x")]).kind, NodeKind::MergeInherited);
    assert_eq!(at(src, &[Segment::Key("m"), Segment::Key("y")]).kind, NodeKind::MergeInherited);
}

#[test]
fn merge_from_a_flow_seq_of_aliases() {
    let src = "a: &a\n  x: 1\nb: &b\n  y: 2\nm:\n  <<: [*a, *b]\n";
    assert_eq!(at(src, &[Segment::Key("m"), Segment::Key("x")]).kind, NodeKind::MergeInherited);
    assert_eq!(at(src, &[Segment::Key("m"), Segment::Key("y")]).kind, NodeKind::MergeInherited);
    assert!(locate(src, &[Segment::Key("m"), Segment::Key("z")]).unwrap().is_none());
}

/// An anchor on a flow map, merged through a flow list. This shape is in the
/// span-neutrality corpus and now resolves end to end.
#[test]
fn merge_from_flow_anchors_through_a_flow_list() {
    let src = "one: &a {x: 1}\ntwo: &c {y: 2}\nm:\n  <<: [*a, *c]\n";
    assert_eq!(at(src, &[Segment::Key("m"), Segment::Key("x")]).kind, NodeKind::MergeInherited);
    assert_eq!(at(src, &[Segment::Key("m"), Segment::Key("y")]).kind, NodeKind::MergeInherited);
}

#[test]
fn nested_merge_resolves_transitively() {
    let src = "root: &r\n  deep: 1\nmid: &m\n  <<: *r\napp:\n  <<: *m\n";
    assert_eq!(at(src, &[Segment::Key("app"), Segment::Key("deep")]).kind, NodeKind::MergeInherited);
}

#[test]
fn merge_present_but_key_absent_is_none() {
    let src = "base: &b\n  image: nginx\napp:\n  <<: *b\n";
    assert!(locate(src, &[Segment::Key("app"), Segment::Key("absent")]).unwrap().is_none());
}

#[test]
fn merge_key_itself_is_addressable() {
    let src = "base: &b\n  k: v\napp:\n  <<: *b\n";
    let l = at(src, &[Segment::Key("app"), Segment::Key("<<")]);
    assert_eq!(l.kind, NodeKind::AliasRef("b"));
}

// ---- misses ----

#[test]
fn missing_key_is_none() {
    assert!(locate("a: 1\n", &[Segment::Key("b")]).unwrap().is_none());
}

#[test]
fn index_out_of_range_is_none() {
    assert!(locate("- a\n", &[Segment::Index(5)]).unwrap().is_none());
}

#[test]
fn key_into_a_seq_is_none() {
    assert!(locate("- a\n", &[Segment::Key("a")]).unwrap().is_none());
}

#[test]
fn index_into_a_map_is_none() {
    assert!(locate("a: 1\n", &[Segment::Index(0)]).unwrap().is_none());
}

#[test]
fn path_past_a_scalar_is_none() {
    assert!(locate("a: 1\n", &[Segment::Key("a"), Segment::Key("b")]).unwrap().is_none());
}

#[test]
fn parse_error_propagates() {
    assert!(locate("unknown: *nope\n", &[Segment::Key("unknown")]).is_err());
}

// ---- documents ----

#[test]
fn empty_path_locates_the_document_root() {
    let src = "a: 1\nb: 2\n";
    assert_eq!(&src[at(src, &[]).span], "a: 1\nb: 2");
}

#[test]
fn doc_segment_selects_a_document() {
    let src = "---\nkind: Pod\n---\nkind: Service\n";
    let l = at(src, &[Segment::Doc(1), Segment::Key("kind")]);
    assert_eq!(l.span, span(src, "Service"));
}

#[test]
fn absent_doc_segment_means_document_zero() {
    let src = "---\nkind: Pod\n---\nkind: Service\n";
    assert_eq!(at(src, &[Segment::Key("kind")]).span, span(src, "Pod"));
}

#[test]
fn doc_index_out_of_range_is_none() {
    assert!(locate("a: 1\n", &[Segment::Doc(3), Segment::Key("a")]).unwrap().is_none());
}

#[test]
fn empty_document_still_holds_a_root_slot() {
    let src = "---\n---\nkind: Service\n";
    assert_eq!(at(src, &[Segment::Doc(1), Segment::Key("kind")]).span, span(src, "Service"));
}

// ---- source preamble ----

#[test]
fn bom_does_not_shift_spans() {
    let src = "\u{FEFF}key: value\n";
    let l = at(src, &[Segment::Key("key")]);
    assert_eq!(&src[l.span.clone()], "value", "span must index the caller's buffer");
    assert_eq!(l.span, span(src, "value"));
}

#[test]
fn directives_do_not_shift_spans() {
    let src = "%YAML 1.2\n---\nkey: value\n";
    assert_eq!(&src[at(src, &[Segment::Key("key")]).span], "value");
}

// ---- anchor scoping ----

#[test]
fn redefined_anchor_resolves_to_the_definition_in_effect() {
    let src = "one: &b\n  x: 1\nuse:\n  <<: *b\ntwo: &b\n  y: 2\n";
    assert_eq!(at(src, &[Segment::Key("use"), Segment::Key("x")]).kind, NodeKind::MergeInherited);
    assert!(locate(src, &[Segment::Key("use"), Segment::Key("y")]).unwrap().is_none());
}

#[test]
fn anchor_names_do_not_leak_across_documents() {
    let src = "---\nbase: &b\n  x: 1\napp:\n  <<: *b\n---\nbase: &b\n  y: 2\n";
    let p = [Segment::Doc(0), Segment::Key("app")];
    assert_eq!(at(src, &[p[0], p[1], Segment::Key("x")]).kind, NodeKind::MergeInherited);
    assert!(locate(src, &[p[0], p[1], Segment::Key("y")]).unwrap().is_none());
}

// ---- an anchor and an alias on the same node ----

/// `b` both defines anchor `y` and aliases `x`. One `kind` field cannot hold
/// both, so anchor-ness lives in its own field.
#[test]
fn a_node_can_define_an_anchor_and_alias_another() {
    let src = "a: &x 1\nb: &y *x\nc: *y\n";

    let a = at(src, &[Segment::Key("a")]);
    assert_eq!((a.kind, a.anchor), (NodeKind::Literal, Some("x")));

    let b = at(src, &[Segment::Key("b")]);
    assert_eq!(b.kind, NodeKind::AliasRef("x"), "alias nature lost");
    assert_eq!(b.anchor, Some("y"), "anchor nature lost");
    assert_eq!(&src[b.span], "*x");

    let c = at(src, &[Segment::Key("c")]);
    assert_eq!((c.kind, c.anchor), (NodeKind::AliasRef("y"), None));
}

#[test]
fn an_unanchored_alias_has_no_anchor() {
    let src = "a: &x 1\nb: *x\n";
    let b = at(src, &[Segment::Key("b")]);
    assert_eq!((b.kind, b.anchor), (NodeKind::AliasRef("x"), None));
}
