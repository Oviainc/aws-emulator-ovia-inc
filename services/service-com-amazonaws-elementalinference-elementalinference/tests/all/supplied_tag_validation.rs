// SPDX-License-Identifier: CC0-1.0

//! The rules a SUPPLIED tag map has to satisfy, end to end over the wire, on every
//! operation of this service that takes one.
//!
//! A `TagMap` arrives at three operations — `CreateFeed`, `CreateDictionary` and
//! `TagResource` — and the rules a supplied map has to satisfy belong to the TAG, not to
//! the operation that carried it, so every row below is asserted on all three:
//!
//! * a tag key lies outside the reserved namespace: a key beginning with the prefix AWS
//!   reserves for its own tags is refused with the modelled `ValidationException`
//!   (HTTP 400) and nothing is stored, while a key that merely contains that prefix
//!   further along, or carries it in another case, is a key of the caller's own
//!   namespace and is stored like any other (a tag key is case-sensitive);
//! * a request asks a resource to carry no more tags than a resource may hold
//!   ([`MAX_TAGS_PER_RESOURCE`]) — accepted at the cap, refused past it;
//! * a tag key is within the `@length` bounds `TagMap`'s key models (1..128,
//!   `smithy_query(query="shape",shape="TagMap")`), which the handler enforces because
//!   the framework's boundary walk reads a map entry's key as the path its value is
//!   reported under rather than as a constrained leaf.
//!
//! A refusal writes NOTHING: a refused create mints no resource (so the store still
//! lists none under that name) and a refused tag write leaves the stored tag set alone.
//! An accepted map round-trips through the resource's own implemented reader
//! (`GetFeed` / `GetDictionary`).

use crate::harness::{call, output_config, router};
use serde_json::{json, Value};
use service_com_amazonaws_elementalinference_elementalinference::state::{
    invalid_tag_key_message, reserved_tag_key_message, too_many_tags_message,
    MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX, TAG_KEY_MAX_LENGTH,
};
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeMap;

/// The two kinds of resource this service mints and tags, as the modelled `resourceArn`
/// pattern spells them (`…:(feed|dictionary)/[a-zA-Z0-9]{1,19}$`).
const KINDS: [&str; 2] = ["feed", "dictionary"];

/// One row: a tag map a request may carry, whether this service stores it, and the
/// refusal it earns when it does not.
struct TagCase {
    label: &'static str,
    tags: Vec<(String, String)>,
    admitted: bool,
    message: Option<String>,
}

/// A pair, from two `&str`s.
fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `count` distinct ordinary tags, for the rows about HOW MANY tags a request carries.
fn many_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| pair(&format!("key{index}"), &format!("value{index}")))
        .collect()
}

/// Rows spanning what the rules generalise over: where the reserved prefix sits in the
/// key and in what case, how many tags the request carries, and how long a key is.
///
/// The reserved prefix is [`RESERVED_TAG_KEY_PREFIX`] and the cap
/// [`MAX_TAGS_PER_RESOURCE`]; the key length bounds are `TagMap`'s own
/// (`smithy_query(query="shape",shape="TagMap")`), so the rows sit on each published
/// bound and one past it.
fn tag_cases() -> Vec<TagCase> {
    let reserved = RESERVED_TAG_KEY_PREFIX;
    let overlong_reserved = format!("{reserved}{}", "k".repeat(TAG_KEY_MAX_LENGTH));
    vec![
        TagCase {
            label: "an ordinary tag",
            tags: vec![pair("env", "prod")],
            admitted: true,
            message: None,
        },
        TagCase {
            label: "no tag at all",
            tags: vec![],
            admitted: true,
            message: None,
        },
        TagCase {
            label: "a key in the reserved namespace",
            tags: vec![pair(&format!("{reserved}cloudformation:stack-name"), "mine")],
            admitted: false,
            message: Some(reserved_tag_key_message(&format!(
                "{reserved}cloudformation:stack-name"
            ))),
        },
        TagCase {
            label: "the bare reserved prefix as the whole key",
            tags: vec![pair(reserved, "mine")],
            admitted: false,
            message: Some(reserved_tag_key_message(reserved)),
        },
        TagCase {
            label: "one reserved key among ordinary ones",
            tags: vec![
                pair("env", "prod"),
                pair(&format!("{reserved}secretsmanager:owning-service"), "x"),
                pair("team", "video"),
            ],
            admitted: false,
            message: Some(reserved_tag_key_message(&format!(
                "{reserved}secretsmanager:owning-service"
            ))),
        },
        TagCase {
            label: "the reserved prefix in upper case: a tag key is case-sensitive",
            tags: vec![pair(&reserved.to_uppercase(), "mine")],
            admitted: true,
            message: None,
        },
        TagCase {
            label: "the reserved prefix further along the key, not at its start",
            tags: vec![pair(&format!("team:{reserved}thing"), "mine")],
            admitted: true,
            message: None,
        },
        TagCase {
            label: "a key that starts with the prefix's letters but not the prefix",
            tags: vec![pair("awsome", "yes")],
            admitted: true,
            message: None,
        },
        TagCase {
            label: "exactly the cap's worth of tags",
            tags: many_tags(MAX_TAGS_PER_RESOURCE),
            admitted: true,
            message: None,
        },
        TagCase {
            label: "one tag short of the cap",
            tags: many_tags(MAX_TAGS_PER_RESOURCE - 1),
            admitted: true,
            message: None,
        },
        TagCase {
            label: "one tag past the cap",
            tags: many_tags(MAX_TAGS_PER_RESOURCE + 1),
            admitted: false,
            message: Some(too_many_tags_message(MAX_TAGS_PER_RESOURCE + 1)),
        },
        TagCase {
            label: "well past the cap",
            tags: many_tags(MAX_TAGS_PER_RESOURCE + 10),
            admitted: false,
            message: Some(too_many_tags_message(MAX_TAGS_PER_RESOURCE + 10)),
        },
        TagCase {
            label: "a key at the maximum modelled length",
            tags: vec![pair(&"k".repeat(TAG_KEY_MAX_LENGTH), "v")],
            admitted: true,
            message: None,
        },
        TagCase {
            label: "a key one character past the maximum modelled length",
            tags: vec![pair(&"k".repeat(TAG_KEY_MAX_LENGTH + 1), "v")],
            admitted: false,
            message: Some(invalid_tag_key_message(TAG_KEY_MAX_LENGTH + 1)),
        },
        TagCase {
            label: "an empty key, one character short of the minimum",
            tags: vec![pair("", "v")],
            admitted: false,
            message: Some(invalid_tag_key_message(0)),
        },
        TagCase {
            label: "a reserved key that is also too long: the modelled bound is decided first",
            tags: vec![(overlong_reserved.clone(), "v".to_string())],
            admitted: false,
            message: Some(invalid_tag_key_message(
                overlong_reserved.chars().count(),
            )),
        },
    ]
}

/// A `TagMap` body member built from a list of pairs.
fn tag_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// The `tags` member of a resource response, as a map so a comparison does not depend on
/// key order.
fn tags_of(resource: &Value) -> BTreeMap<String, String> {
    resource["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("a resource response carries a tags map, got {resource}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("a tag value is a string, got {value}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The create body of `kind`, named `name` and carrying `tags`.
fn create_body(kind: &str, name: &str, tags: &[(String, String)]) -> Value {
    match kind {
        "feed" => json!({
            "name": name,
            "outputs": [
                {"name": "crop1", "outputConfig": output_config("cropping"), "status": "ENABLED"}
            ],
            "tags": tag_object(tags),
        }),
        "dictionary" => json!({"name": name, "language": "eng", "tags": tag_object(tags)}),
        other => panic!("no such resource kind: {other}"),
    }
}

/// The path the collection of `kind` is listed under (`ListFeeds` / `ListDictionaries`).
fn list_path(kind: &str) -> &'static str {
    match kind {
        "feed" => "/v1/feeds",
        "dictionary" => "/v1/dictionaries",
        other => panic!("no such resource kind: {other}"),
    }
}

/// The member a list response reports its page under.
fn list_member(kind: &str) -> &'static str {
    match kind {
        "feed" => "feeds",
        "dictionary" => "dictionaries",
        other => panic!("no such resource kind: {other}"),
    }
}

/// How many resources of `kind` the store holds under `name`.
async fn stored_under_name(router: &MultiServiceRouter, kind: &str, name: &str) -> usize {
    let (status, listed) = call(router, "GET", list_path(kind), None).await;
    assert_eq!(
        status, 200,
        "listing {kind}s answers 200, got {status} with {listed}"
    );
    listed[list_member(kind)]
        .as_array()
        .unwrap_or_else(|| panic!("a {kind} list reports an array, got {listed}"))
        .iter()
        .filter(|summary| summary["name"] == json!(name))
        .count()
}

/// Assert the refusal a row expects, whichever operation answered it.
fn assert_refusal(label: &str, case: &TagCase, status: u16, body: &Value) {
    assert_eq!(
        status, 400,
        "case {label:?}: a tag map this service does not store is refused 400, got {status} \
         with {body}"
    );
    assert_eq!(
        body["__type"],
        json!("ValidationException"),
        "case {label:?}: that 400 is the modelled ValidationException, got {body}"
    );
    if let Some(message) = &case.message {
        assert_eq!(
            body["message"],
            json!(message),
            "case {label:?}: expected {message:?}, got {body}"
        );
    }
}

/// `CreateFeed` and `CreateDictionary` judge a supplied tag map before minting anything:
/// an admitted map round-trips through the resource's reader, a refused one leaves no
/// resource behind.
#[tokio::test]
async fn a_create_stores_only_a_tag_map_the_service_admits() {
    for kind in KINDS {
        for (index, case) in tag_cases().into_iter().enumerate() {
            let router = router();
            let label = format!("{} on a created {kind}", case.label);
            let name = format!("tagcreate{index}");
            let (status, body) = call(
                &router,
                "POST",
                &format!("/v1/{kind}"),
                Some(create_body(kind, &name, &case.tags)),
            )
            .await;

            if case.admitted {
                assert_eq!(
                    status, 202,
                    "case {label:?}: a tag map this service stores is accepted, got {status} \
                     with {body}"
                );
                let expected: BTreeMap<String, String> = case.tags.iter().cloned().collect();
                assert_eq!(
                    tags_of(&body),
                    expected,
                    "case {label:?}: the create reports the tags it stored, got {body}"
                );
                let id = body["id"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {label:?}: a created {kind} carries an id, got {body}"));
                let (status, stored) =
                    call(&router, "GET", &format!("/v1/{kind}/{id}"), None).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: the created {kind} reads back, got {status} with {stored}"
                );
                assert_eq!(
                    tags_of(&stored),
                    expected,
                    "case {label:?}: the stored tags are what the next read reports, got {stored}"
                );
            } else {
                assert_refusal(&label, &case, status, &body);
                assert_eq!(
                    stored_under_name(&router, kind, &name).await,
                    0,
                    "case {label:?}: a refused create mints no {kind}"
                );
            }
        }
    }
}

/// `TagResource` judges a supplied tag map by the same rules: an admitted map is merged
/// onto the resource, a refused one leaves the stored set alone.
#[tokio::test]
async fn a_tag_write_stores_only_a_tag_map_the_service_admits() {
    for kind in KINDS {
        for (index, case) in tag_cases().into_iter().enumerate() {
            let router = router();
            let label = format!("{} written onto a {kind}", case.label);
            let name = format!("tagwrite{index}");
            // The resource starts UNTAGGED, so the set the request asks for is the set
            // the resource would end up carrying — the same quantity a create is judged
            // on.
            let (status, created) = call(
                &router,
                "POST",
                &format!("/v1/{kind}"),
                Some(create_body(kind, &name, &[])),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {label:?}: the {kind} under test is created, got {status} with {created}"
            );
            let id = created["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: a created {kind} carries an id, got {created}"))
                .to_string();
            let arn = created["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {label:?}: a created {kind} carries an arn, got {created}"))
                .to_string();

            let encoded: String = arn
                .bytes()
                .map(|byte| match byte {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                        char::from(byte).to_string()
                    }
                    other => format!("%{other:02X}"),
                })
                .collect();
            let (status, body) = call(
                &router,
                "POST",
                &format!("/v1/tags/{encoded}"),
                Some(json!({"tags": tag_object(&case.tags)})),
            )
            .await;

            let (_, stored) = call(&router, "GET", &format!("/v1/{kind}/{id}"), None).await;
            let expected: BTreeMap<String, String> = if case.admitted {
                case.tags.iter().cloned().collect()
            } else {
                BTreeMap::new()
            };
            if case.admitted {
                assert_eq!(
                    status, 204,
                    "case {label:?}: a tag map this service stores is accepted, got {status} \
                     with {body}"
                );
            } else {
                assert_refusal(&label, &case, status, &body);
            }
            assert_eq!(
                tags_of(&stored),
                expected,
                "case {label:?}: the {kind} carries exactly the tags the write left, got {stored}"
            );
        }
    }
}
