// SPDX-License-Identifier: CC0-1.0

//! What a tag KEY and VALUE may contain, end to end over the wire.
//!
//! The merge semantics of the tagging slice live in `tests/all/pipe_tagging.rs`; this
//! suite is about the rules a tag's CONTENT must satisfy before it is stored at all, on
//! EVERY write that installs tags — `TagResource` and the `Tags` a `CreatePipe` carries:
//!
//! * `TagKey` is `@length(min: 1, max: 128)` and `TagValue` is `@length(min: 0, max: 256)`
//!   (source: the `TagMap` key and value members of
//!   `smithy-model/com-amazonaws-pipes-pipes.json`). A constraint on a map KEY is one the
//!   boundary validator does not walk, so a key outside the bound reaches the handler and
//!   has to be refused there with the modelled `ValidationException`.
//! * `aws:` is reserved for AWS's own tags (AWS General Reference, "Tag naming limits and
//!   requirements"), so a caller may not assign a key using that prefix, in any casing.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch`, reusing the harness
//! `pipe_lifecycle` owns and the tagging helpers `pipe_tagging` owns, and every refused
//! call is checked to have persisted NOTHING through the readers of the stored map.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::pipe_lifecycle::{create, describe, router};
use crate::pipe_tagging::{assert_tags_agree, create_body, pipe_arn, tag, tag_map};

/// `TagKey`'s published maximum (128) and `TagValue`'s (256).
const MAX_TAG_KEY_LEN: usize = 128;
const MAX_TAG_VALUE_LEN: usize = 256;

/// A tag object from pairs whose keys are computed rather than literal (`json!` takes
/// only literal keys).
fn tags_of(pairs: Vec<(String, String)>) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in pairs {
        map.insert(key, Value::String(value));
    }
    Value::Object(map)
}

/// One tag, from borrowed parts.
fn one_tag(key: &str, value: &str) -> Value {
    tags_of(vec![(key.to_string(), value.to_string())])
}

/// The rows every write that installs tags must agree on: the tag, and whether the
/// service accepts it.
///
/// They span what the rules generalise over: each length bound from inside and one past
/// it, at both ends; the reserved prefix in several casings and in a position where it is
/// NOT reserved; and unicode keys, which are measured in characters rather than bytes.
fn rows() -> Vec<(&'static str, Value, bool)> {
    let at_key_bound = "k".repeat(MAX_TAG_KEY_LEN);
    let past_key_bound = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let at_value_bound = "v".repeat(MAX_TAG_VALUE_LEN);
    let past_value_bound = "v".repeat(MAX_TAG_VALUE_LEN + 1);
    // 128 characters, 384 bytes: `@length` counts CHARACTERS, so this is ON the bound.
    let unicode_at_key_bound = "é".repeat(MAX_TAG_KEY_LEN);
    let unicode_past_key_bound = "é".repeat(MAX_TAG_KEY_LEN + 1);

    vec![
        ("plain-key-and-value", one_tag("env", "prod"), true),
        ("shortest-key-and-empty-value", one_tag("k", ""), true),
        (
            "key-and-value-exactly-at-their-bounds",
            tags_of(vec![(at_key_bound, at_value_bound)]),
            true,
        ),
        ("empty-key", one_tag("", "prod"), false),
        (
            "key-one-character-past-its-bound",
            tags_of(vec![(past_key_bound, "prod".to_string())]),
            false,
        ),
        (
            "value-one-character-past-its-bound",
            tags_of(vec![("env".to_string(), past_value_bound)]),
            false,
        ),
        (
            "unicode-key-exactly-at-its-bound",
            tags_of(vec![(unicode_at_key_bound, "prod".to_string())]),
            true,
        ),
        (
            "unicode-key-one-character-past-its-bound",
            tags_of(vec![(unicode_past_key_bound, "prod".to_string())]),
            false,
        ),
        (
            "reserved-prefix-lower-case",
            one_tag("aws:cloudformation:stack-name", "demo"),
            false,
        ),
        (
            "reserved-prefix-upper-case",
            one_tag("AWS:createdBy", "demo"),
            false,
        ),
        (
            "reserved-prefix-mixed-case",
            one_tag("Aws:Owner", "demo"),
            false,
        ),
        ("reserved-prefix-alone", one_tag("aws:", "demo"), false),
        // Only the PREFIX is reserved: `aws` elsewhere in the key is an ordinary key, and
        // so is a value that starts with it.
        (
            "aws-later-in-the-key-is-ordinary",
            one_tag("team-aws:owner", "aws:demo"),
            true,
        ),
        (
            "a-legal-tag-beside-an-illegal-one",
            tags_of(vec![
                ("env".to_string(), "prod".to_string()),
                ("aws:owner".to_string(), "demo".to_string()),
            ]),
            false,
        ),
    ]
}

/// RULE: `TagResource` refuses a tag whose key or value breaks the published length bound,
/// or whose key uses the prefix AWS reserves, with 400 `ValidationException`, and persists
/// nothing for it; a tag inside those rules is stored and reported.
#[tokio::test]
async fn tag_resource_refuses_a_tag_whose_key_or_value_breaks_the_published_rules() {
    let router = router();

    for (index, (case, tags, accepted)) in rows().into_iter().enumerate() {
        let name = format!("tagrules{index}");
        let initial = json!({"kept": "yes"});
        let created = create(&router, &name, create_body(Some(&initial))).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );

        let reply = tag(&router, &pipe_arn(&name), &tags).await;
        let expected = if accepted {
            (200, "")
        } else {
            (400, "ValidationException")
        };
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {reply:?}"
        );

        // A refused call leaves the pipe exactly as it was; an accepted one merges.
        let mut stored = tag_map(&initial);
        if accepted {
            stored.extend(tag_map(&tags));
        }
        assert_tags_agree(&router, case, &name, &stored).await;
    }
}

/// RULE, on the other write that installs tags: `CreatePipe` enforces the same rules over
/// the `Tags` it carries, so a pipe can never be created carrying a tag `TagResource`
/// would refuse — and a refused create stores no pipe at all.
#[tokio::test]
async fn create_pipe_refuses_the_same_tags_tag_resource_refuses() {
    let router = router();

    for (index, (case, tags, accepted)) in rows().into_iter().enumerate() {
        let name = format!("createrules{index}");
        let created = create(&router, &name, create_body(Some(&tags))).await;
        let expected = if accepted {
            (200, "")
        } else {
            (400, "ValidationException")
        };
        assert_eq!(
            (created.status, created.error_code.as_str()),
            expected,
            "case {case:?}: expected {expected:?}, got {created:?}"
        );

        if accepted {
            assert_tags_agree(&router, case, &name, &tag_map(&tags)).await;
        } else {
            let described = describe(&router, &name).await;
            assert_eq!(
                (described.status, described.error_code.as_str()),
                (404, "NotFoundException"),
                "case {case:?}: a refused create must store no pipe, got {described:?}"
            );
        }
    }
}

/// RULE: the refusal is reported the way the model declares it — 400 with error code
/// `ValidationException` and a `fieldList` entry naming the input member the tags arrived
/// on, which differs between the two writes (`tags` on `TagResource`, `Tags` on
/// `CreatePipe`).
#[tokio::test]
async fn a_refused_tag_names_the_input_member_it_arrived_on() {
    let router = router();
    let name = "tagrulesfieldlist";
    let created = create(&router, name, create_body(None)).await;
    assert_eq!(
        created.status, 200,
        "CreatePipe must succeed, got {created:?}"
    );

    let refused = tag(&router, &pipe_arn(name), &one_tag("aws:owner", "demo")).await;
    assert_eq!(
        (refused.status, refused.error_code.as_str()),
        (400, "ValidationException"),
        "TagResource must refuse a reserved key with 400 ValidationException, got {refused:?}"
    );
    let names: Vec<String> = refused
        .member("fieldList")
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .map(|field| {
                    field
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["tags".to_string()],
        "TagResource's refusal must name its `tags` member, got {refused:?}"
    );

    let refused_create = create(
        &router,
        "tagrulesfieldlistcreate",
        create_body(Some(&one_tag("aws:owner", "demo"))),
    )
    .await;
    assert_eq!(
        (refused_create.status, refused_create.error_code.as_str()),
        (400, "ValidationException"),
        "CreatePipe must refuse a reserved key with 400 ValidationException, \
         got {refused_create:?}"
    );
    let create_names: Vec<String> = refused_create
        .member("fieldList")
        .as_array()
        .map(|fields| {
            fields
                .iter()
                .map(|field| {
                    field
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        create_names,
        vec!["Tags".to_string()],
        "CreatePipe's refusal must name its `Tags` member, got {refused_create:?}"
    );

    // Neither refusal wrote anything.
    assert_tags_agree(&router, "untouched-after-refusals", name, &BTreeMap::new()).await;
}
