// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for three rules this service states in the SENTENCE it answers with,
//! driven over the wire through the shared harness in [`super::support`]:
//!
//! - a re-create of an existing queue name compares EVERY settable attribute, the access
//!   policy included, and compares a policy on what it GRANTS rather than on the bytes it
//!   was uploaded as: two documents naming the same effects, actions, resources and
//!   principals are the same policy however they are spelled, and a statement's `Sid`
//!   names a statement without changing the grant, so it is not part of the comparison. A
//!   conflicting re-create is refused `QueueNameExists` and leaves the stored policy
//!   alone;
//! - a conflicting TAG set is refused with its own sentence, because tags are not an
//!   attribute (the remedy is `TagQueue`, not `SetQueueAttributes`);
//! - a page size outside the range a listing's `MaxResults` publishes, and a tag key or
//!   value outside the tag rules, are refused with the sentence that OPERATION answers
//!   with — stating the rule and the published bounds, with nothing of the request in it.
//!
//! Every table spans what its rule generalises over rather than one example: for the
//! policy, which element of a statement differs and how the document is spelled; for the
//! page size, the listing and which end of its range was missed; for the tags, which rule
//! was broken, by which operation, at the published bound and one past it. Nothing here
//! reaches into the crate's state for an assertion: what a queue holds is read back
//! through `GetQueueAttributes` and `ListQueueTags`.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::{
    MAX_TAG_KEY_LENGTH, MAX_TAG_VALUE_LENGTH, POLICY, RECREATE_TAGS_CONFLICT_MESSAGE,
    RESERVED_TAG_KEY_PREFIX, SYSTEM_TAG_REFUSAL,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    all_attributes, call, create, create_ok, emulator, error_code, list_tags, ok, OTHER_ACCOUNT,
};

/// The refusal message of a reply, as awsJson1_0 carries it.
fn message(answer: &Value) -> String {
    answer["message"].as_str().unwrap_or_default().to_string()
}

/// A policy document with one statement, as the `Policy` attribute carries it.
///
/// `members` are the statement's own elements, so a row changes exactly the one it is
/// about and nothing else.
fn policy(members: Value) -> String {
    json!({"Version": "2012-10-17", "Statement": [members]}).to_string()
}

/// The statement every policy row starts from: a grant to another account's root, which
/// is a principal this service accepts in a stored policy.
fn granting_statement() -> Value {
    json!({
        "Sid": "first",
        "Effect": "Allow",
        "Principal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")},
        "Action": "sqs:SendMessage",
        "Resource": "*",
    })
}

/// The same statement with `element` replaced by `value`, or removed when `value` is
/// `None`.
fn statement_with(element: &str, value: Option<Value>) -> Value {
    let mut statement = granting_statement();
    match value {
        Some(value) => statement[element] = value,
        None => {
            statement
                .as_object_mut()
                .expect("a statement is an object")
                .remove(element);
        }
    }
    statement
}

/// A re-create is compared on what its policy GRANTS: a document differing only in how it
/// is spelled, or only in a statement's `Sid`, asks for nothing different and is answered
/// with the existing url, while one changing an effect, an action, a resource, a principal
/// or the statement set is refused `QueueNameExists` — and either way the stored policy is
/// left exactly as the create wrote it.
#[tokio::test]
async fn re_creating_a_name_compares_the_policy_on_what_it_grants() {
    let router = emulator();
    let created = policy(granting_statement());
    // (case, the `Policy` the re-create supplies, whether it asks for nothing different)
    let rows: Vec<(&str, Option<String>, bool)> = vec![
        ("the identical document", Some(created.clone()), true),
        (
            "a document differing only in a statement's Sid",
            Some(policy(statement_with("Sid", Some(json!("second"))))),
            true,
        ),
        (
            "a document carrying no Sid at all",
            Some(policy(statement_with("Sid", None))),
            true,
        ),
        (
            // The policy language takes "a single statement or an array of statements",
            // so the two spellings of one statement are one policy.
            "the one statement as a lone object rather than a list",
            Some(
                json!({"Version": "2012-10-17", "Statement": granting_statement()}).to_string(),
            ),
            true,
        ),
        (
            "the same document with its members in another order",
            Some(
                json!({
                    "Statement": [{
                        "Resource": "*",
                        "Action": "sqs:SendMessage",
                        "Principal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")},
                        "Effect": "Allow",
                        "Sid": "first",
                    }],
                    "Version": "2012-10-17",
                })
                .to_string(),
            ),
            true,
        ),
        (
            "a document whose Effect differs",
            Some(policy(statement_with("Effect", Some(json!("Deny"))))),
            false,
        ),
        (
            "a document whose Action differs",
            Some(policy(statement_with(
                "Action",
                Some(json!("sqs:ReceiveMessage")),
            ))),
            false,
        ),
        (
            "a document whose Resource differs",
            Some(policy(statement_with(
                "Resource",
                Some(json!("arn:aws:sqs:us-east-1:000000000000:other")),
            ))),
            false,
        ),
        (
            "a document whose Principal differs",
            Some(policy(statement_with(
                "Principal",
                Some(json!({"AWS": "arn:aws:iam::210987654321:root"})),
            ))),
            false,
        ),
        (
            "a document carrying one statement more",
            Some(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        granting_statement(),
                        statement_with("Action", Some(json!("sqs:ReceiveMessage"))),
                    ],
                })
                .to_string(),
            ),
            false,
        ),
        ("no Policy attribute at all", None, false),
    ];

    for (index, (case, supplied, idempotent)) in rows.iter().enumerate() {
        let name = format!("recreate-policy-{index}");
        let url = create_ok(&router, &name, &json!({ POLICY: created.clone() })).await;

        let asked = match supplied {
            Some(document) => json!({ POLICY: document.clone() }),
            None => json!({}),
        };
        let (status, parsed) = create(&router, &name, &asked).await;
        if *idempotent {
            assert_eq!(
                (status, parsed["QueueUrl"].as_str()),
                (200, Some(url.as_str())),
                "case {case}: expected the existing url {url}, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                (status, error_code(&parsed).as_str(), message(&parsed).as_str()),
                (
                    400,
                    "QueueNameExists",
                    "A queue already exists with the same name and a different value for \
                     attribute Policy"
                ),
                "case {case}: expected the attribute conflict for Policy, got {status} with \
                 {parsed}"
            );
        }
        // Neither outcome writes the queue: the policy the create stored is the one it
        // still reports, verbatim.
        let reported = all_attributes(&router, &url).await;
        assert_eq!(
            reported.get(POLICY).map(String::as_str),
            Some(created.as_str()),
            "case {case}: the re-create must leave the stored policy alone, got {reported:?}"
        );
    }
}

/// A policy a re-create supplies where the queue carries none, and a value that is not a
/// policy document at all, are each compared too: the first conflicts, and the second is
/// compared as the string it is.
#[tokio::test]
async fn a_policy_the_queue_does_not_carry_conflicts_on_a_re_create() {
    let router = emulator();
    let document = policy(granting_statement());
    let url = create_ok(&router, "recreate-policy-added", &json!({})).await;
    let (status, parsed) = create(
        &router,
        "recreate-policy-added",
        &json!({ POLICY: document.clone() }),
    )
    .await;
    assert_eq!(
        (status, error_code(&parsed).as_str()),
        (400, "QueueNameExists"),
        "case a policy added by the re-create: expected a 400 QueueNameExists, got {status} \
         with {parsed}"
    );
    let reported = all_attributes(&router, &url).await;
    assert_eq!(
        reported.get(POLICY),
        None,
        "case a policy added by the re-create: the queue must still carry none, got {reported:?}"
    );
}

/// A re-create that supplies a different TAG set is refused with the TAG sentence, not
/// with the per-attribute one, whichever way the set differs — and the queue keeps its
/// tags.
#[tokio::test]
async fn a_tag_set_conflict_is_refused_with_the_tag_sentence() {
    let router = emulator();
    // (case, the tags the create carried, the `tags` the re-create supplies)
    let rows: &[(&str, Value, Value)] = &[
        (
            "a different value for the one key",
            json!({"Environment": "production"}),
            json!({"Environment": "staging"}),
        ),
        (
            "one tag more than the queue carries",
            json!({"Environment": "production"}),
            json!({"Environment": "production", "Team": "payments"}),
        ),
        (
            "one tag fewer than the queue carries",
            json!({"Environment": "production", "Team": "payments"}),
            json!({"Environment": "production"}),
        ),
        (
            "an empty tag map on a tagged queue",
            json!({"Environment": "production"}),
            json!({}),
        ),
        (
            "a tag set on a queue created without one",
            json!({}),
            json!({"Environment": "production"}),
        ),
    ];

    for (index, (case, created_with, supplied)) in rows.iter().enumerate() {
        let name = format!("recreate-tag-sentence-{index}");
        let parsed = ok(
            &router,
            "CreateQueue",
            json!({"QueueName": name, "tags": created_with}),
        )
        .await;
        let url = parsed["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no QueueUrl in {parsed}"))
            .to_string();
        let before = list_tags(&router, &url).await;

        let (status, parsed) = call(
            &router,
            "CreateQueue",
            &json!({"QueueName": name, "tags": supplied}),
        )
        .await;
        assert_eq!(
            (status, error_code(&parsed).as_str(), message(&parsed)),
            (
                400,
                "QueueNameExists",
                RECREATE_TAGS_CONFLICT_MESSAGE.to_string()
            ),
            "case {case}: expected the tag conflict sentence, got {status} with {parsed}"
        );
        assert_eq!(
            list_tags(&router, &url).await,
            before,
            "case {case}: the refused re-create must leave the queue's tags alone"
        );
    }
}

/// Each listing refuses a page size outside the range its own `MaxResults` publishes, with
/// the sentence THAT listing answers with: the rule and its published bounds, and none of
/// the rejected value.
#[tokio::test]
async fn each_listing_words_its_own_page_size_refusal() {
    let router = emulator();
    let (dead_letter, dead_letter_arn, _source) =
        super::support::dead_letter_queue(&router, "page-size-dlq").await;
    // (case, operation, the request, the sentence that listing answers with)
    //
    // The bounds are each member's own documented range: `ListQueuesRequest$MaxResults`
    // and `ListDeadLetterSourceQueuesRequest$MaxResults` publish "Value range is 1 to
    // 1000", `ListMessageMoveTasksRequest$MaxResults` "The upper limit is 10".
    let queue_range = "Value for parameter MaxResults is invalid. Reason: MaxResults must be an \
                       integer between 1 and 1000.";
    let task_range = "Value for parameter MaxResults is invalid. Reason: You must enter a number \
                      that's between 1 and 10.";
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a queue listing one past its upper bound",
            "ListQueues",
            json!({"MaxResults": 1_001}),
            queue_range,
        ),
        (
            "a queue listing below its lower bound",
            "ListQueues",
            json!({"MaxResults": 0}),
            queue_range,
        ),
        (
            "a queue listing given a negative page size",
            "ListQueues",
            json!({"MaxResults": -7}),
            queue_range,
        ),
        (
            "a dead-letter source listing one past its upper bound",
            "ListDeadLetterSourceQueues",
            json!({"QueueUrl": dead_letter.clone(), "MaxResults": 1_001}),
            queue_range,
        ),
        (
            "a message-move listing below its lower bound",
            "ListMessageMoveTasks",
            json!({"SourceArn": dead_letter_arn.clone(), "MaxResults": 0}),
            task_range,
        ),
        (
            "a message-move listing one past its upper bound",
            "ListMessageMoveTasks",
            json!({"SourceArn": dead_letter_arn.clone(), "MaxResults": 11}),
            task_range,
        ),
    ];

    for (case, operation, request, expected) in &rows {
        let (status, parsed) = call(&router, operation, request).await;
        assert_eq!(
            (status, error_code(&parsed).as_str(), message(&parsed).as_str()),
            (400, "InvalidParameterValue", *expected),
            "case {case}: expected that listing's own page-size sentence, got {status} with \
             {parsed}"
        );
    }
}

/// A tag key or value outside the tag rules is refused with the RULE it broke — the
/// published bounds, or the system-tag refusal for the reserved namespace — by every
/// operation that names tags, and the refused call adds and removes nothing.
#[tokio::test]
async fn a_tag_rule_refusal_states_the_rule_it_broke() {
    let router = emulator();
    let key_rule = format!("Tag keys must be between 1 and {MAX_TAG_KEY_LENGTH} characters in length.");
    let value_rule =
        format!("Tag values must be between 0 and {MAX_TAG_VALUE_LENGTH} characters in length.");
    // The reserved prefix spelled in another case, which is still the reserved prefix.
    let reserved_mixed = format!(
        "{}{}owner",
        RESERVED_TAG_KEY_PREFIX[..1].to_uppercase(),
        &RESERVED_TAG_KEY_PREFIX[1..]
    );
    // (case, operation, the tag member the call carries, the sentence it is refused with)
    let rows: Vec<(String, &str, Value, String)> = vec![
        (
            "a tag value one character past its bound".to_string(),
            "TagQueue",
            json!({"Tags": {"Environment": "v".repeat(MAX_TAG_VALUE_LENGTH + 1)}}),
            value_rule.clone(),
        ),
        (
            "a tag value far past its bound".to_string(),
            "TagQueue",
            json!({"Tags": {"Environment": "v".repeat(MAX_TAG_VALUE_LENGTH * 4)}}),
            value_rule.clone(),
        ),
        (
            "a tag key one character past its bound".to_string(),
            "TagQueue",
            json!({"Tags": {"k".repeat(MAX_TAG_KEY_LENGTH + 1): "v"}}),
            key_rule.clone(),
        ),
        (
            "a tag key holding no character".to_string(),
            "TagQueue",
            json!({"Tags": {"": "v"}}),
            key_rule.clone(),
        ),
        (
            "a tag key in the reserved namespace".to_string(),
            "TagQueue",
            json!({"Tags": {format!("{RESERVED_TAG_KEY_PREFIX}owner"): "me"}}),
            SYSTEM_TAG_REFUSAL.to_string(),
        ),
        (
            "a tag key in the reserved namespace, spelled in another case".to_string(),
            "TagQueue",
            json!({"Tags": { reserved_mixed.clone(): "me"}}),
            SYSTEM_TAG_REFUSAL.to_string(),
        ),
        (
            "an untag naming a key one character past its bound".to_string(),
            "UntagQueue",
            json!({"TagKeys": ["k".repeat(MAX_TAG_KEY_LENGTH + 1)]}),
            key_rule.clone(),
        ),
        (
            "an untag naming a key holding no character".to_string(),
            "UntagQueue",
            json!({"TagKeys": [""]}),
            key_rule.clone(),
        ),
        (
            "an untag naming a key in the reserved namespace".to_string(),
            "UntagQueue",
            json!({"TagKeys": [format!("{RESERVED_TAG_KEY_PREFIX}owner")]}),
            SYSTEM_TAG_REFUSAL.to_string(),
        ),
        (
            "an untag naming one acceptable key and one past the bound".to_string(),
            "UntagQueue",
            json!({"TagKeys": ["Environment", "k".repeat(MAX_TAG_KEY_LENGTH + 1)]}),
            key_rule.clone(),
        ),
    ];

    for (index, (case, operation, member, expected)) in rows.iter().enumerate() {
        let url = create_ok(&router, &format!("tag-rule-{index}"), &json!({})).await;
        let kept = json!({"Environment": "production"});
        let _ = ok(
            &router,
            "TagQueue",
            json!({"QueueUrl": url, "Tags": kept}),
        )
        .await;
        let before = list_tags(&router, &url).await;

        let mut request = member.clone();
        request["QueueUrl"] = json!(url);
        let (status, parsed) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&parsed).as_str(), message(&parsed)),
            (400, "InvalidParameterValue", expected.clone()),
            "case {case}: expected the tag rule it broke, got {status} with {parsed}"
        );
        assert_eq!(
            list_tags(&router, &url).await,
            before,
            "case {case}: the refused call must add and remove nothing"
        );
    }
}

/// The same rules AT the published bound are accepted, by every operation that names
/// tags: a key of one and of [`MAX_TAG_KEY_LENGTH`] characters, a value of none and of
/// [`MAX_TAG_VALUE_LENGTH`] characters, and the reserved prefix's letters where they are
/// not the prefix.
#[tokio::test]
async fn a_tag_at_each_published_bound_is_accepted() {
    let router = emulator();
    let url = create_ok(&router, "tag-bounds", &json!({})).await;
    // (case, the key, the value)
    let rows: Vec<(String, String, String)> = vec![
        ("a single-character key".to_string(), "k".to_string(), "v".to_string()),
        (
            format!("a key of exactly {MAX_TAG_KEY_LENGTH} characters"),
            "k".repeat(MAX_TAG_KEY_LENGTH),
            "v".to_string(),
        ),
        (
            "a value holding no character".to_string(),
            "empty-value".to_string(),
            String::new(),
        ),
        (
            format!("a value of exactly {MAX_TAG_VALUE_LENGTH} characters"),
            "long-value".to_string(),
            "v".repeat(MAX_TAG_VALUE_LENGTH),
        ),
        (
            "the reserved prefix's letters without its colon".to_string(),
            RESERVED_TAG_KEY_PREFIX.trim_end_matches(':').to_string(),
            "v".to_string(),
        ),
        (
            "the reserved prefix anywhere but the front".to_string(),
            format!("mine:{RESERVED_TAG_KEY_PREFIX}owner"),
            "v".to_string(),
        ),
    ];

    for (case, key, value) in &rows {
        let _ = ok(
            &router,
            "TagQueue",
            json!({"QueueUrl": url, "Tags": {key.clone(): value.clone()}}),
        )
        .await;
        let reported = list_tags(&router, &url).await;
        assert_eq!(
            reported.get(key).map(String::as_str),
            Some(value.as_str()),
            "case {case}: the tag must round-trip through ListQueueTags, got {reported:?}"
        );

        let _ = ok(
            &router,
            "UntagQueue",
            json!({"QueueUrl": url, "TagKeys": [key.clone()]}),
        )
        .await;
        let reported = list_tags(&router, &url).await;
        assert!(
            !reported.contains_key(key),
            "case {case}: the untag must remove the key it names, got {reported:?}"
        );
    }
}

/// Every refused tag call above leaves the queue reachable and reporting the tags it had,
/// which is what makes the refusals above mutate nothing: asserted through a reader rather
/// than through the store.
#[tokio::test]
async fn a_refused_tag_call_leaves_the_queue_readable() {
    let router: MultiServiceRouter = emulator();
    let url = create_ok(&router, "tag-refusal-readable", &json!({})).await;
    let _ = ok(
        &router,
        "TagQueue",
        json!({"QueueUrl": url, "Tags": {"Environment": "production"}}),
    )
    .await;
    let (status, _parsed) = call(
        &router,
        "TagQueue",
        &json!({"QueueUrl": url, "Tags": {format!("{RESERVED_TAG_KEY_PREFIX}owner"): "me"}}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a reserved key on a tagged queue: expected the call to be refused"
    );
    let reported = list_tags(&router, &url).await;
    assert_eq!(
        reported.get("Environment").map(String::as_str),
        Some("production"),
        "case a reserved key on a tagged queue: the queue must still report its tag, got \
         {reported:?}"
    );
}
