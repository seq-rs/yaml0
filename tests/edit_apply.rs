#![cfg(feature = "edit")]

use serde::{Deserialize, Serialize};
use yaml0::{Change, Document, SetOpts, path};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Service {
    image: Option<String>,
    replicas: Option<u32>,
    ports: Option<Vec<String>>,
    command: Option<String>,
}

const SRC: &str = "\
# stack
image: nginx:1.25   # bump me
replicas: 2
ports:
  - \"80:80\"
  - \"443:443\"
";

// ---- remove ----

#[test]
fn remove_takes_key_and_value() {
    let mut doc = Document::new(SRC);
    doc.remove(path!["replicas"]).unwrap();
    assert_eq!(doc.as_str(), "# stack\nimage: nginx:1.25   # bump me\nports:\n  - \"80:80\"\n  - \"443:443\"\n");
}

#[test]
fn remove_takes_a_multi_line_value_with_its_key() {
    let mut doc = Document::new(SRC);
    doc.remove(path!["ports"]).unwrap();
    assert_eq!(doc.as_str(), "# stack\nimage: nginx:1.25   # bump me\nreplicas: 2\n");
}

/// A comment on the key's own line goes; one above it stays.
#[test]
fn remove_and_comments() {
    let mut doc = Document::new(SRC);
    doc.remove(path!["image"]).unwrap();
    assert!(doc.as_str().starts_with("# stack\nreplicas: 2\n"), "got {:?}", doc.as_str());
    assert!(!doc.as_str().contains("bump me"));
}

#[test]
fn remove_the_last_entry_leaves_no_trailing_blank() {
    let mut doc = Document::new("a: 1\nb: 2\n");
    doc.remove(path!["b"]).unwrap();
    assert_eq!(doc.as_str(), "a: 1\n");
}

#[test]
fn remove_refuses_what_it_cannot_do_safely() {
    let cases: &[(&str, Vec<yaml0::Segment<'_>>)] = &[
        // inherited: not physically present
        ("base: &b\n  k: v\napp:\n  <<: *b\n", path!["app", "k"].to_vec()),
        // anchored: removing it breaks the aliases
        ("base: &b 1\nuse: *b\n", path!["base"].to_vec()),
        // inside an anchor: same reason
        ("base: &b\n  k: v\nuse: *b\n", path!["base", "k"].to_vec()),
        // a sequence item has no key to remove
        ("list:\n  - a\n  - b\n", path!["list", 0].to_vec()),
        // a flow entry has no line of its own
        ("opts: {a: 1, b: 2}\n", path!["opts", "a"].to_vec()),
        // not there at all
        ("a: 1\n", path!["nope"].to_vec()),
    ];

    for (src, p) in cases {
        let mut doc = Document::new(*src);
        assert!(doc.remove(p).is_err(), "should refuse {src:?} at {p:?}");
        assert_eq!(doc.as_str(), *src, "buffer changed despite refusal");
    }
}

// ---- apply ----

fn parse(src: &str) -> Service {
    yaml0::from_str(src).unwrap()
}

#[test]
fn apply_changes_only_what_differs() {
    let mut doc = Document::new(SRC);
    let before = parse(SRC);
    let mut after = before.clone();
    after.image = Some("nginx:1.26".into());

    let applied = doc.apply(&before, &after).unwrap();

    assert_eq!(applied.changes.len(), 1);
    assert!(matches!(&applied.changes[0], Change::Set { path, .. } if path == "image"));
    assert_eq!(doc.as_str(), SRC.replace("1.25", "1.26"));
}

#[test]
fn apply_with_no_mutation_changes_nothing() {
    let mut doc = Document::new(SRC);
    let before = parse(SRC);
    let applied = doc.apply(&before, &before.clone()).unwrap();

    assert!(applied.changes.is_empty());
    assert_eq!(doc.as_str(), SRC);
}

#[test]
fn clearing_a_field_removes_its_key() {
    let mut doc = Document::new(SRC);
    let before = parse(SRC);
    let mut after = before.clone();
    after.ports = None;

    let applied = doc.apply(&before, &after).unwrap();

    assert!(matches!(&applied.changes[0], Change::Removed { path } if path == "ports"));
    assert_eq!(doc.as_str(), "# stack\nimage: nginx:1.25   # bump me\nreplicas: 2\n");
}

/// A field that was never in the source has nowhere to be written.
#[test]
fn giving_a_value_to_an_absent_field_is_refused() {
    let mut doc = Document::new(SRC);
    let before = parse(SRC);
    let mut after = before.clone();
    after.command = Some("nginx -g".into());

    let err = doc.apply(&before, &after).unwrap_err();
    assert!(err.msg.contains("absent field"), "{}", err.msg);
    assert_eq!(doc.as_str(), SRC, "refusal must not write");
}

#[test]
fn a_sequence_length_change_is_refused() {
    let mut doc = Document::new(SRC);
    let before = parse(SRC);
    let mut after = before.clone();
    after.ports.as_mut().unwrap().pop();

    let err = doc.apply(&before, &after).unwrap_err();
    assert!(err.msg.contains("sequence length 2 -> 1"), "{}", err.msg);
    assert_eq!(doc.as_str(), SRC);
}

/// Every problem is collected before any edit runs, so one bad change cannot
/// leave the document half written.
#[test]
fn a_refusal_discards_the_whole_apply() {
    let mut doc = Document::new(SRC);
    let before = parse(SRC);
    let mut after = before.clone();
    after.image = Some("nginx:1.26".into()); // valid on its own
    after.ports.as_mut().unwrap().push("9090:9090".into()); // but this is not

    assert!(doc.apply(&before, &after).is_err());
    assert_eq!(doc.as_str(), SRC, "the valid edit must not have landed either");
}

#[test]
fn apply_edits_several_fields_in_one_pass() {
    let mut doc = Document::new(SRC);
    let before = parse(SRC);
    let mut after = before.clone();
    after.image = Some("nginx:1.26".into());
    after.replicas = Some(5);
    after.ports.as_mut().unwrap()[1] = "8443:443".into();

    let applied = doc.apply(&before, &after).unwrap();

    assert_eq!(applied.changes.len(), 3);
    let out = doc.as_str();
    assert!(out.contains("image: nginx:1.26   # bump me"), "comment lost");
    assert!(out.contains("replicas: 5"));
    assert!(out.contains("- \"8443:443\""), "quoting lost");
    assert!(out.starts_with("# stack\n"));
}

#[test]
fn apply_respects_set_opts() {
    let src = "base: &b 1\nuse: *b\n";
    #[derive(Serialize, Deserialize, Clone)]
    struct Two {
        base: i64,
        r#use: i64,
    }

    let before: Two = yaml0::from_str(src).unwrap();
    let mut after = before.clone();
    after.base = 2;

    let mut doc = Document::new(src);
    assert!(doc.apply(&before, &after).is_err(), "anchors need the option");

    let mut doc = Document::new(src);
    doc.apply_with(&before, &after, SetOpts::new().edit_anchors()).unwrap();
    assert_eq!(doc.as_str(), "base: &b 2\nuse: *b\n");
}

// ---- Edit<T> ----

#[test]
fn edit_derefs_and_commits() {
    let mut doc = Document::new(SRC);

    let mut svc = doc.edit::<Service>().unwrap();
    assert_eq!(svc.replicas, Some(2)); // Deref
    svc.image = Some("nginx:1.26".into()); // DerefMut
    svc.replicas = Some(3);
    let applied = svc.commit().unwrap();

    assert_eq!(applied.changes.len(), 2);
    assert!(doc.as_str().contains("image: nginx:1.26   # bump me"));
    assert!(doc.as_str().contains("replicas: 3"));
}

#[test]
fn edit_distinguishes_cleared_from_never_present() {
    let mut doc = Document::new(SRC);

    let mut svc = doc.edit::<Service>().unwrap();
    svc.ports = None; // cleared: was in the source
    // `command` is None in both and must stay silent
    let applied = svc.commit().unwrap();

    assert_eq!(applied.changes.len(), 1, "{:?}", applied.changes);
    assert!(matches!(&applied.changes[0], Change::Removed { path } if path == "ports"));
}

#[test]
fn edit_can_be_committed_without_changes() {
    let mut doc = Document::new(SRC);
    let svc = doc.edit::<Service>().unwrap();
    assert!(svc.commit().unwrap().changes.is_empty());
    assert_eq!(doc.as_str(), SRC);
}
