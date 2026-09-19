#![cfg(feature = "edit")]

use yaml0::{Segment, locate, parse_path, path};

fn keys(path: &str) -> Vec<Segment<'_>> {
    parse_path(path).expect("path should parse")
}

// ---- parse_path: shapes ----

#[test]
fn dotted_keys() {
    assert_eq!(
        keys("services.web.image"),
        vec![
            Segment::Key("services"),
            Segment::Key("web"),
            Segment::Key("image")
        ]
    );
}

#[test]
fn bracketed_index() {
    assert_eq!(keys("ports[0]"), vec![Segment::Key("ports"), Segment::Index(0)]);
}

#[test]
fn indices_chain_without_a_separator() {
    assert_eq!(
        keys("a[0][1]"),
        vec![Segment::Key("a"), Segment::Index(0), Segment::Index(1)]
    );
}

#[test]
fn a_dot_before_an_index_is_allowed() {
    assert_eq!(keys("a.[0]"), keys("a[0]"));
}

#[test]
fn leading_index() {
    assert_eq!(keys("[0].name"), vec![Segment::Index(0), Segment::Key("name")]);
}

#[test]
fn empty_path_is_the_document_root() {
    assert_eq!(keys(""), vec![]);
}

// ---- parse_path: quoting ----

#[test]
fn double_quoted_key_holds_dots() {
    assert_eq!(
        keys(r#"data."application.yaml""#),
        vec![Segment::Key("data"), Segment::Key("application.yaml")]
    );
}

#[test]
fn single_quotes_carry_a_double_quote() {
    assert_eq!(keys(r#"'it"s'"#), vec![Segment::Key("it\"s")]);
}

#[test]
fn double_quotes_carry_a_single_quote() {
    assert_eq!(keys(r#""it's""#), vec![Segment::Key("it's")]);
}

#[test]
fn quoted_key_holds_brackets() {
    assert_eq!(keys(r#""a[0]""#), vec![Segment::Key("a[0]")]);
}

/// k8s annotation keys contain dots, so they only address correctly when
/// quoted. Unquoted they split, which is a wrong address, not an error.
#[test]
fn unquoted_dotted_key_splits() {
    assert_eq!(
        keys("annotations.kubernetes.io/ingress.class"),
        vec![
            Segment::Key("annotations"),
            Segment::Key("kubernetes"),
            Segment::Key("io/ingress"),
            Segment::Key("class"),
        ]
    );
    assert_eq!(
        keys(r#"annotations."kubernetes.io/ingress.class""#),
        vec![
            Segment::Key("annotations"),
            Segment::Key("kubernetes.io/ingress.class"),
        ]
    );
}

// ---- parse_path: documents ----

#[test]
fn leading_doc_selector() {
    assert_eq!(
        keys("#1.metadata.name"),
        vec![Segment::Doc(1), Segment::Key("metadata"), Segment::Key("name")]
    );
}

#[test]
fn doc_selector_alone() {
    assert_eq!(keys("#0"), vec![Segment::Doc(0)]);
}

/// `#` is only a document selector in leading position.
#[test]
fn hash_inside_a_path_is_a_key() {
    assert_eq!(keys("a.#1"), vec![Segment::Key("a"), Segment::Key("#1")]);
}

// ---- parse_path: rejections ----

#[test]
fn missing_separator_is_rejected() {
    for bad in ["a[0]b", r#""a""b""#, r#"a"b""#, "a]b"] {
        assert!(parse_path(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn empty_and_trailing_segments_are_rejected() {
    for bad in ["a.", "a..b", ".a", "#1.", "a.]"] {
        assert!(parse_path(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn malformed_indices_are_rejected() {
    for bad in ["a[", "a[]", "a[x]", "a[0", "a[-1]", "a[0)"] {
        assert!(parse_path(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn index_overflow_errors_rather_than_wrapping() {
    assert!(parse_path("a[99999999999999999999999]").is_err());
}

#[test]
fn malformed_doc_selectors_are_rejected() {
    for bad in ["#", "#x", "#1x"] {
        assert!(parse_path(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn unterminated_quote_is_rejected() {
    for bad in [r#"a."b"#, r#"'a"#, r#""a"#] {
        assert!(parse_path(bad).is_err(), "{bad:?} should be rejected");
    }
}

#[test]
fn errors_carry_an_offset_into_the_path_string() {
    let e = parse_path("a[0]b").unwrap_err();
    assert_eq!(e.col, Some(4));
    assert_eq!(e.line, None);
}

// ---- path! macro ----

#[test]
fn macro_builds_keys_and_indices() {
    assert_eq!(
        path!["services", "web", "ports", 0].as_slice(),
        &[
            Segment::Key("services"),
            Segment::Key("web"),
            Segment::Key("ports"),
            Segment::Index(0),
        ]
    );
}

#[test]
fn macro_binds_to_a_local() {
    let p = path!["a", 1];
    assert_eq!(p.as_slice(), &[Segment::Key("a"), Segment::Index(1)]);
}

#[test]
fn macro_takes_runtime_values() {
    let service = String::from("web");
    let index = 2usize;
    assert_eq!(
        path!["services", service.as_str(), index].as_slice(),
        &[
            Segment::Key("services"),
            Segment::Key("web"),
            Segment::Index(2)
        ]
    );
}

#[test]
fn macro_accepts_a_trailing_comma() {
    assert_eq!(path!["a", 0,].as_slice(), path!["a", 0].as_slice());
}

// ---- both forms reach the same node ----

const SRC: &str = "\
services:
  web:
    image: nginx:1.0
    ports:
      - \"8080:80\"
";

#[test]
fn macro_and_parse_path_agree() {
    let parsed = parse_path("services.web.ports[0]").unwrap();
    assert_eq!(parsed.as_slice(), path!["services", "web", "ports", 0].as_slice());

    let a = locate(SRC, &parsed).unwrap().unwrap();
    let b = locate(SRC, path!["services", "web", "ports", 0]).unwrap().unwrap();
    assert_eq!(a, b);
    assert_eq!(&SRC[a.span], "\"8080:80\"");
}

#[test]
fn locate_accepts_array_slice_and_vec() {
    let expected = locate(SRC, path!["services", "web", "image"]).unwrap();

    let owned: Vec<Segment<'_>> = parse_path("services.web.image").unwrap();
    assert_eq!(locate(SRC, &owned).unwrap(), expected);

    let slice: &[Segment<'_>] = &owned;
    assert_eq!(locate(SRC, slice).unwrap(), expected);

    assert_eq!(
        locate(SRC, &[Segment::Key("services"), Segment::Key("web"), Segment::Key("image")]).unwrap(),
        expected
    );
}

#[test]
fn empty_macro_path_locates_the_root() {
    let root = locate(SRC, path![]).unwrap().unwrap();
    assert_eq!(&SRC[root.span.start..root.span.start + 8], "services");
}

/// The macro never parses its arguments: any string is one whole key, which
/// is what makes it the escape hatch for keys `parse_path` would split.
#[test]
fn macro_does_not_split_keys() {
    let p = path!["annotations.kubernetes.io/ingress.class", 0, "aaa"];
    assert_eq!(
        p.as_slice(),
        &[
            Segment::Key("annotations.kubernetes.io/ingress.class"),
            Segment::Index(0),
            Segment::Key("aaa"),
        ]
    );
    assert_eq!(p.len(), 3);
}
