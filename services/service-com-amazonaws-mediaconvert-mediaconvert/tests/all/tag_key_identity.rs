// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the rule that decides which values are tag KEYS, on both
//! doors that write a record's one tag map.
//!
//! A tag key is the identity of an entry in that map, so what makes a value a key is the
//! same question whichever member it arrived in — the `Tags` pairs `TagResource` carries or
//! the `TagKeys` list `UntagResource` carries, both of which target the model's bare
//! `__string` with no pattern and no length
//! (`smithy_query(query="operation",operation="TagResource,UntagResource",model_path="services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json")`),
//! so the framework's boundary validation passes every string alike. A key of zero length
//! addresses no entry of that map: none a write could create, and none a removal could
//! reach. So both doors answer such a key with the modelled `BadRequestException` (HTTP
//! 400) stating the one sentence the state module words that refusal with, and leave the
//! stored set exactly as it was — while a key carrying ANY character is a key on both
//! doors: the write door stores it verbatim and the removal door addresses that very
//! entry.
//!
//! The rows below span what the rule generalises over: whether the value carries any
//! character at all, which characters they are (whitespace, word, punctuation, beyond
//! ASCII), how many there are, and — for the removal door — where in the key list the
//! value sits and how many keys travel beside it. What a stored set reads back as in
//! general is the business of `tests/all/resource_tagging.rs`, and the classification of a
//! refused tag SET the business of `tests/all/shared_tag_refusal.rs`; this suite is about
//! the two doors agreeing on which keys exist.
//!
//! Each case drives the real restJson1 wire path through the crate's one test harness (the
//! router, the SigV4 scope and the reply decoding `job_lifecycle` exports), so what is
//! asserted is the bytes a client would see.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mediaconvert_mediaconvert::state::{
    empty_tag_key_message, not_found_message,
};

use super::job_lifecycle::{arn, queue_arn, Emulator, Reply};
use super::resource_tagging::encode_label;

/// A queue carrying `tags`, and its ARN — the record whose one tag map both doors write.
async fn queue_with_tags(emulator: &Emulator, name: &str, tags: Value) -> String {
    let created = emulator
        .create_queue(json!({ "name": name, "tags": tags }))
        .await;
    assert_eq!(
        created.status, 201,
        "fixture queue {name:?}: expected HTTP 201, got {} with body {}",
        created.status, created.body
    );
    queue_arn(name)
}

/// `TagResource`: the door that WRITES pairs onto a record's tag map.
async fn tag(emulator: &Emulator, resource_arn: &str, tags: Value) -> Reply {
    emulator
        .send(
            "POST",
            "/2017-08-29/tags",
            Some(json!({ "arn": resource_arn, "tags": tags })),
        )
        .await
}

/// `UntagResource`: the door that REMOVES keys from that same map.
async fn untag(emulator: &Emulator, resource_arn: &str, keys: &[&str]) -> Reply {
    emulator
        .send(
            "PUT",
            &format!("/2017-08-29/tags/{}", encode_label(resource_arn)),
            Some(json!({ "tagKeys": keys })),
        )
        .await
}

/// The set the record actually carries, read back through `ListTagsForResource` as a
/// client would, sorted so a comparison does not depend on the order a JSON object
/// happens to carry.
async fn stored_tags(emulator: &Emulator, resource_arn: &str) -> BTreeMap<String, String> {
    let listed = emulator
        .send(
            "GET",
            &format!("/2017-08-29/tags/{}", encode_label(resource_arn)),
            None,
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "reading the stored tags back: expected HTTP 200, got {} with body {}",
        listed.status, listed.body
    );
    listed
        .at("/resourceTags/tags")
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The pairs an expectation is written as.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// What a door answers a value that names no tag: the modelled `BadRequestException`
/// (HTTP 400) stating the sentence the state module words that refusal with — never a
/// second copy of it here.
fn assert_names_no_tag(label: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 400,
        "case {label}: expected HTTP 400, got {} with body {}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("BadRequestException"),
        "case {label}: expected the BadRequestException error type, got {:?} with body {}",
        reply.error_type,
        reply.body
    );
    assert_eq!(
        reply.string("/message"),
        empty_tag_key_message(),
        "case {label}: expected the one empty-key sentence {:?}, got {}",
        empty_tag_key_message(),
        reply.body
    );
}

/// One value a tag key can be, and whether it names a tag at all.
struct Key {
    case: &'static str,
    key: String,
    names_a_tag: bool,
}

/// The values both doors are driven through: the one value that carries no characters, and
/// values spanning the kinds of character a key can be made of and the lengths it can
/// have. `TagKeys` and `Tags` target the bare `__string`, so the service itself is what
/// decides every one of these.
fn keys() -> Vec<Key> {
    vec![
        Key {
            case: "no characters at all",
            key: String::new(),
            names_a_tag: false,
        },
        Key {
            case: "one space",
            key: " ".to_string(),
            names_a_tag: true,
        },
        Key {
            case: "three spaces",
            key: "   ".to_string(),
            names_a_tag: true,
        },
        Key {
            case: "one tab",
            key: "\t".to_string(),
            names_a_tag: true,
        },
        Key {
            case: "a plain word",
            key: "env".to_string(),
            names_a_tag: true,
        },
        Key {
            case: "a word padded with spaces",
            key: " env ".to_string(),
            names_a_tag: true,
        },
        Key {
            case: "punctuation",
            key: "cost-center:team/division=video+alpha@example.com_1.2".to_string(),
            names_a_tag: true,
        },
        Key {
            case: "characters beyond ASCII",
            key: "clé-ключ-キー".to_string(),
            names_a_tag: true,
        },
        Key {
            case: "one character",
            key: "k".to_string(),
            names_a_tag: true,
        },
        Key {
            case: "a long key",
            key: "k".repeat(128),
            names_a_tag: true,
        },
    ]
}

/// The two doors onto a record's tag map agree about which values are keys: a value that
/// names a tag is stored by the write door and addressed by the removal door, and the one
/// value that names no tag earns the same refusal from both, with the stored set untouched.
#[tokio::test]
async fn both_doors_onto_a_records_tags_agree_on_which_values_are_keys() {
    for row in keys() {
        // A store of its own per row, so no earlier row's record decides this one's
        // answer.
        let emulator = Emulator::new();
        let resource_arn = queue_with_tags(&emulator, "tag-key-identity", json!({})).await;
        // One pair that is a key beyond doubt, so every assertion below is about the row's
        // value and not about an empty map.
        let seeded = tag(&emulator, &resource_arn, json!({ "keep": "yes" })).await;
        assert_eq!(
            seeded.status, 200,
            "fixture tag for {:?}: expected HTTP 200, got {} with body {}",
            row.case, seeded.status, seeded.body
        );

        // The row's value as the KEY of a `Tags` pair, built through the JSON map so the
        // key is the row's bytes exactly.
        let mut pairs = serde_json::Map::new();
        pairs.insert(row.key.clone(), Value::String("v".to_string()));
        let written = tag(&emulator, &resource_arn, Value::Object(pairs)).await;
        if row.names_a_tag {
            assert_eq!(
                written.status, 200,
                "case {:?}/TagResource: expected HTTP 200, got {} with body {}",
                row.case, written.status, written.body
            );
            assert_eq!(
                stored_tags(&emulator, &resource_arn).await,
                expected(&[("keep", "yes"), (row.key.as_str(), "v")]),
                "case {:?}/TagResource: the key must be stored verbatim beside the pair \
                 that was already there",
                row.case
            );
        } else {
            assert_names_no_tag(&format!("{:?}/TagResource", row.case), &written);
            assert_eq!(
                stored_tags(&emulator, &resource_arn).await,
                expected(&[("keep", "yes")]),
                "case {:?}/TagResource: a refused write must store nothing",
                row.case
            );
        }

        let removed = untag(&emulator, &resource_arn, &[row.key.as_str()]).await;
        if row.names_a_tag {
            assert_eq!(
                removed.status, 200,
                "case {:?}/UntagResource: expected HTTP 200, got {} with body {}",
                row.case, removed.status, removed.body
            );
            assert_eq!(
                stored_tags(&emulator, &resource_arn).await,
                expected(&[("keep", "yes")]),
                "case {:?}/UntagResource: the entry the write door created must be the one \
                 this key addresses",
                row.case
            );
        } else {
            assert_names_no_tag(&format!("{:?}/UntagResource", row.case), &removed);
            assert_eq!(
                stored_tags(&emulator, &resource_arn).await,
                expected(&[("keep", "yes")]),
                "case {:?}/UntagResource: a refused removal must remove nothing",
                row.case
            );
        }
    }
}

/// A key that names no tag refuses the WHOLE removal, wherever in the list it sits and
/// whatever travels beside it — and a list made only of keys is served, removing exactly
/// the entries those keys address.
#[tokio::test]
async fn a_key_naming_no_tag_refuses_the_whole_removal_wherever_it_sits() {
    /// One key list, and the set the record carries afterwards — `None` when the call is
    /// refused, in which case the record keeps the set it had.
    struct Case {
        case: &'static str,
        keys: Vec<&'static str>,
        remaining: Option<Vec<(&'static str, &'static str)>>,
    }

    let cases = vec![
        Case {
            case: "the value that names no tag, alone",
            keys: vec![""],
            remaining: None,
        },
        Case {
            case: "it first, a key after it",
            keys: vec!["", "alpha"],
            remaining: None,
        },
        Case {
            case: "it last, a key before it",
            keys: vec!["alpha", ""],
            remaining: None,
        },
        Case {
            case: "it between two keys",
            keys: vec!["alpha", "", "beta"],
            remaining: None,
        },
        Case {
            case: "it twice",
            keys: vec!["", ""],
            remaining: None,
        },
        Case {
            case: "it beside a key the record does not carry",
            keys: vec!["absent", ""],
            remaining: None,
        },
        Case {
            case: "no keys at all",
            keys: vec![],
            remaining: Some(vec![("alpha", "1"), ("beta", "2")]),
        },
        Case {
            case: "one key the record carries",
            keys: vec!["alpha"],
            remaining: Some(vec![("beta", "2")]),
        },
        Case {
            case: "one key the record does not carry",
            keys: vec!["absent"],
            remaining: Some(vec![("alpha", "1"), ("beta", "2")]),
        },
        Case {
            case: "every key the record carries",
            keys: vec!["alpha", "beta"],
            remaining: Some(vec![]),
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        let resource_arn = queue_with_tags(
            &emulator,
            "tag-key-list",
            json!({ "alpha": "1", "beta": "2" }),
        )
        .await;

        let reply = untag(&emulator, &resource_arn, &case.keys).await;
        match &case.remaining {
            Some(remaining) => {
                assert_eq!(
                    reply.status, 200,
                    "case {:?}: expected HTTP 200, got {} with body {}",
                    case.case, reply.status, reply.body
                );
                assert_eq!(
                    stored_tags(&emulator, &resource_arn).await,
                    expected(remaining),
                    "case {:?}: expected exactly the keys the call did not name to remain",
                    case.case
                );
            }
            None => {
                assert_names_no_tag(case.case, &reply);
                assert_eq!(
                    stored_tags(&emulator, &resource_arn).await,
                    expected(&[("alpha", "1"), ("beta", "2")]),
                    "case {:?}: a refused removal must leave the stored set exactly as it \
                     was, keys named beside the bad one included",
                    case.case
                );
            }
        }
    }
}

/// A value that names no tag is a problem in the REQUEST, so both doors answer it as one
/// before the ARN is resolved against the store: the same 400 whether or not the ARN names
/// a resource this store holds, while the same call with a real key is the lookup refusal.
#[tokio::test]
async fn a_key_naming_no_tag_is_a_request_problem_whatever_the_arn_names() {
    let emulator = Emulator::new();
    // A resource DOES exist, so an answer below is about the ARN in the request, not an
    // empty store.
    let _ = queue_with_tags(&emulator, "tag-key-present", json!({ "env": "prod" })).await;

    // The ARN forms MediaConvert documents as taggable, naming resources nobody created.
    let absent = [
        ("a queue nobody created", arn("queues/absent")),
        ("a preset nobody created", arn("presets/Not-A-Preset")),
        (
            "a job template nobody created",
            arn("jobTemplates/Not-A-Template"),
        ),
    ];

    for (case, resource_arn) in &absent {
        let written = tag(&emulator, resource_arn, json!({ "": "v" })).await;
        assert_names_no_tag(&format!("{case}/TagResource"), &written);
        let removed = untag(&emulator, resource_arn, &[""]).await;
        assert_names_no_tag(&format!("{case}/UntagResource"), &removed);

        // The control: the same ARN with a value that IS a key gets as far as the lookup,
        // which is what reports the missing resource.
        let looked_up = untag(&emulator, resource_arn, &["env"]).await;
        assert_eq!(
            looked_up.status, 404,
            "case {case}/UntagResource with a real key: expected HTTP 404, got {} with \
             body {}",
            looked_up.status, looked_up.body
        );
        assert_eq!(
            looked_up.error_type.as_deref(),
            Some("NotFoundException"),
            "case {case}/UntagResource with a real key: expected the NotFoundException \
             error type, got {:?} with body {}",
            looked_up.error_type,
            looked_up.body
        );
        assert_eq!(
            looked_up.string("/message"),
            not_found_message("arn"),
            "case {case}/UntagResource with a real key: expected the one not-found \
             sentence {:?}, got {}",
            not_found_message("arn"),
            looked_up.body
        );
    }

    // None of the refusals above touched the resource that does exist.
    assert_eq!(
        stored_tags(&emulator, &queue_arn("tag-key-present")).await,
        expected(&[("env", "prod")]),
        "the existing queue's tags must survive every refusal above"
    );
}
