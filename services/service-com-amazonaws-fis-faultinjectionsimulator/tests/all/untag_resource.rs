// SPDX-License-Identifier: CC0-1.0

//! `UntagResource`, end to end over HTTP.
//!
//! The rule under test is the REMOVING half of FIS's tagging surface: a call names its
//! resource by ARN and the tag keys to remove in the query string, the ARN is resolved in
//! this service's OWN namespace (an ARN this service could not have issued is a bad
//! request, an ARN naming a resource of this service the store holds no record of is a
//! miss), and otherwise EXACTLY the keys the request names leave that resource's own tag map — a key
//! it does not carry is nothing to remove rather than a failure, a key the request does
//! not name keeps the value it had, a request naming no key removes nothing, and nothing
//! else about the record changes. A successful removal answers 204 No Content. The tags a
//! call leaves are the tags `ListTagsForResource`
//! and the resource's own reader report, which is what makes the removal persisted rather
//! than acknowledged.
//!
//! The rows below differ along everything that rule generalises over: WHICH kind of
//! resource is untagged (an experiment template and an experiment, the two records of this
//! service that carry caller tags), how many keys a call names (none, one, three, every key
//! the resource carries, the most a `TagMap` holds), whether a named key is carried or
//! absent, how many calls the removals arrive in, and the shape of a key (plain,
//! whitespace, non-ASCII, and at the ceiling `TagKey` publishes).
//!
//! The resources, the tag-map helpers, the modelled ceilings and the table of ARNs this
//! service cannot address come from the tag WRITE suite (`resource_tagging`), so the three
//! halves of the one rule — write, read and remove — are asserted over the same inputs.

use std::collections::BTreeMap;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{emulator, send, Answer, FALLBACK_REGION};
use crate::resource_tagging::{
    generated, label, new_resource, new_template, pairs, read_resource, tag_map, tag_resource,
    tags_of, unaddressable_arns, unminted_arns, without_tags, Kind, TAGS_PER_RESOURCE,
    TAG_KEY_CEILING,
};

/// Percent-encode one query value. `tagKeys` is `@httpQuery`, so a key carrying a space,
/// a `+`, a `&` or a non-ASCII character reaches the server only when the client encodes
/// it; the encoding is the maintained `percent-encoding` crate's, over everything outside
/// the alphanumerics, which is what an SDK does to a query value.
fn query_value(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// `UntagResource`: remove the keys `tag_keys` names from the resource `resource_arn`
/// names. `None` sends no `tagKeys` parameter at all.
async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: Option<&[String]>,
) -> Answer {
    let query = match tag_keys {
        None => String::new(),
        Some(keys) => {
            let pairs: Vec<String> = keys
                .iter()
                .map(|key| format!("tagKeys={}", query_value(key)))
                .collect();
            format!("?{}", pairs.join("&"))
        }
    };
    send(
        router,
        "DELETE",
        &format!("/tags/{}{query}", label(resource_arn)),
        None,
        FALLBACK_REGION,
    )
    .await
}

/// One row of the removal table: the tags the resource starts with, the `tagKeys` of each
/// call in order (`None` for a call carrying none), and the tags its readers must report
/// afterwards.
struct Row {
    case: String,
    initial: Vec<(String, String)>,
    removals: Vec<Option<Vec<String>>>,
    expected: Vec<(String, String)>,
}

fn keys(names: &[&str]) -> Option<Vec<String>> {
    Some(names.iter().map(|name| (*name).to_string()).collect())
}

/// The names of the pairs `generated` builds, so a row can remove what it created.
fn generated_keys(count: usize) -> Option<Vec<String>> {
    Some(
        generated(count, "ignored")
            .into_iter()
            .map(|(key, _)| key)
            .collect(),
    )
}

fn removal_rows() -> Vec<Row> {
    let key_at_ceiling = "k".repeat(TAG_KEY_CEILING);
    vec![
        Row {
            case: "one key from a resource carrying three".to_string(),
            initial: pairs(&[("Env", "test"), ("Owner", "fis"), ("Team", "chaos")]),
            removals: vec![keys(&["Env"])],
            expected: pairs(&[("Owner", "fis"), ("Team", "chaos")]),
        },
        Row {
            case: "three keys in one call".to_string(),
            initial: pairs(&[
                ("Env", "test"),
                ("Owner", "fis"),
                ("Suite", "untag"),
                ("Team", "chaos"),
            ]),
            removals: vec![keys(&["Env", "Owner", "Team"])],
            expected: pairs(&[("Suite", "untag")]),
        },
        Row {
            case: "every key the resource carries".to_string(),
            initial: pairs(&[("Env", "test"), ("Owner", "fis")]),
            removals: vec![keys(&["Env", "Owner"])],
            expected: Vec::new(),
        },
        Row {
            case: "a key the resource does not carry".to_string(),
            initial: pairs(&[("Env", "test")]),
            removals: vec![keys(&["Absent"])],
            expected: pairs(&[("Env", "test")]),
        },
        Row {
            case: "a carried key alongside one the resource does not carry".to_string(),
            initial: pairs(&[("Env", "test"), ("Owner", "fis")]),
            removals: vec![keys(&["Env", "Absent"])],
            expected: pairs(&[("Owner", "fis")]),
        },
        Row {
            case: "the same removal twice is the same as removing once".to_string(),
            initial: pairs(&[("Env", "test"), ("Owner", "fis")]),
            removals: vec![keys(&["Env"]), keys(&["Env"])],
            expected: pairs(&[("Owner", "fis")]),
        },
        Row {
            case: "removals arriving in three calls accumulate".to_string(),
            initial: pairs(&[
                ("Env", "test"),
                ("Owner", "fis"),
                ("Suite", "untag"),
                ("Team", "chaos"),
            ]),
            removals: vec![keys(&["Env"]), keys(&["Team"]), keys(&["Owner"])],
            expected: pairs(&[("Suite", "untag")]),
        },
        Row {
            case: "no tagKeys parameter at all removes nothing".to_string(),
            initial: pairs(&[("Env", "test"), ("Owner", "fis")]),
            removals: vec![None],
            expected: pairs(&[("Env", "test"), ("Owner", "fis")]),
        },
        Row {
            case: "a key carrying whitespace, which TagKey's pattern admits".to_string(),
            initial: pairs(&[("two words", "a b"), (" ", "just a space"), ("Env", "test")]),
            removals: vec![keys(&["two words", " "])],
            expected: pairs(&[("Env", "test")]),
        },
        Row {
            case: "a non-ASCII key".to_string(),
            initial: pairs(&[("環境", "本番"), ("naïve", "café")]),
            removals: vec![keys(&["環境"])],
            expected: pairs(&[("naïve", "café")]),
        },
        Row {
            case: "a key at TagKey's published ceiling".to_string(),
            initial: vec![
                (key_at_ceiling.clone(), "x".to_string()),
                ("Env".to_string(), "test".to_string()),
            ],
            removals: vec![Some(vec![key_at_ceiling])],
            expected: pairs(&[("Env", "test")]),
        },
        Row {
            case: "a key whose value is empty".to_string(),
            initial: pairs(&[("Env", ""), ("Owner", "fis")]),
            removals: vec![keys(&["Env"])],
            expected: pairs(&[("Owner", "fis")]),
        },
        Row {
            case: "ten keys of the most tags a resource carries".to_string(),
            initial: generated(TAGS_PER_RESOURCE, "kept"),
            removals: vec![generated_keys(10)],
            expected: generated(TAGS_PER_RESOURCE, "kept")
                .into_iter()
                .skip(10)
                .collect(),
        },
        Row {
            case: "the most tags a resource carries, all of them".to_string(),
            initial: generated(TAGS_PER_RESOURCE, "kept"),
            removals: vec![generated_keys(TAGS_PER_RESOURCE)],
            expected: Vec::new(),
        },
    ]
}

/// Every answer `UntagResource` gives on success: HTTP 204 carrying no members.
fn assert_empty_success(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 204,
        "case {case:?}: expected HTTP 204 from UntagResource, got {} with {:?}",
        answer.status, answer.body
    );
    let empty = match &answer.body {
        Value::Null => true,
        Value::Object(members) => members.is_empty(),
        _ => false,
    };
    assert!(
        empty,
        "case {case:?}: UntagResource's response carries no members, got {:?}",
        answer.body
    );
}

/// The removal rule, over both kinds of taggable resource.
#[tokio::test]
async fn untag_removes_exactly_the_named_keys_from_the_persisted_tags() {
    for kind in [Kind::ExperimentTemplate, Kind::Experiment] {
        for row in removal_rows() {
            let case = format!("{} / {}", kind.as_str(), row.case);
            // One emulator per row, so each row judges a resource of its own.
            let router = emulator();
            let resource = new_resource(&router, kind, &case, &tag_map(&row.initial)).await;
            let before = without_tags(&read_resource(&router, &resource, &case).await);

            for (index, removal) in row.removals.iter().enumerate() {
                let answer = untag_resource(&router, &resource.arn, removal.as_deref()).await;
                assert_empty_success(&answer, &format!("{case} / call {index}"));
            }

            let expected: BTreeMap<String, String> = row.expected.iter().cloned().collect();

            // The one map: the resource's own reader reports what the removal left.
            let record = read_resource(&router, &resource, &case).await;
            assert_eq!(
                tags_of(&record, &case),
                expected,
                "case {case:?}: expected the remaining tags, got {:?}",
                record["tags"]
            );

            // And so does the tag read, which answers the same persisted map.
            let listed = send(
                &router,
                "GET",
                &format!("/tags/{}", label(&resource.arn)),
                None,
                FALLBACK_REGION,
            )
            .await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} with {:?}",
                listed.status, listed.body
            );
            assert_eq!(
                tags_of(&listed.body, &case),
                expected,
                "case {case:?}: the tag read must report what the removal left, got {:?}",
                listed.member("tags")
            );

            if kind == Kind::ExperimentTemplate {
                // A template is a resource this service reports in one terminal shape, so
                // everything but its tag map must be exactly what it was.
                assert_eq!(
                    without_tags(&record),
                    before,
                    "case {case:?}: an untag call must change only the tag map, got {:?}",
                    without_tags(&record)
                );
            }
        }
    }
}

/// A removed key is removed, not hidden: applying it again puts the value back, and
/// removing it again takes it away — over a table of keys of differing shape.
#[tokio::test]
async fn a_removed_key_can_be_applied_again() {
    let router = emulator();
    let case = "re-applied after removal";
    let resource = new_resource(
        &router,
        Kind::ExperimentTemplate,
        case,
        &tag_map(&pairs(&[("Suite", "untag")])),
    )
    .await;

    for (shape, key, value) in [
        ("a plain key", "Env".to_string(), "test"),
        ("a key carrying whitespace", "two words".to_string(), "a b"),
        ("a non-ASCII key", "環境".to_string(), "本番"),
        ("a key at TagKey's ceiling", "k".repeat(TAG_KEY_CEILING), "x"),
    ] {
        let case = format!("{case} / {shape}");
        let applied = tag_resource(
            &router,
            &resource.arn,
            &tag_map(&[(key.clone(), value.to_string())]),
        )
        .await;
        assert_eq!(
            applied.status, 204,
            "case {case:?}: expected HTTP 204 from TagResource, got {} with {:?}",
            applied.status, applied.body
        );
        let carried = tags_of(&read_resource(&router, &resource, &case).await, &case);
        assert_eq!(
            carried.get(&key).map(String::as_str),
            Some(value),
            "case {case:?}: expected the applied tag to be carried, got {carried:?}"
        );

        let removed =
            untag_resource(&router, &resource.arn, Some(std::slice::from_ref(&key))).await;
        assert_empty_success(&removed, &case);
        let left = tags_of(&read_resource(&router, &resource, &case).await, &case);
        assert!(
            !left.contains_key(&key),
            "case {case:?}: expected {key:?} to be gone, got {left:?}"
        );
        assert_eq!(
            left.get("Suite").map(String::as_str),
            Some("untag"),
            "case {case:?}: a key the request did not name keeps its value, got {left:?}"
        );
    }
}

/// An ARN this service cannot address is a bad request, and the live resource is unaffected
/// by having been named wrongly.
#[tokio::test]
async fn an_arn_this_service_cannot_address_is_a_bad_request() {
    let stored = pairs(&[("Suite", "untag"), ("Env", "test")]);

    for (case, arn) in unaddressable_arns() {
        // One emulator per case, so the "nothing was written" assertion is about this
        // case's call alone.
        let router = emulator();
        let resource =
            new_resource(&router, Kind::ExperimentTemplate, case, &tag_map(&stored)).await;

        let answer = untag_resource(&router, &arn, keys(&["Env"]).as_deref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400 for {arn:?}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException for {arn:?}, got {:?}",
            answer.body
        );

        let record = read_resource(&router, &resource, case).await;
        assert_eq!(
            tags_of(&record, case),
            stored.iter().cloned().collect(),
            "case {case:?}: a refused untag call must leave the stored tags alone, got {:?}",
            record["tags"]
        );
    }
}

/// An ARN this service CAN address, naming a resource the store holds no record of, is a
/// miss — the same refusal a tag write or a tag read of that ARN earns.
#[tokio::test]
async fn an_arn_naming_no_record_is_a_miss_and_writes_nothing() {
    let router = emulator();
    let case = "the suite's own template";
    let stored = pairs(&[("Suite", "untag")]);
    let resource = new_resource(&router, Kind::ExperimentTemplate, case, &tag_map(&stored)).await;

    // A template this suite creates and then deletes: the ARN stays well formed, and the
    // record behind it is gone.
    let (deleted_id, deleted_arn) = new_template(&router, "deleted", &json!({})).await;
    let deleted = send(
        &router,
        "DELETE",
        &format!("/experimentTemplates/{deleted_id}"),
        None,
        FALLBACK_REGION,
    )
    .await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteExperimentTemplate, got {} with {:?}",
        deleted.status, deleted.body
    );

    let mut misses = unminted_arns();
    misses.push(("a template that has since been deleted", deleted_arn));

    for (case, arn) in misses {
        for (named, removal) in [
            ("naming a key", keys(&["Env"])),
            ("naming no key", None),
        ] {
            let case = format!("{case} / {named}");
            let answer = untag_resource(&router, &arn, removal.as_deref()).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404 for {arn:?}, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for {arn:?}, got {:?}",
                answer.body
            );

            // A removal and a write of the same ARN agree about the miss.
            let write = tag_resource(&router, &arn, &tag_map(&pairs(&[("Env", "test")]))).await;
            assert_eq!(
                write.status, answer.status,
                "case {case:?}: a tag write of {arn:?} must answer the same status as the \
                 removal, got {} with {:?}",
                write.status, write.body
            );
        }
    }

    // Nothing any of those calls carried reached a record.
    let record = read_resource(&router, &resource, case).await;
    assert_eq!(
        tags_of(&record, case),
        stored.iter().cloned().collect(),
        "case {case:?}: a refused untag call must write nothing, got {:?}",
        record["tags"]
    );
}

/// A `tagKeys` entry outside the bounds `TagKey` publishes is refused, and the stored tags
/// are left alone: `@length(min: 0, max: 128)` with `pattern ^[\s\S]+$`, so a key one
/// character past the ceiling and an empty key are both outside them.
#[tokio::test]
async fn a_tag_key_outside_its_modelled_bounds_is_refused() {
    let stored = pairs(&[("Suite", "untag"), ("Env", "test")]);

    for (case, named) in [
        (
            "a tag key one character past TagKey's ceiling",
            vec!["k".repeat(TAG_KEY_CEILING + 1)],
        ),
        ("an empty tag key", vec![String::new()]),
        (
            "an admissible key alongside an over-long one",
            vec!["Env".to_string(), "k".repeat(TAG_KEY_CEILING + 1)],
        ),
    ] {
        // One emulator per case, so the "nothing was written" assertion is about this
        // case's call alone.
        let router = emulator();
        let resource =
            new_resource(&router, Kind::ExperimentTemplate, case, &tag_map(&stored)).await;

        let answer = untag_resource(&router, &resource.arn, Some(&named)).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );

        let record = read_resource(&router, &resource, case).await;
        assert_eq!(
            tags_of(&record, case),
            stored.iter().cloned().collect(),
            "case {case:?}: a refused untag call must leave the stored tags alone, got {:?}",
            record["tags"]
        );
    }
}
