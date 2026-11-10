// SPDX-License-Identifier: CC0-1.0

//! The Directory Service event-topic associations, driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body. The
//! harness (`service`, `call`, `ok`, `refused`, `create_directory`) is the one
//! `directory_lifecycle.rs` already defines, so nothing here reaches into the state
//! struct and a response that is not computed from persisted state cannot pass.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call:
//! `RegisterEventTopic` then `DescribeEventTopics` over a table of directories whose
//! rows differ in how many topics are associated and in the form and length of the
//! topic names (including both published `TopicName` length bounds and one past the
//! upper one); the `TopicArn` Region over two signing Regions; the narrowing the
//! `DirectoryId` and `TopicNames` filters perform over a table of filters; and the
//! refusal of a directory that does not exist over a table of absent ids.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{
    call, call_in_region, create_directory, minimal_create, ok, refused, service, REGION,
};

/// The `TopicStatus` a registration that holds reports: the first value of the modelled
/// `TopicStatus` enum (`smithy_query(query="shape",shape="TopicStatus")` lists
/// `Registered`, `Topic not found`, `Failed`, `Deleted`).
const STATUS_REGISTERED: &str = "Registered";

/// The published bounds of the modelled `TopicName` shape: `length 1..256`.
const TOPIC_NAME_MIN_LEN: usize = 1;
const TOPIC_NAME_MAX_LEN: usize = 256;

/// This emulator's documented per-directory association guard
/// (`state::MAX_EVENT_TOPICS_PER_DIRECTORY`).
const TOPIC_GUARD: usize = 50;

/// This emulator's documented `TopicNames` filter guard
/// (`state::MAX_TOPIC_NAMES_PER_REQUEST`).
const TOPIC_NAMES_GUARD: usize = 100;

/// `RegisterEventTopic` for `topic_name`, asserting it was accepted.
async fn register(router: &MultiServiceRouter, directory_id: &str, topic_name: &str, case: &str) {
    ok(
        router,
        "RegisterEventTopic",
        json!({ "DirectoryId": directory_id, "TopicName": topic_name }),
        case,
    )
    .await;
}

/// The `EventTopics` list of a `DescribeEventTopics` answer.
fn event_topics(body: &Value, case: &str) -> Vec<Value> {
    body["EventTopics"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected EventTopics, got {body}"))
        .clone()
}

/// `DescribeEventTopics` with `body`, returning its `EventTopics` list.
async fn describe(router: &MultiServiceRouter, body: Value, case: &str) -> Vec<Value> {
    let answer = ok(router, "DescribeEventTopics", body, case).await;
    event_topics(&answer, case)
}

/// The `(DirectoryId, TopicName)` pairs a `DescribeEventTopics` answer reports, as a
/// set: the association identity, independent of the order the list came back in.
fn reported_pairs(topics: &[Value]) -> BTreeSet<(String, String)> {
    topics
        .iter()
        .map(|topic| {
            (
                topic["DirectoryId"].as_str().unwrap_or_default().to_string(),
                topic["TopicName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The topic names a `DescribeEventTopics` answer reports, as a set.
fn reported_names(topics: &[Value]) -> BTreeSet<String> {
    topics
        .iter()
        .map(|topic| topic["TopicName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Seconds since the epoch, the unit an `awsJson1_1` timestamp is rendered in.
///
/// Shared with `tests/all/directory_networking_and_settings.rs`, which compares a
/// seeded instant with what the wire carries the same way.
pub(crate) fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One row of the filter table: a `DescribeEventTopics` request and the associations
/// it must narrow the account's to.
struct FilterCase {
    case: &'static str,
    request: Value,
    expected: BTreeSet<(String, String)>,
}

/// One row of the association table: a directory and the topic names registered on it.
struct TopicCase {
    name: &'static str,
    directory: &'static str,
    topics: Vec<String>,
}

#[tokio::test]
async fn registered_event_topics_are_reported_from_persisted_state() {
    let router = service();

    // The rows differ along everything a registration varies in: how many topics a
    // directory publishes to (none, one, several) and the form and length of the topic
    // names the modelled `TopicName` admits — letters, digits, underscores and hyphens,
    // at both published length bounds.
    let cases = vec![
        TopicCase {
            name: "no association",
            directory: "quiet.example.com",
            topics: vec![],
        },
        TopicCase {
            name: "one simple name",
            directory: "corp.example.com",
            topics: vec!["ds-status".to_string()],
        },
        TopicCase {
            name: "several names of mixed form",
            directory: "sales.example.org",
            topics: vec![
                "Topic_1".to_string(),
                "topic-2".to_string(),
                "TOPIC3".to_string(),
            ],
        },
        TopicCase {
            name: "shortest name the model admits",
            directory: "short.example.com",
            topics: vec!["a".repeat(TOPIC_NAME_MIN_LEN)],
        },
        TopicCase {
            name: "longest name the model admits",
            directory: "long.example.com",
            topics: vec!["x".repeat(TOPIC_NAME_MAX_LEN)],
        },
    ];

    for case in &cases {
        let label = case.name;
        let directory_id = create_directory(&router, minimal_create(case.directory), label).await;

        // A directory with no association reports an empty list, not a 501 and not a
        // list of something nobody registered.
        let before = describe(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: before registering"),
        )
        .await;
        assert!(
            before.is_empty(),
            "case {label:?}: expected no association before RegisterEventTopic, got {before:?}"
        );

        let registered_from = epoch_seconds(SystemTime::now());
        for topic in &case.topics {
            register(&router, &directory_id, topic, label).await;
        }
        let registered_until = epoch_seconds(SystemTime::now());

        let reported = describe(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: after registering"),
        )
        .await;
        assert_eq!(
            reported_names(&reported),
            case.topics.iter().cloned().collect::<BTreeSet<String>>(),
            "case {label:?}: expected exactly the registered topic names, got {reported:?}"
        );

        for topic in &reported {
            let topic_name = topic["TopicName"].as_str().unwrap_or_default();
            assert_eq!(
                topic["DirectoryId"],
                json!(&directory_id),
                "case {label:?}: expected the association to name its directory, got {topic}"
            );
            // An SNS topic ARN's resource portion is the bare topic name, and the
            // account segment is the one canonical account id the emulator presents.
            assert_eq!(
                topic["TopicArn"],
                json!(engine::emulator_arn("sns", REGION, topic_name)),
                "case {label:?}: expected the SNS topic ARN for {topic_name:?}, got {topic}"
            );
            assert_eq!(
                topic["Status"],
                json!(STATUS_REGISTERED),
                "case {label:?}: expected the registration status, got {topic}"
            );
            let created = topic["CreatedDateTime"].as_f64().unwrap_or_else(|| {
                panic!("case {label:?}: expected a CreatedDateTime timestamp, got {topic}")
            });
            assert!(
                (registered_from - 1.0..=registered_until + 1.0).contains(&created),
                "case {label:?}: expected CreatedDateTime within the registering window \
                 [{registered_from}, {registered_until}], got {created}"
            );
        }

        // A read does not change what the next caller sees, and it does not consume the
        // association either.
        let again = describe(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: repeated read"),
        )
        .await;
        assert_eq!(
            reported, again,
            "case {label:?}: expected a repeated DescribeEventTopics to report the same \
             associations, got {again:?}"
        );

        // Re-registering a topic the directory already publishes to is the association
        // it already has: no duplicate, and `CreatedDateTime` does not move.
        for topic in &case.topics {
            register(
                &router,
                &directory_id,
                topic,
                &format!("{label}: re-register {topic}"),
            )
            .await;
        }
        let after_repeat = describe(
            &router,
            json!({ "DirectoryId": &directory_id }),
            &format!("{label}: after re-registering"),
        )
        .await;
        assert_eq!(
            after_repeat, reported,
            "case {label:?}: expected re-registering to change nothing, got {after_repeat:?}"
        );
    }

    // One past the upper `TopicName` length bound is refused by the modelled
    // constraint, before any handler runs.
    let directory_id = create_directory(&router, minimal_create("bounds.example.com"), "bounds").await;
    let (status, parsed) = call(
        &router,
        "RegisterEventTopic",
        json!({ "DirectoryId": &directory_id, "TopicName": "y".repeat(TOPIC_NAME_MAX_LEN + 1) }),
        )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the TopicName length bound\": expected 400, got {status} with {parsed}"
    );
    let remaining = describe(
        &router,
        json!({ "DirectoryId": &directory_id }),
        "one past the TopicName length bound",
    )
    .await;
    assert!(
        remaining.is_empty(),
        "case \"one past the TopicName length bound\": expected a refused register to store \
         nothing, got {remaining:?}"
    );
}

#[tokio::test]
async fn the_topic_arn_names_the_region_the_request_was_signed_for() {
    let router = service();

    // The topic "must be in the same region as the specified Directory ID", so the ARN
    // follows the caller's own Region rather than a fixed one. Both rows go through the
    // same handler with a different sigv4 credential scope.
    for region in [REGION, "eu-west-1"] {
        let directory_id =
            create_directory(&router, minimal_create(&format!("{region}.example.com")), region)
                .await;
        let topic_name = format!("status-{region}");

        let (status, parsed) = call_in_region(
            &router,
            "RegisterEventTopic",
            json!({ "DirectoryId": &directory_id, "TopicName": &topic_name }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected RegisterEventTopic to answer 200, got {status} with {parsed}"
        );

        let reported = describe(&router, json!({ "DirectoryId": &directory_id }), region).await;
        assert_eq!(
            reported.len(),
            1,
            "case {region:?}: expected exactly one association, got {reported:?}"
        );
        assert_eq!(
            reported[0]["TopicArn"],
            json!(engine::emulator_arn("sns", region, &topic_name)),
            "case {region:?}: expected the ARN to name the signing Region, got {}",
            reported[0]
        );
    }
}

#[tokio::test]
async fn event_topics_are_narrowed_by_the_filters_the_request_gives() {
    let router = service();

    let first = create_directory(&router, minimal_create("one.example.com"), "filters").await;
    let second = create_directory(&router, minimal_create("two.example.com"), "filters").await;
    for topic in ["alpha", "beta", "gamma"] {
        register(&router, &first, topic, "filters").await;
    }
    for topic in ["beta", "delta"] {
        register(&router, &second, topic, "filters").await;
    }

    let everything: BTreeSet<(String, String)> = [
        (first.clone(), "alpha".to_string()),
        (first.clone(), "beta".to_string()),
        (first.clone(), "gamma".to_string()),
        (second.clone(), "beta".to_string()),
        (second.clone(), "delta".to_string()),
    ]
    .into_iter()
    .collect();

    // The rows differ along which filters the request carries: neither, the directory
    // only, the topic names only, both, a name registered on the other directory only,
    // and a name nothing registered.
    let cases: Vec<FilterCase> = vec![
        FilterCase {
            case: "no filter: every association in the account",
            request: json!({}),
            expected: everything.clone(),
        },
        FilterCase {
            case: "directory filter only",
            request: json!({ "DirectoryId": &first }),
            expected: everything
                .iter()
                .filter(|(directory, _)| *directory == first)
                .cloned()
                .collect(),
        },
        FilterCase {
            case: "topic names only, across directories",
            request: json!({ "TopicNames": ["beta"] }),
            expected: [
                (first.clone(), "beta".to_string()),
                (second.clone(), "beta".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        FilterCase {
            case: "both filters, one name",
            request: json!({ "DirectoryId": &second, "TopicNames": ["delta"] }),
            expected: [(second.clone(), "delta".to_string())]
                .into_iter()
                .collect(),
        },
        FilterCase {
            case: "both filters, two names",
            request: json!({ "DirectoryId": &first, "TopicNames": ["alpha", "gamma"] }),
            expected: [
                (first.clone(), "alpha".to_string()),
                (first.clone(), "gamma".to_string()),
            ]
            .into_iter()
            .collect(),
        },
        FilterCase {
            case: "a name registered only on the other directory",
            request: json!({ "DirectoryId": &first, "TopicNames": ["delta"] }),
            expected: BTreeSet::new(),
        },
        FilterCase {
            case: "a name nothing registered",
            request: json!({ "DirectoryId": &first, "TopicNames": ["never-registered"] }),
            expected: BTreeSet::new(),
        },
        FilterCase {
            case: "a filter naming more topics than are registered",
            request: json!({ "TopicNames": ["alpha", "delta", "never-registered"] }),
            expected: [
                (first.clone(), "alpha".to_string()),
                (second.clone(), "delta".to_string()),
            ]
            .into_iter()
            .collect(),
        },
    ];

    for FilterCase {
        case,
        request,
        expected,
    } in cases
    {
        let reported = describe(&router, request, case).await;
        assert_eq!(
            reported_pairs(&reported),
            expected,
            "case {case:?}: expected the filters to narrow the result, got {reported:?}"
        );
    }

    // "An empty list results in an InvalidParameterException being thrown."
    refused(
        &router,
        "DescribeEventTopics",
        json!({ "TopicNames": [] }),
        "InvalidParameterException",
        "empty TopicNames",
    )
    .await;

    // A `TopicNames` filter exactly at this emulator's documented guard is resolved...
    let at_guard: Vec<String> = (0..TOPIC_NAMES_GUARD)
        .map(|index| format!("topic-{index}"))
        .collect();
    let reported = describe(
        &router,
        json!({ "TopicNames": at_guard }),
        "a filter at the TopicNames guard",
    )
    .await;
    assert!(
        reported.is_empty(),
        "case \"a filter at the TopicNames guard\": expected no association to match the \
         generated names, got {reported:?}"
    );
    // ...and one entry past it is refused before anything is allocated from it.
    let past_guard: Vec<String> = (0..=TOPIC_NAMES_GUARD)
        .map(|index| format!("topic-{index}"))
        .collect();
    refused(
        &router,
        "DescribeEventTopics",
        json!({ "TopicNames": past_guard }),
        "InvalidParameterException",
        "one past the TopicNames guard",
    )
    .await;
}

#[tokio::test]
async fn an_absent_directory_is_refused_by_both_operations() {
    let router = service();

    let existing = create_directory(&router, minimal_create("present.example.com"), "absent").await;
    register(&router, &existing, "kept", "absent").await;

    // A directory this account never created, in several forms the modelled
    // `DirectoryId` pattern `^d-[0-9a-f]{10}$` admits...
    let mut absent: Vec<String> = vec![
        "d-0000000000".to_string(),
        "d-00000000ff".to_string(),
        "d-abcdef0123".to_string(),
        "d-ffffffffff".to_string(),
    ];
    // ...and one that existed until it was deleted, so the refusal follows the store
    // rather than the shape of the id.
    let doomed = create_directory(&router, minimal_create("doomed.example.com"), "absent").await;
    register(&router, &doomed, "doomed-topic", "absent").await;
    crate::harness::delete_directory_and_wait(&router, &doomed, "absent: delete").await;
    absent.push(doomed.clone());

    for directory_id in &absent {
        for (operation, body) in [
            (
                "RegisterEventTopic",
                json!({ "DirectoryId": directory_id, "TopicName": "nowhere" }),
            ),
            (
                "DescribeEventTopics",
                json!({ "DirectoryId": directory_id }),
            ),
        ] {
            let case = format!("{operation} against {directory_id}");
            refused(
                &router,
                operation,
                body.clone(),
                "EntityDoesNotExistException",
                &case,
            )
            .await;
            // The refusal is built from the request: it quotes the id the caller
            // asked for, so the caller can fix the request from the message alone.
            let (_, parsed) = call(&router, operation, body).await;
            let message = parsed["message"]
                .as_str()
                .or_else(|| parsed["Message"].as_str())
                .unwrap_or_default();
            assert!(
                message.contains(&format!("'{directory_id}'")),
                "case {case:?}: expected the message to name the requested directory, got {parsed}"
            );
        }
    }

    // A refused register stored nothing, and a deleted directory took its own
    // associations with it: the account's associations are the surviving directory's.
    let reported = describe(&router, json!({}), "absent: account-wide").await;
    assert_eq!(
        reported_pairs(&reported),
        [(existing.clone(), "kept".to_string())]
            .into_iter()
            .collect::<BTreeSet<(String, String)>>(),
        "case \"absent\": expected only the surviving directory's association, got {reported:?}"
    );
}

#[tokio::test]
async fn associations_are_capped_by_the_documented_per_directory_guard() {
    let router = service();

    let directory_id = create_directory(&router, minimal_create("busy.example.com"), "guard").await;
    for index in 0..TOPIC_GUARD {
        register(
            &router,
            &directory_id,
            &format!("topic-{index}"),
            "guard: filling",
        )
        .await;
    }

    // Re-registering one the directory already publishes to stays accepted at the
    // guard: it adds nothing.
    register(&router, &directory_id, "topic-0", "guard: repeat at the guard").await;
    // A fresh name past the guard is refused, and the stored associations are untouched.
    refused(
        &router,
        "RegisterEventTopic",
        json!({ "DirectoryId": &directory_id, "TopicName": "one-too-many" }),
        "ClientException",
        "one past the per-directory guard",
    )
    .await;

    let reported = describe(&router, json!({ "DirectoryId": &directory_id }), "guard").await;
    assert_eq!(
        reported.len(),
        TOPIC_GUARD,
        "case \"guard\": expected exactly {TOPIC_GUARD} associations, got {}",
        reported.len()
    );
    assert!(
        !reported_names(&reported).contains("one-too-many"),
        "case \"guard\": expected the refused register to store nothing, got {reported:?}"
    );
}
