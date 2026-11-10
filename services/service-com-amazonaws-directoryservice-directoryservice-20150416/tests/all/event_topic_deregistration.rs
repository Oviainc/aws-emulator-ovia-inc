// SPDX-License-Identifier: CC0-1.0

//! `DeregisterEventTopic` — removing a directory as a publisher to an Amazon SNS topic,
//! driven end to end over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (the harness is the sibling suite's, reached as
//! `crate::directory_lifecycle::<item>`). Nothing a response claims is read out of the
//! state struct: what survives a deregistration is read back through
//! `DescribeEventTopics`, so an answer that is not computed from persisted state cannot
//! pass.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded call:
//!
//! * a deregistration drops exactly the association the request names — the directory it
//!   names and the topic it names — and leaves every other association of that directory,
//!   and the same topic name on another directory, in place;
//! * a topic the directory does not publish to leaves nothing to drop: the state the call
//!   asks for already holds, so the call is answered and the stored associations do not
//!   move (which also makes a repeated deregistration answer the same);
//! * a `DirectoryId` that satisfies the modelled pattern but names no stored directory is
//!   refused with `EntityDoesNotExistException`, quoting the id the request asked for —
//!   the same refusal its sibling operations give that id — and removes nothing;
//! * the operation never answers the scaffold's `NotImplemented`.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{call, create_directory, minimal_create, ok, refused, service};

/// The published bounds of the modelled `TopicName` shape: `length 1..256`
/// (`smithy_query(query="operation", operation="DeregisterEventTopic")`).
const TOPIC_NAME_MIN_LEN: usize = 1;
const TOPIC_NAME_MAX_LEN: usize = 256;

/// Well-formed `DirectoryId`s no directory is created under in this suite, as
/// `(id, class)`. The form is the model's: `DirectoryId` is `^d-[0-9a-f]{10}$`, so every
/// row is a value the boundary validator admits and the handler has to resolve in the
/// store.
const ABSENT_DIRECTORY_IDS: [(&str, &str); 4] = [
    ("d-0000000000", "all zeros, the low end of the alphabet"),
    ("d-ffffffffff", "all f, the high end of the alphabet"),
    ("d-abcdef0123", "every hex digit once"),
    ("d-b99b7e1a29", "digits and letters interleaved"),
];

/// One row of the association table: a directory, the topic names registered on it, and
/// the one this row deregisters.
struct TopicCase {
    case: &'static str,
    domain: &'static str,
    topics: Vec<String>,
    deregister: String,
}

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

/// `DeregisterEventTopic`, asserting it was accepted.
async fn deregister(router: &MultiServiceRouter, directory_id: &str, topic_name: &str, case: &str) {
    let answer = ok(
        router,
        "DeregisterEventTopic",
        json!({ "DirectoryId": directory_id, "TopicName": topic_name }),
        case,
    )
    .await;
    // The operation models no output members, so the answer carries none.
    assert_eq!(
        answer,
        json!({}),
        "case {case:?}: expected an empty DeregisterEventTopicResult, got {answer}"
    );
}

/// The topic names `DescribeEventTopics` reports for `directory_id`, as a set.
async fn reported_names(
    router: &MultiServiceRouter,
    directory_id: &str,
    case: &str,
) -> BTreeSet<String> {
    let answer = ok(
        router,
        "DescribeEventTopics",
        json!({ "DirectoryId": directory_id }),
        case,
    )
    .await;
    answer["EventTopics"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected EventTopics, got {answer}"))
        .iter()
        .map(|topic| {
            topic["TopicName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The `(DirectoryId, TopicName)` pairs `DescribeEventTopics` reports for the whole
/// account, as a set.
async fn account_pairs(router: &MultiServiceRouter, case: &str) -> BTreeSet<(String, String)> {
    let answer = ok(router, "DescribeEventTopics", json!({}), case).await;
    answer["EventTopics"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected EventTopics, got {answer}"))
        .iter()
        .map(|topic| {
            (
                topic["DirectoryId"].as_str().unwrap_or_default().to_string(),
                topic["TopicName"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `Message` of a refusal body, whichever casing the envelope carries it in.
fn message(parsed: &Value, case: &str) -> String {
    parsed["Message"]
        .as_str()
        .or_else(|| parsed["message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a message, got {parsed}"))
        .to_string()
}

/// The association table: the rows differ in how many topics a directory publishes to and
/// in the form and length of the names the modelled `TopicName` admits — letters, digits,
/// underscores and hyphens, at both published length bounds.
fn topic_rows() -> Vec<TopicCase> {
    vec![
        TopicCase {
            case: "the only association of a directory",
            domain: "only.example.com",
            topics: vec!["ds-status".to_string()],
            deregister: "ds-status".to_string(),
        },
        TopicCase {
            case: "one of several, names of mixed form",
            domain: "several.example.com",
            topics: vec![
                "Topic_1".to_string(),
                "topic-2".to_string(),
                "TOPIC3".to_string(),
            ],
            deregister: "topic-2".to_string(),
        },
        TopicCase {
            case: "the shortest name the model admits",
            domain: "short.example.com",
            topics: vec!["a".repeat(TOPIC_NAME_MIN_LEN), "other".to_string()],
            deregister: "a".repeat(TOPIC_NAME_MIN_LEN),
        },
        TopicCase {
            case: "the longest name the model admits",
            domain: "long.example.com",
            topics: vec!["x".repeat(TOPIC_NAME_MAX_LEN), "other".to_string()],
            deregister: "x".repeat(TOPIC_NAME_MAX_LEN),
        },
        TopicCase {
            case: "a name that differs from another only in case",
            domain: "cased.example.com",
            topics: vec!["Status".to_string(), "status".to_string()],
            deregister: "status".to_string(),
        },
    ]
}

#[tokio::test]
async fn a_deregistration_drops_exactly_the_association_the_request_names() {
    let router = service();

    // A second directory publishing to every name any row uses, so a row that drops a
    // name cannot be read as dropping that name account-wide.
    let bystander = create_directory(
        &router,
        minimal_create("bystander.example.com"),
        "the directory that keeps its associations",
    )
    .await;
    let rows = topic_rows();
    let mut bystander_names: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        register(&router, &bystander, &row.deregister, row.case).await;
        bystander_names.insert(row.deregister.clone());
    }

    for row in &rows {
        let case = row.case;
        let directory_id = create_directory(&router, minimal_create(row.domain), case).await;
        for topic in &row.topics {
            register(&router, &directory_id, topic, case).await;
        }
        let before = reported_names(&router, &directory_id, &format!("{case}: before")).await;
        assert_eq!(
            before,
            row.topics.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: expected the registered names before deregistering, got {before:?}"
        );

        deregister(&router, &directory_id, &row.deregister, case).await;

        let expected: BTreeSet<String> = row
            .topics
            .iter()
            .filter(|topic| **topic != row.deregister)
            .cloned()
            .collect();
        let after = reported_names(&router, &directory_id, &format!("{case}: after")).await;
        assert_eq!(
            after, expected,
            "case {case:?}: expected only {:?} to be dropped, got {after:?}",
            row.deregister
        );

        // Dropping it again asks for a state that already holds: the call is answered and
        // the surviving associations do not move.
        deregister(
            &router,
            &directory_id,
            &row.deregister,
            &format!("{case}: repeated"),
        )
        .await;
        let repeated = reported_names(&router, &directory_id, &format!("{case}: repeated")).await;
        assert_eq!(
            repeated, expected,
            "case {case:?}: expected a repeated deregistration to change nothing, got \
             {repeated:?}"
        );

        // A name this directory never published to is the same case, and it does not
        // reach into the names it does publish to.
        deregister(
            &router,
            &directory_id,
            "never-registered",
            &format!("{case}: never registered"),
        )
        .await;
        let untouched =
            reported_names(&router, &directory_id, &format!("{case}: never registered")).await;
        assert_eq!(
            untouched, expected,
            "case {case:?}: expected deregistering an unregistered name to change nothing, \
             got {untouched:?}"
        );

        // The same topic name on another directory is another association, and this row
        // did not touch it.
        let bystander_after = reported_names(
            &router,
            &bystander,
            &format!("{case}: the other directory"),
        )
        .await;
        assert_eq!(
            bystander_after, bystander_names,
            "case {case:?}: expected the other directory's associations to survive, got \
             {bystander_after:?}"
        );
    }
}

#[tokio::test]
async fn a_directory_id_that_names_no_directory_is_refused_and_removes_nothing() {
    let router = service();

    let existing = create_directory(
        &router,
        minimal_create("present.example.com"),
        "the stored directory",
    )
    .await;
    register(&router, &existing, "kept", "the stored directory").await;

    // Well-formed ids no directory is stored under...
    let mut absent: Vec<(String, &str)> = ABSENT_DIRECTORY_IDS
        .iter()
        .map(|(id, class)| ((*id).to_string(), *class))
        .collect();
    // ...and one that existed until it was deleted, so the refusal follows the store
    // rather than the shape of the id.
    let doomed = create_directory(&router, minimal_create("doomed.example.com"), "deleted").await;
    register(&router, &doomed, "doomed-topic", "deleted").await;
    crate::harness::delete_directory_and_wait(&router, &doomed, "deleted").await;
    absent.push((doomed.clone(), "a directory that was deleted"));

    for (directory_id, class) in &absent {
        let case = format!("DeregisterEventTopic against {class}");
        let body = json!({ "DirectoryId": directory_id, "TopicName": "nowhere" });
        refused(
            &router,
            "DeregisterEventTopic",
            body.clone(),
            "EntityDoesNotExistException",
            &case,
        )
        .await;

        // The refusal is built from the request and reads exactly like the one its
        // sibling operations give the same id: the quoted id, the fixed not-found
        // sentence, then this service's request-id suffix.
        let (_, parsed) = call(&router, "DeregisterEventTopic", body).await;
        let request_id = parsed["RequestId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RequestId, got {parsed}"));
        assert_eq!(
            message(&parsed, &case),
            format!("Directory: '{directory_id}' does not exist. : RequestId: {request_id}"),
            "case {case:?}: expected this service's not-found sentence, got {parsed}"
        );
        // 501 is "nobody has written it yet"; this operation is written, so no request
        // can reach it.
        assert!(
            !parsed.to_string().contains("NotImplemented"),
            "case {case:?}: expected a modelled refusal rather than the scaffold, got {parsed}"
        );
    }

    // Nothing the refused calls named was removed, and the deleted directory took its own
    // association with it.
    let pairs = account_pairs(&router, "the surviving associations").await;
    assert_eq!(
        pairs,
        [(existing.clone(), "kept".to_string())]
            .into_iter()
            .collect::<BTreeSet<(String, String)>>(),
        "case \"the surviving associations\": expected only the stored directory's \
         association, got {pairs:?}"
    );
}

#[tokio::test]
async fn a_request_the_model_refuses_never_reaches_the_directory_lookup() {
    let router = service();

    let directory_id = create_directory(
        &router,
        minimal_create("modelled.example.com"),
        "the stored directory",
    )
    .await;
    register(&router, &directory_id, "kept", "the stored directory").await;

    // Every row breaks one published constraint of this operation's input
    // (`smithy_query(query="operation", operation="DeregisterEventTopic")`): the
    // `DirectoryId` pattern, the `TopicName` pattern, and both `TopicName` length bounds.
    let rows = [
        (
            json!({ "DirectoryId": "not-a-directory-id", "TopicName": "kept" }),
            "a DirectoryId outside its pattern",
        ),
        (
            json!({ "DirectoryId": &directory_id, "TopicName": "no spaces allowed" }),
            "a TopicName outside its pattern",
        ),
        (
            json!({ "DirectoryId": &directory_id, "TopicName": "" }),
            "a TopicName under its length minimum",
        ),
        (
            json!({
                "DirectoryId": &directory_id,
                "TopicName": "y".repeat(TOPIC_NAME_MAX_LEN + 1)
            }),
            "a TopicName over its length maximum",
        ),
    ];

    for (body, case) in rows {
        let (status, parsed) = call(&router, "DeregisterEventTopic", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        assert_eq!(
            parsed["__type"],
            json!("ValidationException"),
            "case {case:?}: expected the boundary validation envelope, got {parsed}"
        );
    }

    // None of them reached the store.
    let surviving = reported_names(&router, &directory_id, "after the refused requests").await;
    assert_eq!(
        surviving,
        ["kept".to_string()].into_iter().collect::<BTreeSet<String>>(),
        "case \"after the refused requests\": expected the association to survive, got \
         {surviving:?}"
    );
}
