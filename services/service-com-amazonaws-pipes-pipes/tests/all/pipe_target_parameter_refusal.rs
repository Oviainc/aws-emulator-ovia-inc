// SPDX-License-Identifier: CC0-1.0

//! What a pipe is refused with when its TARGET resource requires a target parameter the
//! configuration does not supply, end to end over the wire.
//!
//! One rule, and it is about the WORDING of that refusal as much as about which
//! configurations are refused: a target parameter the target resource requires and the
//! configuration does not supply is refused with `ValidationException` (400) whose single
//! entry — in the exception's `ValidationErrorFields` message document and in the modelled
//! `fieldList` alike — names the `TargetParameters` member that had to carry the
//! configuration (`TargetParameters.SqsQueueParameters` for an Amazon SQS queue target) and
//! states that a required target parameter is missing. Neither part is derived from the
//! request, so a row that changes the queue's name, account or region changes nothing in
//! the refusal.
//!
//! The one such requirement this service has is the message group a FIFO queue orders its
//! messages by (`PipeTargetSqsQueueParameters.MessageGroupId`, optional in the model
//! because it is a property of the queue the `Target` names — a `.fifo` queue — that makes
//! it required), so this is where that rule's whole table lives: both which configurations
//! it refuses and what the refusal says, over both writes that can configure a pipe.
//! `tests/all/pipe_configuration_rules.rs` owns the other two cross-resource rules and the
//! [`assert_refused`] this suite reaches for rather than copying.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch` — the one path a
//! socket-served request takes — reusing the harness `pipe_lifecycle`, `pipe_update` and
//! `pipe_configuration_rules` already own rather than copying it.

use serde_json::{json, Value};

use crate::pipe_configuration_rules::assert_refused;
use crate::pipe_lifecycle::{create, describe, router, PAST_SETTLE};
use crate::pipe_update::{create_body, update};

const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-role";
const SOURCE_SQS: &str = "arn:aws:sqs:us-west-2:000000000000:orders";

/// The member path the refusal names: the `TargetParameters` member that configures an
/// Amazon SQS queue target. Source: the `SqsQueueParameters` member of
/// `PipeTargetParameters` in `smithy-model/com-amazonaws-pipes-pipes.json`, under the
/// request member that holds it.
const MISSING_FIELD: &str = "TargetParameters.SqsQueueParameters";

/// The sentence it states: fixed text, with nothing taken from the request.
const MISSING_MESSAGE: &str = "Missing required target parameter.";

/// `MessageGroupId`'s own `@length` maximum, from `PipeTargetSqsQueueParameters` in that
/// same model — a group id at the bound is a value the member admits.
const MESSAGE_GROUP_ID_MAX: usize = 100;

/// One row of the `CreatePipe` table: a target and the target parameters the request
/// carries for it.
struct Create {
    /// Also the pipe's name, so every row is independent.
    case: &'static str,
    target: String,
    /// The `TargetParameters` the request carries, or `Value::Null` for a request that
    /// carries none at all.
    target_parameters: Value,
    /// Whether this configuration is one the target resource's requirements admit.
    accepted: bool,
}

/// RULE: a target parameter the pipe's target resource requires and the configuration does
/// not supply is refused with `ValidationException` (400) naming the `TargetParameters`
/// member that had to carry it and stating that a required target parameter is missing —
/// the same entry in the message document and in `fieldList` — and the pipe is not stored;
/// a configuration that supplies it, and a target whose resource requires no such
/// parameter, is accepted and reads back.
///
/// The rows differ along what the rule generalises over: the form the configuration takes
/// (absent altogether, present without the queue's parameters, present with the queue's
/// other parameter, present with an empty value), the value that satisfies it (a literal
/// group, a group at the member's modelled maximum length, a group read from the event by
/// dynamic path), the identity of the queue (name, account, region — none of which the
/// refusal may echo), and the kind of target resource (a FIFO queue, a standard queue, a
/// standard queue whose name merely contains the suffix' letters, a function).
#[tokio::test]
async fn a_missing_required_target_parameter_is_refused_naming_the_member_that_had_to_carry_it() {
    let at_the_bound = "g".repeat(MESSAGE_GROUP_ID_MAX);
    let rows = vec![
        Create {
            case: "fifo-with-no-target-parameters",
            target: "arn:aws:sqs:us-west-2:000000000000:results.fifo".to_string(),
            target_parameters: Value::Null,
            accepted: false,
        },
        Create {
            case: "fifo-with-only-an-input-template",
            target: "arn:aws:sqs:us-west-2:000000000000:results.fifo".to_string(),
            target_parameters: json!({"InputTemplate": "<$.body>"}),
            accepted: false,
        },
        Create {
            case: "fifo-with-the-queues-other-parameter-only",
            target: "arn:aws:sqs:us-west-2:000000000000:results.fifo".to_string(),
            target_parameters: json!({
                "SqsQueueParameters": {"MessageDeduplicationId": "dedupe-1"},
            }),
            accepted: false,
        },
        Create {
            case: "fifo-with-an-empty-group",
            target: "arn:aws:sqs:us-west-2:000000000000:results.fifo".to_string(),
            target_parameters: json!({"SqsQueueParameters": {"MessageGroupId": ""}}),
            accepted: false,
        },
        Create {
            // Another account, another region, another queue name: the refusal states the
            // same member path and the same sentence, because neither comes from the
            // request.
            case: "another-accounts-fifo-queue-in-another-region",
            target: "arn:aws:sqs:eu-west-1:111122223333:eu-orders.fifo".to_string(),
            target_parameters: Value::Null,
            accepted: false,
        },
        Create {
            case: "fifo-with-a-group",
            target: "arn:aws:sqs:us-west-2:000000000000:results.fifo".to_string(),
            target_parameters: json!({"SqsQueueParameters": {"MessageGroupId": "orders"}}),
            accepted: true,
        },
        Create {
            case: "fifo-with-a-group-at-the-members-maximum-length",
            target: "arn:aws:sqs:us-west-2:000000000000:results.fifo".to_string(),
            target_parameters: json!({"SqsQueueParameters": {"MessageGroupId": at_the_bound}}),
            accepted: true,
        },
        Create {
            // A group read from each event rather than configured literally is still a
            // group the configuration supplies.
            case: "fifo-with-a-group-read-from-the-event",
            target: "arn:aws:sqs:us-west-2:000000000000:results.fifo".to_string(),
            target_parameters: json!({
                "SqsQueueParameters": {"MessageGroupId": "$.body.group"},
            }),
            accepted: true,
        },
        Create {
            case: "standard-queue-with-no-target-parameters",
            target: "arn:aws:sqs:us-west-2:000000000000:results".to_string(),
            target_parameters: Value::Null,
            accepted: true,
        },
        Create {
            case: "standard-queue-with-parameters-that-carry-no-group",
            target: "arn:aws:sqs:us-west-2:000000000000:results".to_string(),
            target_parameters: json!({"InputTemplate": "$.detail"}),
            accepted: true,
        },
        Create {
            // Only a queue whose name ENDS with the suffix is a FIFO queue, so this
            // standard queue requires nothing.
            case: "standard-queue-whose-name-only-contains-the-suffix",
            target: "arn:aws:sqs:us-west-2:000000000000:fifo-results".to_string(),
            target_parameters: Value::Null,
            accepted: true,
        },
        Create {
            case: "function-target-requires-no-queue-parameters",
            target: "arn:aws:lambda:us-west-2:000000000000:function:handler".to_string(),
            target_parameters: Value::Null,
            accepted: true,
        },
    ];

    for row in rows {
        let router = router();
        let mut extra = json!({});
        if !row.target_parameters.is_null() {
            extra["TargetParameters"] = row.target_parameters.clone();
        }
        let created = create(
            &router,
            row.case,
            create_body(SOURCE_SQS, &row.target, extra),
        )
        .await;

        if row.accepted {
            assert_eq!(
                created.status, 200,
                "case {:?}: this configuration must be accepted, got {created:?}",
                row.case
            );
            let described = describe(&router, row.case).await;
            assert_eq!(
                described.string("Target"),
                row.target,
                "case {:?}: the accepted pipe must read back its target, got {described:?}",
                row.case
            );
            assert_eq!(
                described.member("TargetParameters"),
                &reported_target_parameters(&row.target_parameters),
                "case {:?}: the accepted pipe must read back its target parameters, \
                 got {described:?}",
                row.case
            );
            continue;
        }

        assert_refused(row.case, &created, MISSING_FIELD, MISSING_MESSAGE);
        let described = describe(&router, row.case).await;
        assert_eq!(
            described.status, 404,
            "case {:?}: a refused CreatePipe must have stored no pipe, got {described:?}",
            row.case
        );
    }
}

/// One row of the `UpdatePipe` table: the configuration the pipe is created with and the
/// update that would leave it without the target parameter its target requires.
struct Update {
    /// Also the pipe's name.
    case: &'static str,
    created_target: &'static str,
    /// The `TargetParameters` the pipe is created with, or `Value::Null` for none.
    created_parameters: Value,
    /// The `UpdatePipe` body, which the pipe must be left unchanged by.
    update: Value,
}

/// RULE: the same requirement is checked against what an UPDATE would leave the pipe with,
/// so the refusal reads identically on that write — and a refused update stores nothing,
/// leaving the target and target parameters exactly as they were.
///
/// The rows differ along how the update would lose the parameter: by moving the pipe to a
/// FIFO target under target parameters that carry no group, by replacing the stored target
/// parameters with ones that carry none (the update replaces that object as a whole), and
/// by replacing them with an empty group.
#[tokio::test]
async fn an_update_that_would_leave_a_required_target_parameter_missing_is_refused() {
    const FIFO: &str = "arn:aws:sqs:us-west-2:000000000000:results.fifo";
    const STANDARD: &str = "arn:aws:sqs:us-west-2:000000000000:results";
    let rows = vec![
        Update {
            case: "moved-to-a-fifo-target-with-nothing-to-order-by",
            created_target: STANDARD,
            created_parameters: Value::Null,
            update: json!({"RoleArn": ROLE, "Target": FIFO}),
        },
        Update {
            case: "target-parameters-replaced-by-ones-without-the-group",
            created_target: FIFO,
            created_parameters: json!({"SqsQueueParameters": {"MessageGroupId": "orders"}}),
            update: json!({
                "RoleArn": ROLE,
                "TargetParameters": {"InputTemplate": "<$.body>"},
            }),
        },
        Update {
            case: "target-parameters-replaced-by-an-empty-group",
            created_target: FIFO,
            created_parameters: json!({"SqsQueueParameters": {"MessageGroupId": "orders"}}),
            update: json!({
                "RoleArn": ROLE,
                "TargetParameters": {"SqsQueueParameters": {"MessageGroupId": ""}},
            }),
        },
    ];

    for row in rows {
        let router = router();
        let mut extra = json!({});
        if !row.created_parameters.is_null() {
            extra["TargetParameters"] = row.created_parameters.clone();
        }
        let created = create(
            &router,
            row.case,
            create_body(SOURCE_SQS, row.created_target, extra),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the fixture pipe must be created, got {created:?}",
            row.case
        );
        tokio::time::sleep(PAST_SETTLE).await;

        let refused = update(&router, row.case, row.update.clone()).await;
        assert_refused(row.case, &refused, MISSING_FIELD, MISSING_MESSAGE);

        let described = describe(&router, row.case).await;
        assert_eq!(
            described.string("Target"),
            row.created_target,
            "case {:?}: a refused update must leave the stored Target as it was, \
             got {described:?}",
            row.case
        );
        assert_eq!(
            described.member("TargetParameters"),
            &reported_target_parameters(&row.created_parameters),
            "case {:?}: a refused update must leave the stored TargetParameters as they \
             were, got {described:?}",
            row.case
        );
    }
}

/// What a read reports for the `TargetParameters` a pipe was configured with: the
/// configuration itself, and the EMPTY object for a pipe configured with none — a read
/// reports the container whether the request mentioned it or not, which is the rule and the
/// table of `tests/all/pipe_reported_configuration.rs`.
fn reported_target_parameters(configured: &Value) -> Value {
    if configured.is_null() {
        return json!({});
    }
    configured.clone()
}
