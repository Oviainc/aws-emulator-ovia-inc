// SPDX-License-Identifier: CC0-1.0

//! The `EventTargets` rule of an eventbridge rule template, as BOTH operations
//! that write that member answer it, end to end over the restJson1 wire.
//!
//! `CreateEventBridgeRuleTemplate` and `UpdateEventBridgeRuleTemplate` write the
//! same member of the same resource, so one target list is admitted, or refused
//! with one status, one code and one message, whichever of them the request went
//! to. Every row below is driven through both, and the two answers are compared
//! with each other as well as with the rule — so a list the create admits can
//! never be one the update refuses, and the reverse.
//!
//! Every test drives the service through the shared `wire` harness — real HTTP
//! requests dispatched through `MultiServiceRouter` — so what is asserted is the
//! emulator's wire behaviour, not an internal function's return value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::member;
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT, DEFAULT_REGION as REGION};

/// `CreateEventBridgeRuleTemplate`'s collection, and the one
/// `GetEventBridgeRuleTemplate` and `UpdateEventBridgeRuleTemplate` address a
/// template inside (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplate,UpdateEventBridgeRuleTemplate")`:
/// POST /prod/eventbridge-rule-templates and PATCH
/// /prod/eventbridge-rule-templates/{Identifier}).
const TEMPLATES: &str = "/prod/eventbridge-rule-templates";

/// The group collection every template here is created inside
/// (`smithy_query(query="operation",
/// operation="CreateEventBridgeRuleTemplateGroup")`).
const GROUPS: &str = "/prod/eventbridge-rule-template-groups";

/// The most targets one template may carry:
/// `EventBridgeRuleTemplateSummary$EventTargetCount` ("The number of targets
/// configured to send matching events") targets `__integerMax5`
/// (`smithy_query(query="shape", shape="EventBridgeRuleTemplateSummary")`).
const MAX_TARGETS: usize = 5;

/// The longest target `Arn` an `EventTargets` entry may carry, from
/// `EventBridgeRuleTemplateTarget$Arn`'s target shape
/// `__stringMin1Max2048PatternArn` (`smithy_query(query="shape",
/// shape="EventBridgeRuleTemplateTarget")`).
const MAX_TARGET_ARN: usize = 2048;

/// One `EventBridgeRuleTemplateEventType` value, which every create here needs
/// (`smithy_query(query="shape", shape="EventBridgeRuleTemplateEventType")`).
const EVENT_TYPE: &str = "MEDIALIVE_CHANNEL_STATE_CHANGE";

/// The target the update subject is created with, so a REFUSED update can be
/// shown to have left the stored targets as they were.
fn original_target() -> Value {
    json!([{ "arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:original") }])
}

/// An SNS topic ARN of `length` characters — one of the two kinds
/// `EventBridgeRuleTemplateTarget$Arn` documents ("Target ARNs must be either an
/// SNS topic or CloudWatch log group") — for the rows that walk the length
/// bound.
fn sns_arn(length: usize) -> String {
    let prefix = format!("arn:aws:sns:{REGION}:{ACCOUNT}:");
    format!("{prefix}{}", "t".repeat(length.saturating_sub(prefix.len())))
}

/// `count` distinct SNS topic ARNs, for the rows that walk the count ceiling.
fn sns_targets(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:topic-{index}") }))
            .collect(),
    )
}

/// The message a target list carrying more than [`MAX_TARGETS`] entries is
/// refused with, as a template over the count the request carried.
fn too_many(carried: usize) -> String {
    format!("eventTargets must carry at most {MAX_TARGETS} targets, but carried {carried}")
}

/// The message a target naming neither of the two documented ARN kinds is
/// refused with, as a template over the ARN the request carried.
fn wrong_kind(arn: &str) -> String {
    format!(
        "eventTargets.arn must be the ARN of an SNS topic or a CloudWatch log group, but was {arn}"
    )
}

/// The message a target ARN outside the length its target shape names is
/// refused with, as a template over the length the request carried.
fn wrong_length(characters: usize) -> String {
    format!("eventTargets.arn must be between 1 and {MAX_TARGET_ARN} characters long, but was {characters}")
}

/// What the rule says about one target list.
#[derive(Debug)]
enum Expected {
    /// The list is stored, and reported back as the request listed it.
    Admitted,
    /// The list is refused as a client error carrying exactly this message.
    Refused(String),
}

/// One row of the table: a target list and what the rule says about it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    targets: Value,
    expected: Expected,
}

/// One eventbridge rule template group, answering its name (which is also an
/// identifier a template's `GroupIdentifier` may name).
async fn create_group(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let (status, group) = call(router, "POST", GROUPS, Some(json!({ "name": name }))).await;
    assert_eq!(
        status, 201,
        "case {case:?}: setup: expected the group {name:?} to be created, got {status}: {group}"
    );
    name.to_string()
}

/// One `CreateEventBridgeRuleTemplate` carrying `targets` as its `EventTargets`.
async fn create_with_targets(
    router: &MultiServiceRouter,
    group: &str,
    name: &str,
    targets: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        TEMPLATES,
        Some(json!({
            "name": name,
            "groupIdentifier": group,
            "eventType": EVENT_TYPE,
            "eventTargets": targets,
        })),
    )
    .await
}

/// One `UpdateEventBridgeRuleTemplate` carrying `targets` as its `EventTargets`.
async fn update_targets(
    router: &MultiServiceRouter,
    identifier: &str,
    targets: &Value,
) -> (u16, Value) {
    call(
        router,
        "PATCH",
        &format!("{TEMPLATES}/{identifier}"),
        Some(json!({ "eventTargets": targets })),
    )
    .await
}

/// `GetEventBridgeRuleTemplate`, the reader both writes are proven through.
async fn get(router: &MultiServiceRouter, identifier: &str) -> (u16, Value) {
    call(router, "GET", &format!("{TEMPLATES}/{identifier}"), None).await
}

/// The `eventTargets` member a response reports for a template whose stored
/// targets are `requested`: the list as it was given, or an ABSENT member when
/// the template carries none, since an empty list would claim a target set the
/// caller never supplied.
fn reported_targets(requested: &Value) -> Value {
    if requested.as_array().is_some_and(|targets| targets.is_empty()) {
        Value::Null
    } else {
        requested.clone()
    }
}

/// Assert one refusal: the status, the code and the exact message.
fn assert_refused(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected status 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        "BadRequestException",
        "case {case:?}: expected BadRequestException, got {body}"
    );
    assert_eq!(
        error_message(body),
        expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {:?}",
        error_message(body)
    );
}

/// Rule: a template's `EventTargets` is admitted only when it carries at most
/// the number of targets the model can count, each target ARN within the length
/// its target shape names and naming one of the two resource kinds the member
/// documents; a list outside that rule is refused as a client error with one
/// message, and the rule is the member's, so it does not depend on which
/// operation wrote it.
///
/// The rows span what the rule generalises over: the COUNT (none, one, two,
/// the ceiling, one past it, well past it), the ARN KIND (SNS topic, CloudWatch
/// log group, both together, another partition's topic, and ARNs of services
/// the member does not document, at the first and at a later position), and the
/// ARN LENGTH (at the bound, one past it, and the empty string below the
/// minimum). Each row is driven through the create AND through the update, and
/// the two answers are compared with each other.
#[tokio::test]
async fn one_event_targets_rule_answers_the_create_and_the_update_alike() {
    let past_arn_bound = sns_arn(MAX_TARGET_ARN + 1);
    let s3_bucket = format!("arn:aws:s3:::bucket-{ACCOUNT}");
    let lambda_function = format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:notify");
    let log_group = format!("arn:aws:logs:{REGION}:{ACCOUNT}:log-group:/aws/medialive:*");

    let rows: [Row; 13] = [
        Row {
            case: "no targets at all",
            targets: json!([]),
            expected: Expected::Admitted,
        },
        Row {
            case: "one SNS topic",
            targets: sns_targets(1),
            expected: Expected::Admitted,
        },
        Row {
            case: "one CloudWatch log group",
            targets: json!([{ "arn": log_group }]),
            expected: Expected::Admitted,
        },
        Row {
            case: "both documented kinds together",
            targets: json!([
                { "arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:alerts") },
                { "arn": log_group },
            ]),
            expected: Expected::Admitted,
        },
        Row {
            // The kind is the ARN's SERVICE segment, so a topic in another
            // partition is the same kind of target.
            case: "an SNS topic in another partition",
            targets: json!([{ "arn": "arn:aws-cn:sns:cn-north-1:000000000000:alerts" }]),
            expected: Expected::Admitted,
        },
        Row {
            case: "as many targets as the model counts",
            targets: sns_targets(MAX_TARGETS),
            expected: Expected::Admitted,
        },
        Row {
            case: "a target ARN at the length bound",
            targets: json!([{ "arn": sns_arn(MAX_TARGET_ARN) }]),
            expected: Expected::Admitted,
        },
        Row {
            case: "one target past what the model counts",
            targets: sns_targets(MAX_TARGETS + 1),
            expected: Expected::Refused(too_many(MAX_TARGETS + 1)),
        },
        Row {
            case: "well past what the model counts",
            targets: sns_targets(MAX_TARGETS * 2),
            expected: Expected::Refused(too_many(MAX_TARGETS * 2)),
        },
        Row {
            case: "an S3 bucket, which is neither documented kind",
            targets: json!([{ "arn": s3_bucket.clone() }]),
            expected: Expected::Refused(wrong_kind(&s3_bucket)),
        },
        Row {
            case: "an SNS topic followed by a Lambda function",
            targets: json!([
                { "arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:alerts") },
                { "arn": lambda_function.clone() },
            ]),
            expected: Expected::Refused(wrong_kind(&lambda_function)),
        },
        Row {
            case: "a target ARN one past the length bound",
            targets: json!([{ "arn": past_arn_bound }]),
            expected: Expected::Refused(wrong_length(MAX_TARGET_ARN + 1)),
        },
        Row {
            case: "a target ARN below the minimum length",
            targets: json!([{ "arn": "" }]),
            expected: Expected::Refused(wrong_length(0)),
        },
    ];

    for Row {
        case,
        targets,
        expected,
    } in rows
    {
        let router = medialive();
        let group = create_group(&router, case, "targets-group").await;

        // The update subject exists before the row runs, carrying a target the
        // rule admits, so a refused update can be shown to have written
        // nothing.
        let (status, subject) =
            create_with_targets(&router, &group, "update-subject", &original_target()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: setup: expected the update subject to be created, got {status}: {subject}"
        );
        let subject_id = subject["id"].as_str().unwrap_or_default().to_string();

        let (create_status, created) =
            create_with_targets(&router, &group, "created", &targets).await;
        let (update_status, updated) = update_targets(&router, &subject_id, &targets).await;

        match expected {
            Expected::Admitted => {
                assert_eq!(
                    create_status, 201,
                    "case {case:?}: expected the create to answer 201, got {create_status}: {created}"
                );
                assert_eq!(
                    update_status, 200,
                    "case {case:?}: expected the update to answer 200, got {update_status}: {updated}"
                );
                let reported = reported_targets(&targets);
                assert_eq!(
                    member(&created, "eventTargets"),
                    reported,
                    "case {case:?}: expected the create to report the targets the request listed, got {:?}",
                    created["eventTargets"]
                );
                assert_eq!(
                    member(&updated, "eventTargets"),
                    reported,
                    "case {case:?}: expected the update to report the targets the request listed, got {:?}",
                    updated["eventTargets"]
                );

                // The round trip: the reader reports what each write stored,
                // and reports both templates alike.
                for identifier in [created["id"].as_str().unwrap_or_default(), &subject_id] {
                    let (status, read) = get(&router, identifier).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected {identifier} to be readable, got {status}: {read}"
                    );
                    assert_eq!(
                        member(&read, "eventTargets"),
                        reported,
                        "case {case:?}: expected {identifier} to report the stored targets, got {:?}",
                        read["eventTargets"]
                    );
                }
            }
            Expected::Refused(message) => {
                assert_refused(
                    &format!("create with {case}"),
                    create_status,
                    &created,
                    &message,
                );
                assert_refused(
                    &format!("update with {case}"),
                    update_status,
                    &updated,
                    &message,
                );
                assert_eq!(
                    error_message(&created),
                    error_message(&updated),
                    "case {case:?}: expected the create and the update to refuse the same list alike, got {:?} and {:?}",
                    error_message(&created),
                    error_message(&updated)
                );

                // A refused write stores nothing: the refused create left no
                // template behind, and the refused update left the subject's
                // stored targets as they were.
                let (status, read) = get(&router, "created").await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the refused create to have stored no template, got {status}: {read}"
                );
                let (status, read) = get(&router, &subject_id).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the update subject to still be readable, got {status}: {read}"
                );
                assert_eq!(
                    member(&read, "eventTargets"),
                    original_target(),
                    "case {case:?}: expected the refused update to leave the stored targets alone, got {:?}",
                    read["eventTargets"]
                );
            }
        }
    }
}
