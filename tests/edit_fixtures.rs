//! Editing real files from the ecosystems this crate targets.
//!
//! The bar: change one scalar and leave every other byte alone, including
//! comments, blank lines, key order and quoting style.

#![cfg(feature = "edit")]

use std::fs;

use yaml0::{Parser, Segment, SetOutcome, locate, parse_path, path, set};

const FIXTURES: &str = "tests/fixtures";

fn read(name: &str) -> String {
    fs::read_to_string(format!("{FIXTURES}/{name}"))
        .unwrap_or_else(|e| panic!("failed to read fixture {name}: {e}"))
}

/// Change one scalar and assert the edit was surgical.
///
/// Line-based rather than span-based on purpose: comparing against the span
/// `set` used would only restate what `set` did, not check it picked the
/// right bytes.
fn edit_one(src: &str, path: &[Segment<'_>], new: &str) -> String {
    let before = locate(src, path)
        .expect("parse")
        .unwrap_or_else(|| panic!("no node at {path:?}"));
    let old = src[before.span].to_string();

    let (out, outcome) = set(src, path, new).expect("set");
    assert!(matches!(outcome, SetOutcome::Spliced), "{outcome:?}");

    let changed: Vec<_> = src
        .lines()
        .zip(out.lines())
        .filter(|(a, b)| a != b)
        .collect();
    assert_eq!(changed.len(), 1, "expected one changed line, got {changed:?}");
    assert_eq!(src.lines().count(), out.lines().count());

    let (was, now) = changed[0];
    assert!(now.contains(new), "new value missing from {now:?}");

    // Remove the value from each line; what is left is indentation, the key,
    // and any trailing comment. Those must be byte-identical.
    let new_text = out[locate(&out, path).unwrap().unwrap().span].to_string();
    assert_eq!(
        was.replacen(&old, "", 1),
        now.replacen(&new_text, "", 1),
        "the line changed by more than its value"
    );

    Parser::new(&out).parse_all().expect("result reparses");
    out
}

// ---- docker-compose ----

#[test]
fn compose_image_bump() {
    let src = read("compose.yaml");
    let out = edit_one(&src, &path!["services", "web", "image"], "nginx:1.26");
    assert!(out.contains("image: nginx:1.26"));
    // the anchor block and both merge keys are untouched
    assert!(out.contains("x-defaults: &defaults"));
    assert_eq!(out.matches("<<: *defaults").count(), 2);
}

#[test]
fn compose_quoted_port_keeps_its_quotes() {
    let src = read("compose.yaml");
    let out = edit_one(&src, &path!["services", "web", "ports", 0], "80:8080");
    assert!(out.contains(r#"- "80:8080""#), "quoting style lost");
}

/// `api` overrides `restart` explicitly, so it is editable; `web` inherits it
/// through `<<` and is refused.
#[test]
fn compose_merge_overridden_vs_inherited() {
    let src = read("compose.yaml");
    edit_one(&src, &path!["services", "api", "restart"], "unless-stopped");

    let inherited = yaml0::set(&src, path!["services", "web", "restart"], "never");
    assert!(inherited.is_err(), "inherited key should be refused");
}

// ---- kubernetes ----

#[test]
fn k8s_container_image_bump() {
    let src = read("k8s_pod.yaml");
    edit_one(
        &src,
        &path!["spec", "containers", 0, "image"],
        "nginx:1.26-alpine",
    );
}

#[test]
fn k8s_label_edit_through_a_parsed_path() {
    let src = read("k8s_pod.yaml");
    let p = parse_path("metadata.labels.tier").unwrap();
    let out = edit_one(&src, &p, "backend");
    assert!(out.contains("tier: backend"));
    assert!(out.contains("app: nginx"), "sibling label moved");
}

// ---- multi-document streams ----

#[test]
fn kubectl_stream_second_document() {
    let src = read("kubectl_stream.yaml");
    let out = edit_one(&src, &path![Segment::Doc(1), "metadata", "name"], "web-svc-v2");
    assert!(out.contains("name: web-svc-v2"));
    // document 0 still has its own name
    assert!(out.contains("name: web\n"));
    assert_eq!(Parser::new(&out).parse_all().unwrap().len(), 2);
}

// ---- sops ----

/// An ARN is full of colons and must stay plain: only `: ` is ambiguous.
#[test]
fn sops_arn_stays_plain() {
    let src = read("sops_secret.yaml");
    let out = edit_one(
        &src,
        &path!["sops", "kms", 0, "arn"],
        "arn:aws:kms:eu-west-1:444455556666:key/other",
    );
    assert!(
        out.contains("arn: arn:aws:kms:eu-west-1:444455556666:key/other"),
        "ARN was quoted unnecessarily"
    );
}

// ---- commented config ----

#[test]
fn lazygit_edit_preserves_every_comment() {
    let src = read("lazygit_config.yml");
    let out = edit_one(&src, &path!["git", "paging", "pager"], "less -R");

    for comment in [
        "# lazygit configuration",
        "# https://github.com/jesseduffield/lazygit",
        "# colours used for the border of the focused panel",
        "# requires a nerd font",
        "# avoid network on open",
    ] {
        assert!(out.contains(comment), "lost {comment:?}");
    }
}

#[test]
fn lazygit_inline_comment_survives_on_the_edited_line() {
    let src = read("lazygit_config.yml");
    let out = edit_one(&src, &path!["gui", "showIcons"], "false");
    assert!(
        out.contains("showIcons: false          # requires a nerd font"),
        "inline comment or its spacing moved"
    );
}

#[test]
fn lazygit_empty_quoted_string_keeps_its_quotes() {
    let src = read("lazygit_config.yml");
    let out = edit_one(&src, &path!["os", "editCommand"], "nvim +Man!");
    assert!(out.contains("editCommand: 'nvim +Man!'"), "quoting style lost");
}

#[test]
fn lazygit_seq_item() {
    let src = read("lazygit_config.yml");
    let out = edit_one(&src, &path!["gui", "activeBorderColor", 1], "underline");
    assert!(out.contains("- green"), "sibling item moved");
    assert!(out.contains("- underline"));
}

/// `web` inherits `restart` from `x-defaults`; overriding it for that one
/// service must not touch the anchor or the other consumer.
#[test]
fn compose_override_an_inherited_restart() {
    let src = read("compose.yaml");
    let (out, _) = yaml0::set_with(
        &src,
        path!["services", "web", "restart"],
        "unless-stopped",
        yaml0::SetOpts::new().override_inherited(),
    )
    .unwrap();

    assert!(
        out.contains("  web:\n    <<: *defaults\n    restart: unless-stopped\n    image:"),
        "wrong placement or indent:\n{out}"
    );

    // one line added, every original line still present
    assert_eq!(out.lines().count(), src.lines().count() + 1);
    for line in src.lines() {
        assert!(out.lines().any(|l| l == line), "lost: {line:?}");
    }

    // the anchor and the other service are unchanged
    assert!(out.contains("x-defaults: &defaults\n  restart: always"));
    assert!(out.contains("    restart: on-failure"));
    Parser::new(&out).parse_all().expect("result reparses");
}
