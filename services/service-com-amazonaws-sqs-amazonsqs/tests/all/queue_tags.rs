// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SQS queue-tag slice: `TagQueue`, `UntagQueue` and
//! `ListQueueTags`, driven over the wire through the shared harness in `support`.
//!
//! The rule under test: a queue carries ONE tag map, `TagQueue` UPSERTS into it and
//! `UntagQueue` removes exactly the keys it names —
//! "A new tag with a key identical to that of an existing tag overwrites the existing
//! tag", every key the call does not name keeps the value it had, "Tags are
//! case-sensitive" and "Amazon SQS interprets tags as character strings" with no
//! semantic meaning, so keys and values round-trip verbatim through `ListQueueTags`.
//!
//! Nothing here reaches into `crate::state`: a tag is only believed once
//! `ListQueueTags` reports it back. The tables span what the rule generalises over —
//! the tag count (none, one, many), where the tag came from (the create's `tags`, one
//! `TagQueue`, a sequence of them), and the value forms SQS accepts as character
//! strings (plain, empty, spaced, punctuated, non-ASCII, long) — rather than one
//! recorded example.
//!
//! The bound rows are generated from the constants in
//! [`service_com_amazonaws_sqs_amazonsqs::state`], which carry the tag restriction each
//! one comes from, so every published bound is exercised AT the bound and one past it.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    MAX_TAGS_PER_QUEUE, MAX_TAG_KEY_LENGTH, MAX_TAG_VALUE_LENGTH,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    account, all_attributes, as_map, call, create_ok, emulator, error_code, ok, HOST,
};

/// The tags `ListQueueTags` reports for the queue at `url`, as a sorted map. A queue
/// with no tag reports no `Tags` member at all, which is the empty map here.
async fn list_tags(router: &MultiServiceRouter, url: &str) -> BTreeMap<String, String> {
    let reported = ok(router, "ListQueueTags", json!({"QueueUrl": url})).await;
    as_map(reported.get("Tags").unwrap_or(&Value::Null))
}

/// Whether `ListQueueTags` reported a `Tags` member at all for the queue at `url`.
async fn reports_a_tags_member(router: &MultiServiceRouter, url: &str) -> bool {
    let reported = ok(router, "ListQueueTags", json!({"QueueUrl": url})).await;
    reported.get("Tags").is_some_and(|tags| !tags.is_null())
}

/// A `Value` object of string pairs as a sorted map.
fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// `count` distinct tags, as a map: the filler the count-bound rows are built from.
fn generated_tags(count: usize, prefix: &str) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("{prefix}{index:03}"), format!("value-{index}")))
        .collect()
}

/// A tag map as the request body's `Tags`/`tags` member.
fn as_value(tags: &BTreeMap<String, String>) -> Value {
    serde_json::to_value(tags).expect("a tag map serializes")
}

/// One row of the tag table: a queue created with `created_with` tags, then a SEQUENCE
/// of `TagQueue` calls, and the tag map every later `ListQueueTags` must report.
struct TagRow {
    case: &'static str,
    name: &'static str,
    created_with: Value,
    tag_calls: Vec<Value>,
    expected: Vec<(&'static str, &'static str)>,
}

/// `TagQueue` upserts into the queue's one tag map, and `ListQueueTags` reports exactly
/// that map back.
#[tokio::test]
async fn tag_queue_upserts_tags_and_list_queue_tags_reports_them() {
    let router = emulator();
    // A value longer than any row would otherwise carry, to show a value is stored
    // verbatim however long it is: SQS "interprets tags as character strings".
    let long_value = "v".repeat(256);
    let rows = vec![
        TagRow {
            case: "one tag added to an untagged queue",
            name: "tags-one",
            created_with: json!({}),
            tag_calls: vec![json!({"Environment": "production"})],
            expected: vec![("Environment", "production")],
        },
        TagRow {
            case: "several tags in one call",
            name: "tags-many",
            created_with: json!({}),
            tag_calls: vec![json!({
                "Environment": "staging",
                "Team": "payments",
                "CostCenter": "4711",
            })],
            expected: vec![
                ("CostCenter", "4711"),
                ("Environment", "staging"),
                ("Team", "payments"),
            ],
        },
        TagRow {
            case: "a tag the create carried is kept, and merged with a later one",
            name: "tags-created-with",
            created_with: json!({"Owner": "platform"}),
            tag_calls: vec![json!({"Environment": "production"})],
            expected: vec![("Environment", "production"), ("Owner", "platform")],
        },
        TagRow {
            case: "an identical key overwrites the existing tag",
            name: "tags-overwrite",
            created_with: json!({"Environment": "staging"}),
            tag_calls: vec![
                json!({"Environment": "production", "Team": "payments"}),
                json!({"Environment": "retired"}),
            ],
            expected: vec![("Environment", "retired"), ("Team", "payments")],
        },
        TagRow {
            case: "tags are case-sensitive, so keys differing only in case are distinct",
            name: "tags-case",
            created_with: json!({}),
            tag_calls: vec![json!({"Environment": "upper", "environment": "lower"})],
            expected: vec![("Environment", "upper"), ("environment", "lower")],
        },
        TagRow {
            case: "a sequence of single-tag calls accumulates",
            name: "tags-sequence",
            created_with: json!({}),
            tag_calls: vec![
                json!({"A": "1"}),
                json!({"B": "2"}),
                json!({"C": "3"}),
                json!({"D": "4"}),
            ],
            expected: vec![("A", "1"), ("B", "2"), ("C", "3"), ("D", "4")],
        },
        TagRow {
            case: "values are character strings: empty, spaced, punctuated, non-ASCII",
            name: "tags-value-forms",
            created_with: json!({}),
            tag_calls: vec![json!({
                "Empty": "",
                "Spaced": "two words",
                "Punctuated": "a/b:c_d-e.f+g=h@i",
                "NonAscii": "München — 東京 🚚",
            })],
            expected: vec![
                ("Empty", ""),
                ("NonAscii", "München — 東京 🚚"),
                ("Punctuated", "a/b:c_d-e.f+g=h@i"),
                ("Spaced", "two words"),
            ],
        },
    ];

    for row in &rows {
        let mut body = json!({"QueueName": row.name});
        if row
            .created_with
            .as_object()
            .is_some_and(|map| !map.is_empty())
        {
            body["tags"] = row.created_with.clone();
        }
        let created = ok(&router, "CreateQueue", body).await;
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: no QueueUrl in {created}", row.case))
            .to_string();

        // Before any TagQueue, the queue reports exactly what the create carried.
        assert_eq!(
            list_tags(&router, &url).await,
            as_map(&row.created_with),
            "case {}: before tagging, expected the create's tags {}, got a different map",
            row.case,
            row.created_with
        );

        for tags in &row.tag_calls {
            let (status, parsed) =
                call(&router, "TagQueue", &json!({"QueueUrl": url, "Tags": tags})).await;
            assert_eq!(
                status, 200,
                "case {}: TagQueue with {tags} expected HTTP 200, got {status} with {parsed}",
                row.case
            );
        }

        let mut reported = expected(&row.expected);
        assert_eq!(
            list_tags(&router, &url).await,
            reported,
            "case {}: ListQueueTags reported the wrong tag map",
            row.case
        );

        // The same map is reported again, and a long value survives the round trip
        // unchanged — so does every key already there.
        let (status, parsed) = call(
            &router,
            "TagQueue",
            &json!({"QueueUrl": url, "Tags": {"Long": long_value.clone()}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {}: TagQueue with a long value expected HTTP 200, got {status} with {parsed}",
            row.case
        );
        reported.insert("Long".to_string(), long_value.clone());
        assert_eq!(
            list_tags(&router, &url).await,
            reported,
            "case {}: a long value must round-trip beside every earlier tag",
            row.case
        );
    }
}

/// Tagging a queue changes that queue only: another queue's tags, and this queue's
/// reported attributes, are untouched.
#[tokio::test]
async fn tag_queue_changes_only_the_queue_it_names() {
    let router = emulator();
    let tagged = create_ok(&router, "tagged", &json!({"VisibilityTimeout": "45"})).await;
    let untouched = create_ok(&router, "untouched", &json!({})).await;

    let attributes_before = all_attributes(&router, &tagged).await;
    let _ = ok(
        &router,
        "TagQueue",
        json!({"QueueUrl": tagged, "Tags": {"Environment": "production"}}),
    )
    .await;

    assert_eq!(
        list_tags(&router, &tagged).await,
        expected(&[("Environment", "production")]),
        "case the tagged queue: expected the tag it was given"
    );
    assert_eq!(
        list_tags(&router, &untouched).await,
        BTreeMap::new(),
        "case the other queue: tagging one queue must not tag another"
    );

    // A tag is not a queue attribute, so no attribute `GetQueueAttributes` reports
    // changes — including the two timestamps.
    let attributes_after = all_attributes(&router, &tagged).await;
    assert_eq!(
        attributes_after, attributes_before,
        "case the tagged queue's attributes: tagging must not change the reported attribute map"
    );
}

/// A queue with no tag reports no `Tags` member at all, and a listing never changes
/// what a later caller sees.
#[tokio::test]
async fn list_queue_tags_omits_tags_for_an_untagged_queue_and_mutates_nothing() {
    let router = emulator();
    let url = create_ok(&router, "reporting", &json!({})).await;

    assert!(
        !reports_a_tags_member(&router, &url).await,
        "case an untagged queue: expected no Tags member at all"
    );

    let _ = ok(
        &router,
        "TagQueue",
        json!({"QueueUrl": url, "Tags": {"Team": "payments"}}),
    )
    .await;

    // Repeated reads are identical, and the tag survives every one of them.
    let first = list_tags(&router, &url).await;
    let second = list_tags(&router, &url).await;
    let third = list_tags(&router, &url).await;
    assert_eq!(
        (&first, &second, &third),
        (
            &expected(&[("Team", "payments")]),
            &expected(&[("Team", "payments")]),
            &expected(&[("Team", "payments")])
        ),
        "case three identical reads: a listing must not change what a later caller sees"
    );
    assert!(
        reports_a_tags_member(&router, &url).await,
        "case a tagged queue: expected a Tags member, got none"
    );
}

/// Both operations refuse a `QueueUrl` no queue answers to as `QueueDoesNotExist`,
/// over every form of url that resolves to nothing.
#[tokio::test]
async fn a_queue_url_no_queue_answers_to_is_refused_as_queue_does_not_exist() {
    let router = emulator();
    let _ = create_ok(&router, "existing", &json!({})).await;

    let cases: Vec<(&str, String)> = vec![
        (
            "a name that was never created",
            format!("https://{HOST}/{}/never-created", account()),
        ),
        (
            "another account's queue of the same name",
            format!("https://{HOST}/210987654321/existing"),
        ),
        ("a value that is not a url at all", "existing".to_string()),
        (
            "a url with no account segment",
            format!("https://{HOST}/existing"),
        ),
    ];

    for (case, bad) in &cases {
        let (status, parsed) = call(
            &router,
            "TagQueue",
            &json!({"QueueUrl": bad, "Tags": {"Environment": "production"}}),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "QueueDoesNotExist"),
            "case TagQueue on {case} ({bad}): expected a 400 QueueDoesNotExist, got {status} with {parsed}"
        );

        let (status, parsed) = call(&router, "ListQueueTags", &json!({"QueueUrl": bad})).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "QueueDoesNotExist"),
            "case ListQueueTags on {case} ({bad}): expected a 400 QueueDoesNotExist, got {status} with {parsed}"
        );
    }

    // The queue that does exist is reachable through any endpoint this emulator
    // answers on, and carries no tag from any refusal above.
    let through_another_endpoint = format!("http://127.0.0.1:9297/{}/existing", account());
    assert_eq!(
        list_tags(&router, &through_another_endpoint).await,
        BTreeMap::new(),
        "case a url built against another endpoint: expected the same, still untagged queue"
    );
}

/// One row of the bound table: the tags the queue already carries, the tags one
/// `TagQueue` then supplies, and whether the published tag restrictions accept it.
struct BoundRow {
    case: String,
    existing: BTreeMap<String, String>,
    supplied: BTreeMap<String, String>,
    accepted: bool,
}

/// `TagQueue` accepts a tag map up to every published tag restriction and refuses one
/// past it, counting what the call would LEAVE the queue with; a refused call stores
/// nothing.
#[tokio::test]
async fn tag_queue_enforces_the_published_tag_restrictions() {
    let router = emulator();
    let at_the_bound = generated_tags(MAX_TAGS_PER_QUEUE, "k");
    let rows = vec![
        BoundRow {
            case: format!("a key of {MAX_TAG_KEY_LENGTH} characters"),
            existing: BTreeMap::new(),
            supplied: [("k".repeat(MAX_TAG_KEY_LENGTH), "v".to_string())]
                .into_iter()
                .collect(),
            accepted: true,
        },
        BoundRow {
            case: format!("a key of {} characters", MAX_TAG_KEY_LENGTH + 1),
            existing: BTreeMap::new(),
            supplied: [("k".repeat(MAX_TAG_KEY_LENGTH + 1), "v".to_string())]
                .into_iter()
                .collect(),
            accepted: false,
        },
        BoundRow {
            case: "an empty key".to_string(),
            existing: BTreeMap::new(),
            supplied: [(String::new(), "v".to_string())].into_iter().collect(),
            accepted: false,
        },
        BoundRow {
            case: format!("a value of {MAX_TAG_VALUE_LENGTH} characters"),
            existing: BTreeMap::new(),
            supplied: [("Key".to_string(), "v".repeat(MAX_TAG_VALUE_LENGTH))]
                .into_iter()
                .collect(),
            accepted: true,
        },
        BoundRow {
            case: format!("a value of {} characters", MAX_TAG_VALUE_LENGTH + 1),
            existing: BTreeMap::new(),
            supplied: [("Key".to_string(), "v".repeat(MAX_TAG_VALUE_LENGTH + 1))]
                .into_iter()
                .collect(),
            accepted: false,
        },
        BoundRow {
            case: format!("{MAX_TAGS_PER_QUEUE} tags in one call"),
            existing: BTreeMap::new(),
            supplied: at_the_bound.clone(),
            accepted: true,
        },
        BoundRow {
            case: format!("{} tags in one call", MAX_TAGS_PER_QUEUE + 1),
            existing: BTreeMap::new(),
            supplied: generated_tags(MAX_TAGS_PER_QUEUE + 1, "k"),
            accepted: false,
        },
        BoundRow {
            case: format!("a call that fills the queue to {MAX_TAGS_PER_QUEUE} tags"),
            existing: generated_tags(MAX_TAGS_PER_QUEUE - 10, "k"),
            supplied: generated_tags(10, "later"),
            accepted: true,
        },
        BoundRow {
            case: format!("one new tag on a queue already carrying {MAX_TAGS_PER_QUEUE}"),
            existing: at_the_bound.clone(),
            supplied: [("OneMore".to_string(), "v".to_string())]
                .into_iter()
                .collect(),
            accepted: false,
        },
        BoundRow {
            case: format!(
                "overwriting an existing key on a queue carrying {MAX_TAGS_PER_QUEUE} tags"
            ),
            existing: at_the_bound.clone(),
            supplied: [("k000".to_string(), "overwritten".to_string())]
                .into_iter()
                .collect(),
            accepted: true,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let url = create_ok(&router, &format!("bounds-{index}"), &json!({})).await;
        if !row.existing.is_empty() {
            let _ = ok(
                &router,
                "TagQueue",
                json!({"QueueUrl": url, "Tags": as_value(&row.existing)}),
            )
            .await;
        }
        let before = list_tags(&router, &url).await;
        assert_eq!(
            before, row.existing,
            "case {}: the queue did not start from the tags the row names",
            row.case
        );

        let (status, parsed) = call(
            &router,
            "TagQueue",
            &json!({"QueueUrl": url, "Tags": as_value(&row.supplied)}),
        )
        .await;
        if row.accepted {
            assert_eq!(
                status, 200,
                "case {}: expected HTTP 200, got {status} with {parsed}",
                row.case
            );
            let mut want = before.clone();
            want.extend(row.supplied.clone());
            assert_eq!(
                list_tags(&router, &url).await,
                want,
                "case {}: the accepted call must leave exactly the merged tag map",
                row.case
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {}: expected a 400 ValidationException, got {status} with {parsed}",
                row.case
            );
            assert_eq!(
                list_tags(&router, &url).await,
                before,
                "case {}: a refused call must store nothing",
                row.case
            );
        }
    }
}

/// The create's `tags` member is bounded by exactly the same restrictions, so the cap
/// cannot be bypassed at create time; a refused create makes no queue.
#[tokio::test]
async fn create_queue_enforces_the_same_tag_restrictions() {
    let router = emulator();
    let rows: Vec<(String, BTreeMap<String, String>, bool)> = vec![
        (
            format!("{MAX_TAGS_PER_QUEUE} tags at create"),
            generated_tags(MAX_TAGS_PER_QUEUE, "k"),
            true,
        ),
        (
            format!("{} tags at create", MAX_TAGS_PER_QUEUE + 1),
            generated_tags(MAX_TAGS_PER_QUEUE + 1, "k"),
            false,
        ),
        (
            format!("a value of {} characters at create", MAX_TAG_VALUE_LENGTH + 1),
            [("Key".to_string(), "v".repeat(MAX_TAG_VALUE_LENGTH + 1))]
                .into_iter()
                .collect(),
            false,
        ),
        (
            format!("a key of {} characters at create", MAX_TAG_KEY_LENGTH + 1),
            [("k".repeat(MAX_TAG_KEY_LENGTH + 1), "v".to_string())]
                .into_iter()
                .collect(),
            false,
        ),
    ];

    for (index, (case, tags, accepted)) in rows.iter().enumerate() {
        let name = format!("create-tags-{index}");
        let (status, parsed) = call(
            &router,
            "CreateQueue",
            &json!({"QueueName": name, "tags": as_value(tags)}),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {parsed}"
            );
            let url = parsed["QueueUrl"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                list_tags(&router, &url).await,
                *tags,
                "case {case}: the create must store exactly the tags it carried"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "ValidationException"),
                "case {case}: expected a 400 ValidationException, got {status} with {parsed}"
            );
            // A refused create made no queue at all.
            let (status, parsed) = call(&router, "GetQueueUrl", &json!({"QueueName": name})).await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "QueueDoesNotExist"),
                "case {case}: the refused create must leave no queue, got {status} with {parsed}"
            );
        }
    }
}

/// One row of the untag table: the tags the queue starts from, a SEQUENCE of `UntagQueue`
/// calls, and the tag map every later `ListQueueTags` must report.
struct UntagRow {
    case: &'static str,
    existing: &'static [(&'static str, &'static str)],
    removals: &'static [&'static [&'static str]],
    remaining: &'static [(&'static str, &'static str)],
}

/// `UntagQueue` removes exactly the keys it names from the queue's one tag map, and
/// nothing else: a key the queue does not carry leaves the map as it is (so a repeated
/// request is the same request), and "Tags are case-sensitive", so a key differing only in
/// case names a different tag.
///
/// The rows span what the rule generalises over: how many keys one call names (one,
/// several, every one the queue has), whether each names a tag the queue carries, and
/// whether the keys repeat a call already made.
#[tokio::test]
async fn untag_queue_removes_exactly_the_keys_it_names() {
    let router = emulator();
    let rows: &[UntagRow] = &[
        UntagRow {
            case: "one key of several",
            existing: &[("Environment", "production"), ("Team", "payments")],
            removals: &[&["Environment"]],
            remaining: &[("Team", "payments")],
        },
        UntagRow {
            case: "several keys in one call",
            existing: &[("A", "1"), ("B", "2"), ("C", "3"), ("D", "4")],
            removals: &[&["A", "C"]],
            remaining: &[("B", "2"), ("D", "4")],
        },
        UntagRow {
            case: "every key the queue carries",
            existing: &[("Only", "tag")],
            removals: &[&["Only"]],
            remaining: &[],
        },
        UntagRow {
            case: "a key the queue does not carry leaves the map alone",
            existing: &[("Environment", "production")],
            removals: &[&["Absent"]],
            remaining: &[("Environment", "production")],
        },
        UntagRow {
            case: "the same keys removed twice is the same request",
            existing: &[("Environment", "production"), ("Team", "payments")],
            removals: &[&["Team"], &["Team"], &["Team"]],
            remaining: &[("Environment", "production")],
        },
        UntagRow {
            case: "keys are case-sensitive, so only the key named goes",
            existing: &[("Environment", "upper"), ("environment", "lower")],
            removals: &[&["environment"]],
            remaining: &[("Environment", "upper")],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let url = create_ok(&router, &format!("untag-{index}"), &json!({})).await;
        let _ = ok(
            &router,
            "TagQueue",
            json!({"QueueUrl": url, "Tags": as_value(&expected(row.existing))}),
        )
        .await;
        assert_eq!(
            list_tags(&router, &url).await,
            expected(row.existing),
            "case {case}: the queue did not start from the tags the row names"
        );

        for keys in row.removals {
            let (status, parsed) = call(
                &router,
                "UntagQueue",
                &json!({"QueueUrl": url, "TagKeys": keys}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: UntagQueue {keys:?} expected HTTP 200, got {status} with {parsed}"
            );
        }

        assert_eq!(
            list_tags(&router, &url).await,
            expected(row.remaining),
            "case {case}: UntagQueue left the wrong tag map"
        );
        // A queue left with no tag reports no `Tags` member at all, exactly as one that
        // was never tagged does.
        assert_eq!(
            reports_a_tags_member(&router, &url).await,
            !row.remaining.is_empty(),
            "case {case}: whether a Tags member is reported must follow the tag map"
        );
    }
}

/// Removing tags changes that queue only, and is not an attribute change: another queue's
/// tags and this queue's reported attribute map are untouched.
#[tokio::test]
async fn untag_queue_changes_only_the_queue_it_names() {
    let router = emulator();
    let untagged = create_ok(&router, "untag-target", &json!({"VisibilityTimeout": "45"})).await;
    let other = create_ok(&router, "untag-other", &json!({})).await;
    for url in [&untagged, &other] {
        let _ = ok(
            &router,
            "TagQueue",
            json!({"QueueUrl": url, "Tags": {"Environment": "production", "Team": "payments"}}),
        )
        .await;
    }
    let attributes_before = all_attributes(&router, &untagged).await;

    let _ = ok(
        &router,
        "UntagQueue",
        json!({"QueueUrl": untagged, "TagKeys": ["Environment"]}),
    )
    .await;

    assert_eq!(
        list_tags(&router, &untagged).await,
        expected(&[("Team", "payments")]),
        "case the untagged queue: expected only the key named to go"
    );
    assert_eq!(
        list_tags(&router, &other).await,
        expected(&[("Environment", "production"), ("Team", "payments")]),
        "case the other queue: untagging one queue must not untag another"
    );
    assert_eq!(
        all_attributes(&router, &untagged).await,
        attributes_before,
        "case the untagged queue's attributes: untagging must not change the reported attribute map"
    );
}

/// A `TagKeys` list that names no tag removes nothing: refused as a 400
/// `ValidationException`, exactly as an empty `Tags` map is, and the queue keeps its tags.
#[tokio::test]
async fn untag_queue_refuses_a_tag_keys_list_that_names_no_tag() {
    let router = emulator();
    let url = create_ok(&router, "empty-tag-keys", &json!({})).await;
    let _ = ok(
        &router,
        "TagQueue",
        json!({"QueueUrl": url, "Tags": {"Owner": "platform"}}),
    )
    .await;

    let (status, parsed) = call(
        &router,
        "UntagQueue",
        &json!({"QueueUrl": url, "TagKeys": []}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case an empty TagKeys list: expected a 400 ValidationException, got {status} with {parsed}"
    );
    assert_eq!(
        list_tags(&router, &url).await,
        expected(&[("Owner", "platform")]),
        "case an empty TagKeys list: the refusal must leave the queue's tags alone"
    );
}

/// A `Tags` map that names no tag adds nothing: refused as a 400 `ValidationException`,
/// exactly as an empty `Attributes` map is, and the queue keeps the tags it had.
#[tokio::test]
async fn tag_queue_refuses_a_tags_map_that_names_no_tag() {
    let router = emulator();
    let url = create_ok(&router, "empty-tags", &json!({})).await;
    let _ = ok(
        &router,
        "TagQueue",
        json!({"QueueUrl": url, "Tags": {"Owner": "platform"}}),
    )
    .await;

    let (status, parsed) = call(
        &router,
        "TagQueue",
        &json!({"QueueUrl": url, "Tags": {}}),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "ValidationException"),
        "case an empty Tags map: expected a 400 ValidationException, got {status} with {parsed}"
    );
    assert_eq!(
        list_tags(&router, &url).await,
        expected(&[("Owner", "platform")]),
        "case an empty Tags map: the refusal must leave the queue's tags alone"
    );
}
