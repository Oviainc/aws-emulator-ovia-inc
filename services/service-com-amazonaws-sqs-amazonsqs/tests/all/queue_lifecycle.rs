// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the SQS queue LIFECYCLE rules: what `CreateQueue` does with a
//! name that already exists or was just deleted, what `ListQueues` accepts as a page
//! size, which tag keys a queue takes, and what a send must carry — all driven over the
//! wire through the shared harness in `support`.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one example:
//!
//! - re-creating an existing name answers with that queue's url when the request asks
//!   for nothing different about it, and is refused `QueueNameExists` when it does; the
//!   attributes left out of that comparison are the crate's own catalog
//!   ([`ATTRIBUTES_NOT_COMPARED_ON_RECREATE`]), and a tag set the request supplies is
//!   part of it;
//! - "If you delete a queue, you must wait at least 60 seconds before creating a queue
//!   with the same name" — the re-create inside that interval is refused
//!   `QueueDeletedRecently` and creates nothing;
//! - `MaxResults` is accepted only inside the range `ListQueues` publishes for it
//!   ([`LIST_QUEUES_MAX_RESULTS_RANGE`]), at each bound and refused one past it;
//! - a tag key in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace is refused, by
//!   `TagQueue` and by the create's own `tags` member, and the queue keeps its tags;
//! - a send whose `MessageBody` holds no character is refused `MissingParameter` and
//!   enqueues nothing, one entry of a batch at a time.
//!
//! Nothing here reaches into `crate::state` for an assertion: every claim is read back
//! through `GetQueueAttributes`, `GetQueueUrl`, `ListQueueTags` or `ReceiveMessage`.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    ATTRIBUTES_NOT_COMPARED_ON_RECREATE, LIST_QUEUES_MAX_RESULTS_RANGE, POLICY,
    RESERVED_TAG_KEY_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    all_attributes, call, create, create_ok, emulator, error_code, list_tags, message_counts, ok,
};

/// Two different, valid values for one attribute the re-create comparison leaves out,
/// so a row can assert that a DIFFERENT value for it is still idempotent.
///
/// The catalog is the crate's own [`ATTRIBUTES_NOT_COMPARED_ON_RECREATE`]; the test
/// below asserts this function covers every entry of it, so adding an attribute there
/// without a value pair here fails rather than silently going untested.
fn values_for(attribute: &str) -> Option<(String, String)> {
    let policy = |sid: &str| {
        json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Sid": sid,
                "Effect": "Allow",
                "Principal": {"AWS": "arn:aws:iam::210987654321:root"},
                "Action": "sqs:SendMessage",
                "Resource": "*",
            }],
        })
        .to_string()
    };
    match attribute {
        POLICY => Some((policy("first"), policy("second"))),
        _ => None,
    }
}

/// Create the queue `name` with `tags` and return its url.
async fn create_tagged(router: &MultiServiceRouter, name: &str, tags: &Value) -> String {
    let parsed = ok(
        router,
        "CreateQueue",
        json!({"QueueName": name, "tags": tags}),
    )
    .await;
    parsed["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case create {name}: no QueueUrl in {parsed}"))
        .to_string()
}

/// How many messages the queue at `url` holds, available and delayed together.
async fn held(router: &MultiServiceRouter, url: &str) -> i64 {
    let (available, delayed, _in_flight) = message_counts(router, url).await;
    available + delayed
}

/// An attribute the re-create comparison leaves out does not make a re-create conflict:
/// the second call is answered with the existing queue's url whether it names a
/// different value for that attribute, names none at all, or adds one the first did
/// not — and the queue keeps the value it was created with either way.
#[tokio::test]
async fn re_creating_a_name_ignores_the_attributes_outside_the_compared_set() {
    let router = emulator();

    // Every attribute in the catalog has a value pair, so the rows below span it.
    let uncovered: Vec<&str> = ATTRIBUTES_NOT_COMPARED_ON_RECREATE
        .iter()
        .copied()
        .filter(|attribute| values_for(attribute).is_none())
        .collect();
    assert!(
        uncovered.is_empty(),
        "case the catalog of attributes outside the comparison: no value pair for {uncovered:?}"
    );

    for attribute in ATTRIBUTES_NOT_COMPARED_ON_RECREATE {
        let (first, second) = values_for(attribute).expect("every entry has a value pair");
        // (case, what the first create set, what the second create asks for, the value
        // the queue must still carry)
        let rows: Vec<(String, Value, Value, Option<String>)> = vec![
            (
                format!("{attribute} changed by the second create"),
                json!({ *attribute: first.clone() }),
                json!({ *attribute: second.clone() }),
                Some(first.clone()),
            ),
            (
                format!("{attribute} dropped by the second create"),
                json!({ *attribute: first.clone() }),
                json!({}),
                Some(first.clone()),
            ),
            (
                format!("{attribute} added by the second create"),
                json!({}),
                json!({ *attribute: second.clone() }),
                None,
            ),
            (
                format!("{attribute} identical in both creates"),
                json!({ *attribute: first.clone() }),
                json!({ *attribute: first.clone() }),
                Some(first.clone()),
            ),
        ];

        for (index, (case, created_with, re_created_with, still_carried)) in
            rows.iter().enumerate()
        {
            let name = format!("uncompared-{attribute}-{index}");
            let url = create_ok(&router, &name, created_with).await;
            let (status, parsed) = create(&router, &name, re_created_with).await;
            assert_eq!(
                (status, parsed["QueueUrl"].as_str()),
                (200, Some(url.as_str())),
                "case {case}: expected the existing url {url}, got {status} with {parsed}"
            );
            let reported = all_attributes(&router, &url).await;
            assert_eq!(
                reported.get(*attribute),
                still_carried.as_ref(),
                "case {case}: the re-create must leave the stored attribute alone, got {reported:?}"
            );
        }
    }
}

/// A re-create that supplies TAGS is idempotent only for the tag set the queue already
/// carries; any other set is refused as `QueueNameExists`, a re-create that supplies no
/// `tags` member at all asks nothing about them, and the queue's tags never change.
#[tokio::test]
async fn re_creating_a_name_compares_the_tag_set_the_request_supplies() {
    let router = emulator();
    // (case, the tags the create carried, the second create's `tags` member, accepted)
    let rows: &[(&str, Value, Option<Value>, bool)] = &[
        (
            "the same single tag twice",
            json!({"Environment": "production"}),
            Some(json!({"Environment": "production"})),
            true,
        ),
        (
            "the same several tags, in another order",
            json!({"Environment": "production", "Team": "payments"}),
            Some(json!({"Team": "payments", "Environment": "production"})),
            true,
        ),
        (
            "no tags member at all, on a tagged queue",
            json!({"Environment": "production"}),
            None,
            true,
        ),
        (
            "no tags member at all, on an untagged queue",
            json!({}),
            None,
            true,
        ),
        (
            "an empty tag map on an untagged queue",
            json!({}),
            Some(json!({})),
            true,
        ),
        (
            "a different value for the same key",
            json!({"Environment": "production"}),
            Some(json!({"Environment": "staging"})),
            false,
        ),
        (
            "one tag more than the queue carries",
            json!({"Environment": "production"}),
            Some(json!({"Environment": "production", "Team": "payments"})),
            false,
        ),
        (
            "one tag fewer than the queue carries",
            json!({"Environment": "production", "Team": "payments"}),
            Some(json!({"Environment": "production"})),
            false,
        ),
        (
            "a key differing only in case",
            json!({"Environment": "production"}),
            Some(json!({"environment": "production"})),
            false,
        ),
        (
            "an empty tag map on a tagged queue",
            json!({"Environment": "production"}),
            Some(json!({})),
            false,
        ),
        (
            "a tag set on a queue created without one",
            json!({}),
            Some(json!({"Environment": "production"})),
            false,
        ),
    ];

    for (index, (case, created_with, re_created_with, accepted)) in rows.iter().enumerate() {
        let name = format!("recreate-tags-{index}");
        let url = create_tagged(&router, &name, created_with).await;
        let before = list_tags(&router, &url).await;

        let mut body = json!({"QueueName": name});
        if let Some(tags) = re_created_with {
            body["tags"] = tags.clone();
        }
        let (status, parsed) = call(&router, "CreateQueue", &body).await;
        if *accepted {
            assert_eq!(
                (status, parsed["QueueUrl"].as_str()),
                (200, Some(url.as_str())),
                "case {case}: expected the existing url {url}, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "QueueNameExists"),
                "case {case}: expected a 400 QueueNameExists, got {status} with {parsed}"
            );
        }
        assert_eq!(
            list_tags(&router, &url).await,
            before,
            "case {case}: the re-create must leave the queue's tags alone"
        );
    }
}

/// A name whose queue was just deleted cannot be created again: the re-create is refused
/// `QueueDeletedRecently` and makes no queue, while a name that was never used, and one
/// whose queue still exists, are unaffected. Rows differ along the name form and along
/// what the deleted queue held.
#[tokio::test]
async fn a_name_deleted_a_moment_ago_cannot_be_created_again() {
    let router = emulator();
    // (case, the name, the queue's attributes, messages sent before the delete)
    let rows: &[(&str, &str, Value, &[&str])] = &[
        ("a plain name", "deleted-plain", json!({}), &[]),
        (
            "a name holding a hyphen and an underscore",
            "deleted_with-both",
            json!({}),
            &[],
        ),
        (
            "a FIFO queue's name",
            "deleted-fifo.fifo",
            json!({"FifoQueue": "true", "ContentBasedDeduplication": "true"}),
            &[],
        ),
        (
            "a queue that still held messages",
            "deleted-with-messages",
            json!({}),
            &["one", "two"],
        ),
    ];

    for (case, name, attributes, bodies) in rows {
        let url = create_ok(&router, name, attributes).await;
        for body in *bodies {
            let mut send = json!({"QueueUrl": url, "MessageBody": body});
            if attributes.get("FifoQueue").is_some() {
                send["MessageGroupId"] = json!("group");
            }
            let _ = ok(&router, "SendMessage", send).await;
        }
        let _ = ok(&router, "DeleteQueue", json!({"QueueUrl": url})).await;

        let (status, parsed) = create(&router, name, attributes).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "QueueDeletedRecently"),
            "case {case}: expected a 400 QueueDeletedRecently, got {status} with {parsed}"
        );
        // The refused re-create made no queue, so the name is still absent.
        let (status, parsed) = call(&router, "GetQueueUrl", &json!({"QueueName": name})).await;
        assert_eq!(
            (status, error_code(&parsed).as_str()),
            (400, "QueueDoesNotExist"),
            "case {case}: the refused re-create must leave no queue, got {status} with {parsed}"
        );
    }

    // Only the deleted NAME is held back: a name never used is created, and so is one
    // that differs from a deleted one by a single character.
    for name in ["never-deleted", "deleted-plai", "deleted-plain2"] {
        let (status, parsed) = create(&router, name, &json!({})).await;
        assert_eq!(
            status, 200,
            "case a name no delete touched ({name}): expected HTTP 200, got {status} with {parsed}"
        );
    }

    // A queue that exists is still re-creatable (idempotently): the ledger only holds
    // back names a delete removed.
    let url = create_ok(&router, "still-here", &json!({})).await;
    let (status, parsed) = create(&router, "still-here", &json!({})).await;
    assert_eq!(
        (status, parsed["QueueUrl"].as_str()),
        (200, Some(url.as_str())),
        "case a queue that still exists: expected its url, got {status} with {parsed}"
    );
}

/// `ListQueues` accepts a `MaxResults` at each published bound and refuses one outside
/// the range, rather than quietly paging at a size the caller did not ask for.
#[tokio::test]
async fn list_queues_accepts_a_page_size_only_inside_the_published_range() {
    let router = emulator();
    for name in ["page-a", "page-b"] {
        let _ = create_ok(&router, name, &json!({})).await;
    }
    let (min, max) = LIST_QUEUES_MAX_RESULTS_RANGE;
    // The bounds are `ListQueuesRequest$MaxResults`'s own: "Value range is 1 to 1000."
    let rows: Vec<(String, Value, bool)> = vec![
        ("no MaxResults at all".to_string(), json!({}), true),
        (
            format!("MaxResults at the published lower bound of {min}"),
            json!({"MaxResults": min}),
            true,
        ),
        (
            format!("MaxResults at the published upper limit of {max}"),
            json!({"MaxResults": max}),
            true,
        ),
        (
            format!("MaxResults of {}, one below the lower bound", min - 1),
            json!({"MaxResults": min - 1}),
            false,
        ),
        (
            "a negative MaxResults".to_string(),
            json!({"MaxResults": -1}),
            false,
        ),
        (
            format!("MaxResults of {}, one past the upper limit", max + 1),
            json!({"MaxResults": max + 1}),
            false,
        ),
    ];

    for (case, body, accepted) in &rows {
        let (status, parsed) = call(&router, "ListQueues", body).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {parsed}"
            );
            assert!(
                parsed["QueueUrls"].as_array().is_some_and(|urls| !urls.is_empty()),
                "case {case}: expected the listing to answer with urls, got {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidParameterValue"),
                "case {case}: expected a 400 InvalidParameterValue, got {status} with {parsed}"
            );
            assert!(
                parsed.get("QueueUrls").is_none(),
                "case {case}: a refused listing must answer with no urls, got {parsed}"
            );
        }
    }
}

/// A tag key in the reserved prefix is refused — by `TagQueue` and by the create's own
/// `tags` member — and the queue keeps the tags it had; a key that merely holds the
/// prefix somewhere other than the front is an ordinary key.
#[tokio::test]
async fn a_tag_key_in_the_reserved_prefix_is_refused() {
    let router = emulator();
    let reserved = RESERVED_TAG_KEY_PREFIX;
    // The prefix spelled with its first letter capitalised, which is still the reserved
    // prefix: the reservation is on the prefix, not on one spelling of it.
    let mixed_case = format!("{}{}", reserved[..1].to_uppercase(), &reserved[1..]);
    // (case, the key, accepted)
    let rows: Vec<(String, String, bool)> = vec![
        (
            "the reserved prefix with a name after it".to_string(),
            format!("{reserved}cost-center"),
            false,
        ),
        (
            "the reserved prefix in upper case".to_string(),
            reserved.to_uppercase(),
            false,
        ),
        (
            "the reserved prefix in mixed case".to_string(),
            format!("{mixed_case}team"),
            false,
        ),
        (
            "the reserved prefix on its own".to_string(),
            reserved.to_string(),
            false,
        ),
        (
            "the same letters without the colon".to_string(),
            reserved.trim_end_matches(':').to_string(),
            true,
        ),
        (
            "the reserved prefix not at the front".to_string(),
            format!("mine:{reserved}cost"),
            true,
        ),
        ("an ordinary key".to_string(), "CostCenter".to_string(), true),
    ];

    for (index, (case, key, accepted)) in rows.iter().enumerate() {
        let name = format!("reserved-tag-{index}");
        let url = create_tagged(&router, &name, &json!({"Owner": "platform"})).await;
        let before = list_tags(&router, &url).await;

        let (status, parsed) = call(
            &router,
            "TagQueue",
            &json!({"QueueUrl": url, "Tags": {key.clone(): "v"}}),
        )
        .await;
        let mut expected = before.clone();
        if *accepted {
            assert_eq!(
                status, 200,
                "case TagQueue with {case} ({key}): expected HTTP 200, got {status} with {parsed}"
            );
            expected.insert(key.clone(), "v".to_string());
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidParameterValue"),
                "case TagQueue with {case} ({key}): expected a 400 InvalidParameterValue, got {status} with {parsed}"
            );
        }
        assert_eq!(
            list_tags(&router, &url).await,
            expected,
            "case TagQueue with {case} ({key}): the queue must hold exactly the tags the rule allows"
        );

        // The same key at CREATE time, where the create's `tags` member is bounded by
        // exactly the same rules.
        let created = format!("reserved-create-{index}");
        let (status, parsed) = call(
            &router,
            "CreateQueue",
            &json!({"QueueName": created, "tags": {key.clone(): "v"}}),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case CreateQueue with {case} ({key}): expected HTTP 200, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "InvalidParameterValue"),
                "case CreateQueue with {case} ({key}): expected a 400 InvalidParameterValue, got {status} with {parsed}"
            );
            let (status, parsed) =
                call(&router, "GetQueueUrl", &json!({"QueueName": created})).await;
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "QueueDoesNotExist"),
                "case CreateQueue with {case} ({key}): the refused create must leave no queue, got {status} with {parsed}"
            );
        }
    }
}

/// A `MessageBody` holding no character is the required parameter not supplied:
/// `SendMessage` refuses it as `MissingParameter` and enqueues nothing, while every body
/// that holds at least one character is taken.
#[tokio::test]
async fn a_send_with_an_empty_message_body_is_refused() {
    let router = emulator();
    // (case, the body, accepted)
    let rows: &[(&str, &str, bool)] = &[
        ("an empty body", "", false),
        ("a single space", " ", true),
        ("a single character", "x", true),
        ("a tab, the shortest allowed control character", "\t", true),
        ("a non-ASCII character", "\u{1F600}", true),
        ("an ordinary payload", "{\"order\":1}", true),
    ];

    let url = create_ok(&router, "empty-body", &json!({})).await;
    let mut enqueued = 0i64;
    for (case, body, accepted) in rows {
        let (status, parsed) = call(
            &router,
            "SendMessage",
            &json!({"QueueUrl": url, "MessageBody": body}),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} with {parsed}"
            );
            enqueued += 1;
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str()),
                (400, "MissingParameter"),
                "case {case}: expected a 400 MissingParameter, got {status} with {parsed}"
            );
            assert_eq!(
                parsed["message"].as_str(),
                Some("The request must contain the parameter MessageBody."),
                "case {case}: expected the refusal to name the parameter, got {parsed}"
            );
        }
        assert_eq!(
            held(&router, &url).await,
            enqueued,
            "case {case}: the queue must hold exactly the accepted sends"
        );
    }
}

/// In a batch the same rule is applied per ENTRY: the entry with no body fails alone,
/// under its own `Id`, and its siblings are still enqueued.
#[tokio::test]
async fn a_batch_entry_with_an_empty_message_body_fails_alone() {
    let router = emulator();
    let url = create_ok(&router, "empty-body-batch", &json!({})).await;
    let answer = ok(
        &router,
        "SendMessageBatch",
        json!({
            "QueueUrl": url,
            "Entries": [
                {"Id": "first", "MessageBody": "one"},
                {"Id": "empty", "MessageBody": ""},
                {"Id": "last", "MessageBody": "two"},
            ],
        }),
    )
    .await;

    let successful: Vec<String> = answer["Successful"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry["Id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        successful,
        vec!["first".to_string(), "last".to_string()],
        "case a batch holding one entry with no body: expected the other entries accepted, got {answer}"
    );
    let failed = answer["Failed"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        failed.len(),
        1,
        "case a batch holding one entry with no body: expected exactly one failure, got {answer}"
    );
    assert_eq!(
        (
            failed[0]["Id"].as_str(),
            failed[0]["Code"].as_str(),
            failed[0]["SenderFault"].as_bool(),
        ),
        (Some("empty"), Some("MissingParameter"), Some(true)),
        "case a batch holding one entry with no body: expected the sender's MissingParameter for that entry, got {answer}"
    );
    assert_eq!(
        held(&router, &url).await,
        2,
        "case a batch holding one entry with no body: only the entries with a body may be held"
    );
}
