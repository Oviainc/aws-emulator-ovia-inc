// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the dead-letter redrive DOCUMENT: what a `RedrivePolicy` a write
//! supplies must carry to be stored, and the one form it reads back in. Driven over the
//! wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared
//! harness in [`super::support`].
//!
//! Two rules are under test, over BOTH paths that write a queue attribute (`CreateQueue`
//! and `SetQueueAttributes`), because either is where a caller configures a redrive.
//!
//! The CONTENT rule: a redrive policy attribute is accepted only where the document says
//! what the attribute documents — both of the members it defines, a `maxReceiveCount`
//! inside the range that member publishes, and a `deadLetterTargetArn` naming a queue of
//! the source queue's own type — and a document breaking any of those is refused at 400
//! with the attribute left exactly as it was. The rows walk each member's absence, both
//! published bounds of the count and one value past each, a count that is no number at all,
//! both spellings of the count, and every pairing of the two queue types.
//!
//! The CANONICAL rendering rule: what a later read reports is the service's own rendering
//! of that document — compact, with each member in the type its documentation gives it — so
//! the caller's whitespace, its member order and whether it quoted the numeric count do not
//! survive into what is reported. The rows spell ONE policy every way a client might write
//! it and assert the single document they all read back as.

use serde_json::{json, Value};

use super::support::{
    all_attributes, call, create, create_ok, emulator, error_code, expected_arn, fifo_attributes,
};

/// The inclusive range Amazon SQS publishes for a `RedrivePolicy`'s `maxReceiveCount`,
/// spelled out here rather than imported from the crate under test, so a wrong constant
/// cannot make a row pass. Every row at a bound and one past it is driven from it.
const COUNT_RANGE: (i64, i64) = (1, 1_000);

/// The standard dead-letter queue every row of this suite may drive to.
const STANDARD_TARGET: &str = "document-dlq";

/// The FIFO dead-letter queue every row may drive to; a FIFO queue's name carries the
/// `.fifo` suffix.
const FIFO_TARGET: &str = "document-dlq.fifo";

/// A `RedrivePolicy` document naming `target_arn` and carrying `count` as the JSON value a
/// row spells it with.
fn policy(target_arn: &str, count: Value) -> String {
    json!({"deadLetterTargetArn": target_arn, "maxReceiveCount": count}).to_string()
}

/// What a refusal of a `RedrivePolicy` VALUE says, in the one template this service answers
/// a refused parameter value in: the document the request supplied, the parameter it was
/// supplied for, and the rule it broke.
fn refusal(supplied: &str, reason: &str) -> String {
    format!("Value {supplied} for parameter RedrivePolicy is invalid. Reason: {reason}")
}

/// The refusal of a document that leaves out one of the two members the attribute documents.
fn missing_member(supplied: &str, member: &str) -> String {
    refusal(
        supplied,
        &format!("Redrive policy does not contain mandatory attribute: {member}."),
    )
}

/// The refusal of a count the published range does not hold, which states those bounds.
fn out_of_range(supplied: &str, count: &str) -> String {
    let (min, max) = COUNT_RANGE;
    refusal(
        supplied,
        &format!(
            "Invalid value for maxReceiveCount: {count}, valid values are from {min} to {max} \
             both inclusive."
        ),
    )
}

/// The refusal of a pair of queues of different types, in the sentence Amazon SQS states
/// that rule in. Spelled out here rather than imported from the crate under test, so a
/// wrong constant cannot make a row pass; the sentence really does end in two periods.
fn type_mismatch(supplied: &str) -> String {
    refusal(
        supplied,
        "Dead-letter queue must be same type of queue as the source..",
    )
}

/// The refusal of a document whose dead-letter target is not a queue there is: a redrive
/// moves a message to that queue, so one that does not exist configures a move nothing
/// could make.
fn missing_target(supplied: &str) -> String {
    refusal(supplied, "Dead letter target does not exist.")
}

/// One row of the content table.
struct DocumentRow {
    /// What the row is about, named in every assertion it makes.
    case: &'static str,
    /// Whether the queue the policy is written on is a FIFO queue.
    fifo: bool,
    /// The `RedrivePolicy` the write submits.
    policy: String,
    /// The refusal the write must answer, or `None` for a document that is accepted.
    refusal: Option<String>,
}

/// Create the two dead-letter queues the rows drive to, and return a queue called `name` of
/// the row's own type, already wired to the target of that type with a one-delivery
/// allowance: the attribute a refused write must leave untouched.
async fn source_queue(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
    fifo: bool,
) -> (String, String) {
    let baseline = policy(&expected_arn(target_of(fifo)), json!(COUNT_RANGE.0));
    let mut attributes = if fifo { fifo_attributes(&[]) } else { json!({}) };
    attributes["RedrivePolicy"] = json!(baseline);
    let url = create_ok(router, name, &attributes).await;
    (url, baseline)
}

/// The dead-letter queue of the given type: "the dead-letter queue of a FIFO queue must also
/// be a FIFO queue".
fn target_of(fifo: bool) -> &'static str {
    if fifo {
        FIFO_TARGET
    } else {
        STANDARD_TARGET
    }
}

/// The name a row's own queues carry, with the `.fifo` suffix a FIFO queue's name needs.
fn queue_name(prefix: &str, index: usize, fifo: bool) -> String {
    let suffix = if fifo { ".fifo" } else { "" };
    format!("{prefix}-{index}{suffix}")
}

/// Create both dead-letter queues the content rows may name.
async fn create_targets(router: &smithy_server_runtime::MultiServiceRouter) {
    create_ok(router, STANDARD_TARGET, &json!({})).await;
    create_ok(router, FIFO_TARGET, &fifo_attributes(&[])).await;
}

/// The content rows: what a `RedrivePolicy` document has to carry, spanning each member's
/// absence, both published bounds of `maxReceiveCount` and one value past each, a count that
/// is not a number, both spellings of a count, and every pairing of the two queue types.
fn content_rows() -> Vec<DocumentRow> {
    let (min, max) = COUNT_RANGE;
    let standard = expected_arn(STANDARD_TARGET);
    let fifo = expected_arn(FIFO_TARGET);
    let accepted = |case: &'static str, fifo: bool, policy: String| DocumentRow {
        case,
        fifo,
        policy,
        refusal: None,
    };
    let refused = |case: &'static str, fifo: bool, policy: String, refusal: String| DocumentRow {
        case,
        fifo,
        policy,
        refusal: Some(refusal),
    };
    let no_target = json!({"maxReceiveCount": min}).to_string();
    let no_count = json!({"deadLetterTargetArn": standard}).to_string();
    let neither = "{}".to_string();
    let blank_target = json!({"deadLetterTargetArn": "", "maxReceiveCount": min}).to_string();
    let spaces_target = json!({"deadLetterTargetArn": "   ", "maxReceiveCount": min}).to_string();
    // A queue nothing created, and a value that is not a queue ARN at all: neither names a
    // queue a redrive could move a message to.
    let ghost_target = policy(&expected_arn("never-created"), json!(min));
    let unparsable_target = policy("never-created", json!(min));
    vec![
        accepted(
            "the lowest count the range admits",
            false,
            policy(&standard, json!(min)),
        ),
        accepted(
            "the highest count the range admits",
            false,
            policy(&standard, json!(max)),
        ),
        accepted(
            "a count spelled as a string of digits",
            false,
            policy(&standard, json!("500")),
        ),
        accepted(
            "a FIFO source driving to a FIFO dead-letter queue",
            true,
            policy(&fifo, json!(5)),
        ),
        refused(
            "one below the lowest count the range admits",
            false,
            policy(&standard, json!(min - 1)),
            out_of_range(&policy(&standard, json!(min - 1)), &(min - 1).to_string()),
        ),
        refused(
            "one past the highest count the range admits",
            false,
            policy(&standard, json!(max + 1)),
            out_of_range(&policy(&standard, json!(max + 1)), &(max + 1).to_string()),
        ),
        refused(
            "a negative count",
            false,
            policy(&standard, json!(-1)),
            out_of_range(&policy(&standard, json!(-1)), "-1"),
        ),
        refused(
            "a count that is not a number at all",
            false,
            policy(&standard, json!("many")),
            out_of_range(&policy(&standard, json!("many")), "many"),
        ),
        refused(
            "a quoted count below the lowest the range admits",
            false,
            policy(&standard, json!("0")),
            out_of_range(&policy(&standard, json!("0")), "0"),
        ),
        refused(
            "a document carrying no maxReceiveCount",
            false,
            no_count.clone(),
            missing_member(&no_count, "maxReceiveCount"),
        ),
        refused(
            "a document carrying no deadLetterTargetArn",
            false,
            no_target.clone(),
            missing_member(&no_target, "deadLetterTargetArn"),
        ),
        refused(
            "a document carrying neither member",
            false,
            neither.clone(),
            missing_member(&neither, "deadLetterTargetArn"),
        ),
        refused(
            "a deadLetterTargetArn carrying no text",
            false,
            blank_target.clone(),
            missing_member(&blank_target, "deadLetterTargetArn"),
        ),
        refused(
            "a deadLetterTargetArn carrying only spaces",
            false,
            spaces_target.clone(),
            missing_member(&spaces_target, "deadLetterTargetArn"),
        ),
        refused(
            "a deadLetterTargetArn naming a queue nothing created",
            false,
            ghost_target.clone(),
            missing_target(&ghost_target),
        ),
        refused(
            "a deadLetterTargetArn that is not a queue ARN at all",
            false,
            unparsable_target.clone(),
            missing_target(&unparsable_target),
        ),
        refused(
            "a standard source driving to a FIFO dead-letter queue",
            false,
            policy(&fifo, json!(min)),
            type_mismatch(&policy(&fifo, json!(min))),
        ),
        refused(
            "a FIFO source driving to a standard dead-letter queue",
            true,
            policy(&standard, json!(min)),
            type_mismatch(&policy(&standard, json!(min))),
        ),
    ]
}

/// `SetQueueAttributes` stores a `RedrivePolicy` only where the document says what the
/// attribute documents, and leaves the queue's own policy exactly as it was otherwise.
#[tokio::test]
async fn set_queue_attributes_stores_only_a_redrive_policy_the_attribute_admits() {
    for (index, row) in content_rows().into_iter().enumerate() {
        let case = row.case;
        let router = emulator();
        create_targets(&router).await;
        let (url, baseline) = source_queue(&router, &queue_name("set-source", index, row.fifo), row.fifo).await;

        let (status, parsed) = call(
            &router,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": {"RedrivePolicy": row.policy}}),
        )
        .await;
        let stored = all_attributes(&router, &url)
            .await
            .get("RedrivePolicy")
            .cloned();
        match &row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected the policy {} to be accepted, got {status} with \
                     {parsed}",
                    row.policy
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(canonical(&row.policy).as_str()),
                    "case {case}: expected the accepted policy stored, got {stored:?}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&parsed).as_str(), parsed["message"].as_str()),
                    (400, "InvalidParameterValue", Some(expected.as_str())),
                    "case {case}: expected the policy {} refused with its own sentence, got \
                     {status} with {parsed}",
                    row.policy
                );
                assert_eq!(
                    stored.as_deref(),
                    Some(baseline.as_str()),
                    "case {case}: a refused write must leave the stored policy as it was, got \
                     {stored:?}"
                );
            }
        }
    }
}

/// `CreateQueue` judges the same document the same way, and a refused create leaves no queue
/// behind at all.
#[tokio::test]
async fn create_queue_stores_only_a_redrive_policy_the_attribute_admits() {
    for (index, row) in content_rows().into_iter().enumerate() {
        let case = row.case;
        let router = emulator();
        create_targets(&router).await;
        let name = queue_name("created", index, row.fifo);

        let mut attributes = if row.fifo {
            fifo_attributes(&[])
        } else {
            json!({})
        };
        attributes["RedrivePolicy"] = json!(row.policy);
        let (status, parsed) = create(&router, &name, &attributes).await;
        match &row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case}: expected the policy {} to be accepted, got {status} with \
                     {parsed}",
                    row.policy
                );
                let url = parsed["QueueUrl"].as_str().unwrap_or_default().to_string();
                let stored = all_attributes(&router, &url)
                    .await
                    .get("RedrivePolicy")
                    .cloned();
                assert_eq!(
                    stored.as_deref(),
                    Some(canonical(&row.policy).as_str()),
                    "case {case}: expected the accepted policy stored on the created queue, got \
                     {stored:?}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (status, error_code(&parsed).as_str(), parsed["message"].as_str()),
                    (400, "InvalidParameterValue", Some(expected.as_str())),
                    "case {case}: expected the policy {} refused with its own sentence, got \
                     {status} with {parsed}",
                    row.policy
                );
                let (status, parsed) =
                    call(&router, "GetQueueUrl", &json!({"QueueName": name})).await;
                assert_eq!(
                    (status, error_code(&parsed).as_str()),
                    (400, "QueueDoesNotExist"),
                    "case {case}: a refused create must leave no queue behind, got {status} with \
                     {parsed}"
                );
            }
        }
    }
}

/// The document the rows of the canonical table all mean, as the queue must report it: the
/// compact JSON of the two members the attribute documents, with the count as the NUMBER its
/// own documentation calls it ("the number of times a message is delivered to the source
/// queue before being moved to the dead-letter queue").
fn canonical(supplied: &str) -> String {
    let parsed: Value = serde_json::from_str(supplied).unwrap_or(Value::Null);
    let count = parsed
        .get("maxReceiveCount")
        .and_then(|count| {
            count
                .as_i64()
                .or_else(|| count.as_str().and_then(|raw| raw.trim().parse::<i64>().ok()))
        })
        .unwrap_or_default();
    let target = parsed
        .get("deadLetterTargetArn")
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({"deadLetterTargetArn": target, "maxReceiveCount": count}).to_string()
}

/// Every spelling of one `RedrivePolicy` reads back as the one canonical document — through
/// either write path — so a client that json-loads the attribute and compares the count
/// numerically, or re-submits what it read, sees one value.
#[tokio::test]
async fn one_redrive_policy_reads_back_in_one_rendering_whatever_its_spelling() {
    let target = expected_arn(STANDARD_TARGET);
    let expected = format!(r#"{{"deadLetterTargetArn":"{target}","maxReceiveCount":7}}"#);
    // (case, the document the write submits) — each says the same thing about the queue: the
    // members in either order, with and without whitespace, and the count as a number, as a
    // string of digits and as digits carrying space.
    let rows: Vec<(&str, String)> = vec![
        ("the canonical document itself, re-submitted", expected.clone()),
        (
            "the count quoted",
            format!(r#"{{"deadLetterTargetArn":"{target}","maxReceiveCount":"7"}}"#),
        ),
        (
            "the members in the other order",
            format!(r#"{{"maxReceiveCount":7,"deadLetterTargetArn":"{target}"}}"#),
        ),
        (
            "whitespace around every member",
            format!(r#"{{ "deadLetterTargetArn" : "{target}" , "maxReceiveCount" : "7" }}"#),
        ),
        (
            "the document written over several lines",
            serde_json::to_string_pretty(
                &json!({"deadLetterTargetArn": target, "maxReceiveCount": 7}),
            )
            .expect("the document serializes"),
        ),
        (
            "a quoted count carrying surrounding space",
            format!(r#"{{"deadLetterTargetArn":"{target}","maxReceiveCount":" 7 "}}"#),
        ),
    ];

    for (index, (case, supplied)) in rows.iter().enumerate() {
        let router = emulator();
        create_targets(&router).await;

        // The CREATE path.
        let created = create_ok(
            &router,
            &format!("spelling-created-{index}"),
            &json!({"RedrivePolicy": supplied}),
        )
        .await;
        let stored = all_attributes(&router, &created)
            .await
            .get("RedrivePolicy")
            .cloned();
        assert_eq!(
            stored.as_deref(),
            Some(expected.as_str()),
            "case {case}: CreateQueue must report the policy {supplied} in one rendering, got \
             {stored:?}"
        );

        // The SET path, onto a queue carrying no policy at all.
        let url = create_ok(&router, &format!("spelling-set-{index}"), &json!({})).await;
        let set = call(
            &router,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": {"RedrivePolicy": supplied}}),
        )
        .await;
        assert_eq!(
            set.0, 200,
            "case {case}: expected the policy {supplied} to be accepted, got {} with {}",
            set.0, set.1
        );
        let stored = all_attributes(&router, &url)
            .await
            .get("RedrivePolicy")
            .cloned();
        assert_eq!(
            stored.as_deref(),
            Some(expected.as_str()),
            "case {case}: SetQueueAttributes must report the policy {supplied} in one rendering, \
             got {stored:?}"
        );
    }
}

/// The permission document is rendered the same way: a `RedriveAllowPolicy` reads back as the
/// compact JSON of what it was written with, whatever spacing the caller used.
#[tokio::test]
async fn one_redrive_allow_policy_reads_back_in_one_rendering_whatever_its_spelling() {
    let source = expected_arn("permission-source");
    // (case, the document the write submits, the document it must read back as) — the rows
    // walk both permission forms the attribute documents that carry a document at all: one
    // with only `redrivePermission`, one that also lists `sourceQueueArns`.
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a permission naming no source queue, spaced",
            r#"{ "redrivePermission" : "denyAll" }"#.to_string(),
            r#"{"redrivePermission":"denyAll"}"#.to_string(),
        ),
        (
            "a permission listing one source queue, over several lines",
            serde_json::to_string_pretty(
                &json!({"redrivePermission": "byQueue", "sourceQueueArns": [source.clone()]}),
            )
            .expect("the document serializes"),
            format!(r#"{{"redrivePermission":"byQueue","sourceQueueArns":["{source}"]}}"#),
        ),
        (
            "a permission in the order the members are not documented in",
            format!(
                r#"{{"sourceQueueArns": ["{source}"], "redrivePermission": "byQueue"}}"#
            ),
            format!(r#"{{"redrivePermission":"byQueue","sourceQueueArns":["{source}"]}}"#),
        ),
    ];

    for (index, (case, supplied, expected)) in rows.iter().enumerate() {
        let router = emulator();
        let url = create_ok(
            &router,
            &format!("permission-{index}"),
            &json!({"RedriveAllowPolicy": supplied}),
        )
        .await;
        let stored = all_attributes(&router, &url)
            .await
            .get("RedriveAllowPolicy")
            .cloned();
        assert_eq!(
            stored.as_deref(),
            Some(expected.as_str()),
            "case {case}: CreateQueue must report the permission {supplied} in one rendering, got \
             {stored:?}"
        );

        let (status, parsed) = call(
            &router,
            "SetQueueAttributes",
            &json!({"QueueUrl": url, "Attributes": {"RedriveAllowPolicy": supplied}}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the permission {supplied} to be accepted, got {status} with \
             {parsed}"
        );
        let stored = all_attributes(&router, &url)
            .await
            .get("RedriveAllowPolicy")
            .cloned();
        assert_eq!(
            stored.as_deref(),
            Some(expected.as_str()),
            "case {case}: SetQueueAttributes must report the permission {supplied} in one \
             rendering, got {stored:?}"
        );
    }
}
