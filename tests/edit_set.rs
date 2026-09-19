#![cfg(feature = "edit")]

use yaml0::{SetOpts, SetOutcome, Value, locate, path, set, set_with};

fn reparse(src: &str) -> Value {
    yaml0::from_str(src).expect("output should reparse")
}

fn at<'a>(v: &'a Value, key: &str) -> &'a Value {
    match v {
        Value::Map(pairs) => pairs
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("no key {key:?}")),
        other => panic!("not a map: {other:?}"),
    }
}

// ---- quoting preservation ----

#[test]
fn plain_stays_plain() {
    let (out, outcome) = set("image: nginx:1.0\n", path!["image"], "nginx:2.0").unwrap();
    assert_eq!(out, "image: nginx:2.0\n");
    assert!(matches!(outcome, SetOutcome::Spliced));
}

#[test]
fn plain_upgrades_to_quoted_when_the_value_needs_it() {
    let (out, _) = set("note: hello\n", path!["note"], "a: b").unwrap();
    assert_ne!(out, "note: a: b\n", "an unquoted colon would reparse wrong");
    assert_eq!(at(&reparse(&out), "note").as_str(), Some("a: b"));
}

/// Dropping the quotes would turn a string into a float. This is the reason
/// the source's quoting style is preserved rather than recomputed.
#[test]
fn double_quoted_keeps_its_quotes_and_therefore_its_type() {
    let (out, _) = set("version: \"3.8\"\n", path!["version"], "3.9").unwrap();
    assert_eq!(out, "version: \"3.9\"\n");
    assert!(matches!(at(&reparse(&out), "version"), Value::String(_)));

    // the hazard being avoided:
    assert!(matches!(at(&reparse("version: 3.9\n"), "version"), Value::Float(_)));
}

#[test]
fn single_quoted_stays_single_quoted() {
    let (out, _) = set("name: 'web'\n", path!["name"], "api").unwrap();
    assert_eq!(out, "name: 'api'\n");
}

#[test]
fn single_quoted_doubles_an_embedded_quote() {
    let (out, _) = set("name: 'web'\n", path!["name"], "it's").unwrap();
    assert_eq!(out, "name: 'it''s'\n");
    assert_eq!(at(&reparse(&out), "name").as_str(), Some("it's"));
}

#[test]
fn single_quoted_falls_back_to_double_when_it_cannot_hold_the_value() {
    let (out, _) = set("name: 'web'\n", path!["name"], "a\nb").unwrap();
    assert!(out.contains('"'), "got {out:?}");
    assert_eq!(at(&reparse(&out), "name").as_str(), Some("a\nb"));
}

#[test]
fn double_quoted_escapes_an_embedded_quote() {
    let (out, _) = set("name: \"web\"\n", path!["name"], "say \"hi\"").unwrap();
    assert_eq!(at(&reparse(&out), "name").as_str(), Some("say \"hi\""));
}

/// Block style is not preserved; the value survives, the presentation doesn't.
#[test]
fn block_scalar_becomes_an_inline_scalar() {
    let src = "script: |\n  one\n  two\nnext: 1\n";
    let (out, _) = set(src, path!["script"], "echo hi\n").unwrap();
    let v = reparse(&out);
    assert_eq!(at(&v, "script").as_str(), Some("echo hi\n"));
    assert_eq!(at(&v, "next").as_i64(), Some(1));
}

// ---- minimal diff ----

const COMPOSE: &str = "\
# Production stack
version: \"3.8\"

x-restart: &restart-policy
  restart: unless-stopped

services:
  web:
    <<: *restart-policy
    image: ghcr.io/acme/web:1.4.2   # bump on release
    ports:
      - \"8080:80\"
  db:
    image: postgres:16.1
";

#[test]
fn only_the_addressed_bytes_change() {
    let (out, _) = set(
        COMPOSE,
        path!["services", "web", "image"],
        "ghcr.io/acme/web:1.5.0",
    )
    .unwrap();

    let changed: Vec<_> = COMPOSE
        .lines()
        .zip(out.lines())
        .filter(|(a, b)| a != b)
        .collect();

    assert_eq!(changed.len(), 1, "changed: {changed:?}");
    assert!(changed[0].1.contains("# bump on release"), "comment must survive");
    assert_eq!(COMPOSE.lines().count(), out.lines().count());
    reparse(&out);
}

#[test]
fn sequential_sets_compose() {
    let (out, _) = set(COMPOSE, path!["services", "db", "image"], "postgres:16.2").unwrap();
    let (out, _) = set(&out, path!["version"], "3.9").unwrap();
    assert!(out.contains("postgres:16.2"));
    assert!(out.contains("version: \"3.9\""));
    reparse(&out);
}

#[test]
fn the_new_value_is_locatable_afterwards() {
    let (out, _) = set(COMPOSE, path!["services", "db", "image"], "postgres:16.2").unwrap();
    let found = locate(&out, path!["services", "db", "image"]).unwrap().unwrap();
    assert_eq!(&out[found.span], "postgres:16.2");
}

// ---- outcomes ----

#[test]
fn anchor_definition_reports_its_refs() {
    let src = "base: &b 1\nx: *b\ny: *b\n";
    let (out, outcome) = set_with(src, path!["base"], "2", SetOpts::new().edit_anchors()).unwrap();
    assert_eq!(out, "base: &b 2\nx: *b\ny: *b\n");

    let SetOutcome::PropagatedAnchor { name, refs } = outcome else {
        panic!("expected PropagatedAnchor");
    };
    assert_eq!((name.as_ref(), refs), ("b", 2));
}

#[test]
fn an_unreferenced_anchor_reports_zero_refs() {
    let (_, outcome) =
        set_with("base: &b 1\n", path!["base"], "2", SetOpts::new().edit_anchors()).unwrap();
    let SetOutcome::PropagatedAnchor { refs, .. } = outcome else {
        panic!("expected PropagatedAnchor");
    };
    assert_eq!(refs, 0);
}

#[test]
fn a_redefined_anchor_counts_only_its_own_aliases() {
    // *b between the two definitions belongs to the first.
    let src = "one: &b 1\nuse: *b\ntwo: &b 2\nalso: *b\n";
    let (_, outcome) = set_with(src, path!["one"], "9", SetOpts::new().edit_anchors()).unwrap();
    let SetOutcome::PropagatedAnchor { refs, .. } = outcome else {
        panic!("expected PropagatedAnchor");
    };
    assert_eq!(refs, 1);
}

#[test]
fn a_merge_site_counts_as_a_ref() {
    let src = "base: &b 1\napp:\n  <<: *b\n";
    let (_, outcome) = set_with(src, path!["base"], "2", SetOpts::new().edit_anchors()).unwrap();
    let SetOutcome::PropagatedAnchor { refs, .. } = outcome else {
        panic!("expected PropagatedAnchor");
    };
    assert_eq!(refs, 1);
}

// ---- refusals ----

#[test]
fn refuses_an_alias() {
    let src = "base: &b 1\nuse: *b\n";
    assert!(set(src, path!["use"], "2").is_err());
}

#[test]
fn refuses_a_merge_inherited_key() {
    let src = "base: &b\n  image: nginx\napp:\n  <<: *b\n";
    assert!(set(src, path!["app", "image"], "other").is_err());
}

#[test]
fn refuses_an_empty_value() {
    let src = "volumes:\n  pgdata:\nnext: 1\n";
    assert!(
        set(src, path!["volumes", "pgdata"], "x").is_err(),
        "a zero-width span would splice at the next line"
    );
}

#[test]
fn refuses_a_missing_path() {
    assert!(set("a: 1\n", path!["b"], "2").is_err());
    assert!(set("a:\n  - x\n", path!["a", 5], "2").is_err());
}

// ---- scope: what a "scalar splice" may touch ----

#[test]
fn refuses_to_replace_a_collection() {
    let src = "services:\n  web:\n    image: nginx\n  db:\n    image: postgres\n";
    assert!(
        set(src, path!["services"], "gone").is_err(),
        "replacing a whole subtree is not a scalar splice"
    );
}

#[test]
fn editing_inside_an_anchored_collection_is_refused_by_default() {
    let src = "base: &b\n  image: nginx\napp:\n  <<: *b\n";
    assert!(set(src, path!["base", "image"], "other").is_err());
}

#[test]
fn editing_inside_an_anchored_collection_reports_propagation() {
    let src = "base: &b\n  image: nginx\napp:\n  <<: *b\n";
    let (out, outcome) =
        set_with(src, path!["base", "image"], "other", SetOpts::new().edit_anchors()).unwrap();

    // The edit reaches `app` through the merge, and the outcome says so.
    assert_eq!(at(at(&reparse(&out), "app"), "image").as_str(), Some("other"));
    let SetOutcome::PropagatedAnchor { name, refs } = outcome else {
        panic!("expected PropagatedAnchor");
    };
    assert_eq!((name.as_ref(), refs), ("b", 1));
}

// ---- replace_nodes ----

#[test]
fn replacing_a_collection_needs_the_flag() {
    let src = "services:\n  web:\n    image: nginx\n";
    let (out, outcome) =
        set_with(src, path!["services"], "gone", SetOpts::new().replace_nodes()).unwrap();
    // The map's span starts at its first key, on the next line, so the scalar
    // lands there. Still valid YAML, and still the same value.
    assert_eq!(at(&reparse(&out), "services").as_str(), Some("gone"));
    assert!(matches!(outcome, SetOutcome::ReplacedNode));
}

#[test]
fn an_anchored_collection_needs_both_flags() {
    let src = "base: &b\n  k: v\nuse: *b\n";
    let anchors = SetOpts::new().edit_anchors();
    assert!(set_with(src, path!["base"], "gone", anchors.clone()).is_err());

    let (_, outcome) =
        set_with(src, path!["base"], "gone", anchors.replace_nodes()).unwrap();
    assert!(matches!(outcome, SetOutcome::PropagatedAnchor { .. }));
}

// ---- edit_refs ----

#[test]
fn an_alias_needs_the_flag() {
    let src = "base: &b 1\nuse: *b\n";
    assert!(set(src, path!["use"], "2").is_err());

    let (out, outcome) = set_with(src, path!["use"], "2", SetOpts::new().edit_refs()).unwrap();
    assert_eq!(out, "base: &b 1\nuse: 2\n");
    assert!(matches!(outcome, SetOutcome::ReplacedAlias { ref name } if name == "b"));
}

/// The alias token is not the model: `*b` would classify as a string and
/// quote the replacement, silently retyping it.
#[test]
fn replacing_an_alias_takes_its_type_from_the_anchor() {
    let src = "base: &b 1\nuse: *b\n";
    let (out, _) = set_with(src, path!["use"], "2", SetOpts::new().edit_refs()).unwrap();
    assert!(matches!(at(&reparse(&out), "use"), Value::Int(2)));
}

#[test]
fn an_alias_to_a_collection_needs_replace_nodes_too() {
    let src = "base: &b\n  k: v\nuse: *b\n";
    let refs = SetOpts::new().edit_refs();
    assert!(set_with(src, path!["use"], "x", refs.clone()).is_err());
    assert!(set_with(src, path!["use"], "x", refs.replace_nodes()).is_ok());
}

/// Expanding an alias to edit inside it is a separate feature.
#[test]
fn a_path_through_an_alias_is_refused_even_with_edit_refs() {
    let src = "base: &b\n  k: v\nuse: *b\n";
    let opts = SetOpts::new().edit_refs().replace_nodes().edit_anchors();
    assert!(set_with(src, path!["use", "k"], "x", opts).is_err());
}

// ---- insert_empty ----

#[test]
fn filling_an_empty_value_needs_the_flag() {
    let src = "volumes:\n  pgdata:\nnext: 1\n";
    assert!(set(src, path!["volumes", "pgdata"], "x").is_err());

    let (out, outcome) = set_with(
        src,
        path!["volumes", "pgdata"],
        "x",
        SetOpts::new().insert_empty(),
    )
    .unwrap();
    assert_eq!(out, "volumes:\n  pgdata: x\nnext: 1\n");
    assert!(matches!(outcome, SetOutcome::Inserted));
}

#[test]
fn filling_an_empty_value_at_eof() {
    let (out, _) = set_with("a:\n  b:\n", path!["a", "b"], "x", SetOpts::new().insert_empty())
        .unwrap();
    assert_eq!(out, "a:\n  b: x\n");
}

#[test]
fn filling_does_not_double_the_space() {
    let (out, _) = set_with("a: \nz: 1\n", path!["a"], "x", SetOpts::new().insert_empty()).unwrap();
    assert_eq!(out, "a: x\nz: 1\n");
}

#[test]
fn only_a_mapping_value_can_be_filled() {
    // An empty sequence item has no key to write after.
    assert!(set_with("- \n", path![0], "x", SetOpts::new().insert_empty()).is_err());
}

/// Precedence: the widest effect is what gets reported.
#[test]
fn an_insert_inside_an_anchor_reports_propagation() {
    let src = "base: &b\n  k:\nuse: *b\n";
    let opts = SetOpts::new().insert_empty().edit_anchors();
    let (out, outcome) = set_with(src, path!["base", "k"], "v", opts).unwrap();
    assert_eq!(out, "base: &b\n  k: v\nuse: *b\n");
    assert!(matches!(outcome, SetOutcome::PropagatedAnchor { ref name, refs: 1 } if name == "b"));
}

// ---- allow_type_change ----

#[test]
fn a_plain_string_field_keeps_its_type_by_default() {
    let (out, _) = set("note: hello\n", path!["note"], "42").unwrap();
    assert!(matches!(at(&reparse(&out), "note"), Value::String(_)));

    let (out, _) = set_with(
        "note: hello\n",
        path!["note"],
        "42",
        SetOpts::new().allow_type_change(),
    )
    .unwrap();
    assert_eq!(out, "note: 42\n");
    assert!(matches!(at(&reparse(&out), "note"), Value::Int(42)));
}

#[test]
fn a_plain_int_field_stays_plain_and_stays_an_int() {
    let (out, _) = set("count: 1\n", path!["count"], "2").unwrap();
    assert_eq!(out, "count: 2\n");
    assert!(matches!(at(&reparse(&out), "count"), Value::Int(2)));
}

#[test]
fn a_value_needing_quotes_is_quoted_regardless_of_the_flag() {
    let opts = SetOpts::new().allow_type_change();
    let (out, _) = set_with("note: hello\n", path!["note"], "a: b", opts).unwrap();
    assert_eq!(at(&reparse(&out), "note").as_str(), Some("a: b"));
}

// ---- follow_refs ----

#[test]
fn following_a_ref_edits_the_anchor_and_leaves_the_token() {
    let src = "base: &b 1\nuse: *b\nother: *b\n";
    let (out, outcome) = set_with(src, path!["use"], "2", SetOpts::new().follow_refs()).unwrap();

    assert_eq!(out, "base: &b 2\nuse: *b\nother: *b\n");
    let SetOutcome::PropagatedAnchor { name, refs } = outcome else {
        panic!("expected PropagatedAnchor");
    };
    assert_eq!((name.as_ref(), refs), ("b", 2));
}

#[test]
fn following_reaches_through_a_truncated_path() {
    let src = "base: &b\n  k: v\nuse: *b\n";
    let (out, outcome) =
        set_with(src, path!["use", "k"], "x", SetOpts::new().follow_refs()).unwrap();

    assert_eq!(out, "base: &b\n  k: x\nuse: *b\n");
    assert!(matches!(outcome, SetOutcome::PropagatedAnchor { ref name, refs: 1 } if name == "b"));
}

/// Reaching back edits the anchor, so it implies `edit_anchors` rather than
/// requiring the caller to set both.
#[test]
fn follow_refs_is_sufficient_on_its_own() {
    let src = "base: &b 1\nuse: *b\n";
    assert!(set_with(src, path!["use"], "2", SetOpts::new().follow_refs()).is_ok());
}

#[test]
fn following_to_a_collection_still_needs_replace_nodes() {
    let src = "base: &b\n  k: v\nuse: *b\n";
    let follow = SetOpts::new().follow_refs();
    assert!(set_with(src, path!["use"], "x", follow.clone()).is_err());
    assert!(set_with(src, path!["use"], "x", follow.replace_nodes()).is_ok());
}

/// The model is the anchor's value, so an integer stays an integer.
#[test]
fn following_takes_its_type_from_the_anchor() {
    let src = "base: &b 1\nuse: *b\n";
    let (out, _) = set_with(src, path!["use"], "2", SetOpts::new().follow_refs()).unwrap();
    assert!(matches!(at(&reparse(&out), "base"), Value::Int(2)));
}

/// Without the flag the alias site is what gets edited, not the anchor.
#[test]
fn not_following_edits_the_site_instead() {
    let src = "base: &b 1\nuse: *b\n";
    let (out, outcome) = set_with(src, path!["use"], "2", SetOpts::new().edit_refs()).unwrap();
    assert_eq!(out, "base: &b 1\nuse: 2\n");
    assert!(matches!(outcome, SetOutcome::ReplacedAlias { ref name } if name == "b"));
}

// ---- a node that is both an anchor and an alias ----

const CHAIN: &str = "a: &x 1\nb: &y *x\nc: *y\n";

/// `b` carries `&y` as well as `*x`, so editing it reaches every `*y`.
/// Before anchor-ness was split out of `kind`, this reported ReplacedAlias.
#[test]
fn an_anchored_alias_is_an_anchor_edit() {
    assert!(
        set_with(CHAIN, path!["b"], "9", SetOpts::new().edit_refs()).is_err(),
        "edit_refs alone should not permit an edit that propagates"
    );

    let opts = SetOpts::new().edit_refs().edit_anchors();
    let (out, outcome) = set_with(CHAIN, path!["b"], "9", opts).unwrap();
    assert_eq!(out, "a: &x 1\nb: &y 9\nc: *y\n");
    assert!(
        matches!(outcome, SetOutcome::PropagatedAnchor { ref name, refs: 1 } if name == "y"),
        "got {outcome:?}"
    );
}

/// The model is the anchor `*x` points at, not the text `*x`, which would
/// classify as a string and quote the replacement.
#[test]
fn an_anchored_alias_takes_its_type_from_the_anchor() {
    let opts = SetOpts::new().edit_refs().edit_anchors();
    let (out, _) = set_with(CHAIN, path!["b"], "9", opts).unwrap();
    assert!(matches!(at(&reparse(&out), "b"), Value::Int(9)), "9 was quoted");
}

/// `c` -> `y` -> `x`, two hops. Reachable only once an anchored alias keeps
/// its AliasRef kind.
#[test]
fn following_walks_an_alias_chain() {
    let (out, outcome) = set_with(CHAIN, path!["c"], "9", SetOpts::new().follow_refs()).unwrap();

    assert_eq!(out, "a: &x 9\nb: &y *x\nc: *y\n", "should land on a, the chain's end");
    assert!(matches!(outcome, SetOutcome::PropagatedAnchor { ref name, .. } if name == "x"), "got {outcome:?}");

    // Both links survive, and every path still reads the new value.
    let v = reparse(&out);
    for key in ["a", "b", "c"] {
        assert!(matches!(at(&v, key), Value::Int(9)), "{key} did not follow");
    }
}

#[test]
fn an_unanchored_alias_is_still_just_an_alias() {
    let src = "a: &x 1\nb: *x\n";
    let (out, outcome) = set_with(src, path!["b"], "2", SetOpts::new().edit_refs()).unwrap();
    assert_eq!(out, "a: &x 1\nb: 2\n");
    assert!(matches!(outcome, SetOutcome::ReplacedAlias { ref name } if name == "x"));
}

// ---- flow containers ----

#[test]
fn editing_a_flow_seq_item_leaves_the_rest_of_the_line() {
    let src = "ports: [80, 443, 8080]   # exposed\n";
    let (out, outcome) = set(src, path!["ports", 1], "8443").unwrap();
    assert_eq!(out, "ports: [80, 8443, 8080]   # exposed\n");
    assert!(matches!(outcome, SetOutcome::Spliced));
}

#[test]
fn editing_a_flow_map_value() {
    let src = "opts: {a: 1, b: 2}\n";
    let (out, _) = set(src, path!["opts", "b"], "3").unwrap();
    assert_eq!(out, "opts: {a: 1, b: 3}\n");
}

#[test]
fn a_flow_item_keeps_its_quoting() {
    let src = "ports: [\"80:80\", 443]\n";
    let (out, _) = set(src, path!["ports", 0], "80:8080").unwrap();
    assert_eq!(out, "ports: [\"80:8080\", 443]\n");
}

/// Flow containers are collections now, not opaque leaves, so replacing one
/// wholesale is a structural edit like any other.
#[test]
fn replacing_a_whole_flow_container_needs_replace_nodes() {
    let src = "ports: [80, 443]\nnext: 1\n";
    assert!(set(src, path!["ports"], "none").is_err());

    let (out, outcome) =
        set_with(src, path!["ports"], "none", SetOpts::new().replace_nodes()).unwrap();
    assert_eq!(out, "ports: none\nnext: 1\n", "the closing bracket must go too");
    assert!(matches!(outcome, SetOutcome::ReplacedNode));
}

#[test]
fn editing_through_an_alias_inside_a_flow_seq() {
    let src = "base: &b 1\nrefs: [*b, 2]\n";
    assert!(set(src, path!["refs", 0], "9").is_err(), "alias needs edit_refs");

    let (out, outcome) = set_with(src, path!["refs", 0], "9", SetOpts::new().edit_refs()).unwrap();
    assert_eq!(out, "base: &b 1\nrefs: [9, 2]\n");
    assert!(matches!(outcome, SetOutcome::ReplacedAlias { ref name } if name == "b"));
}

// ---- override_inherited ----

const MERGED: &str = "\
base: &b
  image: nginx
  version: \"3.8\"
app:
  <<: *b
  port: 80
other:
  <<: *b
";

#[test]
fn overriding_an_inherited_key_needs_the_flag() {
    let err = set(MERGED, path!["app", "image"], "other").unwrap_err();
    assert!(
        err.msg.contains("override_inherited"),
        "the refusal should name the way out: {}",
        err.msg
    );
}

#[test]
fn overriding_writes_an_explicit_key_after_the_merge() {
    let (out, outcome) = set_with(
        MERGED,
        path!["app", "image"],
        "other",
        SetOpts::new().override_inherited(),
    )
    .unwrap();

    assert!(out.contains("app:\n  <<: *b\n  image: other\n  port: 80\n"), "got:\n{out}");
    assert!(matches!(outcome, SetOutcome::Inserted));

    let v = reparse(&out);
    assert_eq!(at(at(&v, "app"), "image").as_str(), Some("other"));
    // the merge source and the other consumer keep the old value
    assert_eq!(at(at(&v, "base"), "image").as_str(), Some("nginx"));
    assert_eq!(at(at(&v, "other"), "image").as_str(), Some("nginx"));
}

#[test]
fn overriding_leaves_every_other_byte_alone() {
    let (out, _) = set_with(
        MERGED,
        path!["app", "image"],
        "other",
        SetOpts::new().override_inherited(),
    )
    .unwrap();

    assert_eq!(out.lines().count(), MERGED.lines().count() + 1);
    let added: Vec<_> = out.lines().filter(|l| !MERGED.lines().any(|m| m == *l)).collect();
    assert_eq!(added, vec!["  image: other"], "indent or content wrong");
}

/// The inherited value is the model, so an override keeps its type.
#[test]
fn overriding_preserves_the_inherited_type() {
    let (out, _) = set_with(
        MERGED,
        path!["app", "version"],
        "3.9",
        SetOpts::new().override_inherited(),
    )
    .unwrap();

    assert!(out.contains("  version: \"3.9\""), "quotes lost:\n{out}");
    assert!(matches!(at(at(&reparse(&out), "app"), "version"), Value::String(_)));
}

#[test]
fn an_overriding_key_is_quoted_when_it_has_to_be() {
    let src = "base: &b\n  \"a: b\": 1\napp:\n  <<: *b\n";
    let (out, _) = set_with(src, path!["app", "a: b"], "2", SetOpts::new().override_inherited())
        .unwrap();
    assert_eq!(at(at(&reparse(&out), "app"), "a: b").as_i64(), Some(2));
}

#[test]
fn overriding_a_transitively_inherited_key() {
    let src = "root: &r\n  deep: 1\nmid: &m\n  <<: *r\napp:\n  <<: *m\n";
    let (out, _) = set_with(src, path!["app", "deep"], "2", SetOpts::new().override_inherited())
        .unwrap();

    let v = reparse(&out);
    assert_eq!(at(at(&v, "app"), "deep").as_i64(), Some(2));
    assert_eq!(at(at(&v, "root"), "deep").as_i64(), Some(1), "source changed");
}

#[test]
fn overriding_inside_an_anchored_map_reports_propagation() {
    let src = "base: &b\n  k: v\nwrap: &w\n  <<: *b\nuse: *w\n";
    let opts = SetOpts::new().override_inherited();
    assert!(set_with(src, path!["wrap", "k"], "x", opts.clone()).is_err());

    let (_, outcome) = set_with(src, path!["wrap", "k"], "x", opts.edit_anchors()).unwrap();
    assert!(matches!(outcome, SetOutcome::PropagatedAnchor { ref name, .. } if name == "w"), "{outcome:?}");
}

#[test]
fn a_flow_mapping_has_no_line_to_append_to() {
    let src = "base: &b\n  k: v\napp: {<<: *b}\n";
    let err = set_with(src, path!["app", "k"], "x", SetOpts::new().override_inherited())
        .unwrap_err();
    assert!(err.msg.contains("flow"), "{}", err.msg);
}

#[test]
fn only_a_named_key_can_override() {
    // The path enters inherited territory at `image`, leaving `0` dangling.
    let err = set_with(
        MERGED,
        path!["app", "image", 0],
        "x",
        SetOpts::new().override_inherited(),
    )
    .unwrap_err();
    assert!(err.msg.contains("key name"), "{}", err.msg);
}
