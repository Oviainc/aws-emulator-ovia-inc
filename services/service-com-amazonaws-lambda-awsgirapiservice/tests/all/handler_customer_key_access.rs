// SPDX-License-Identifier: CC0-1.0

//! WHICH DOCUMENTS in force on a function's EXECUTION ROLE decide what this service does on
//! that function's behalf — the half of that rule this target asserts without running a
//! handler.
//!
//! THE RULE: the documents in force on an IAM principal are its inline policies AND the
//! default version of every managed policy ATTACHED to it, whether this account owns that
//! policy or AWS publishes it — and the gates that decide what an execution role may do read
//! them through IAM's own collection of a principal's policies. So a role whose grant arrives
//! through an attachment is granted, and one whose attachment states nothing about the call is
//! refused exactly as a role with no policy at all is.
//!
//! The gate this suite drives it through is the one that reads an execution role's documents
//! with no invocation: `CreateEventSourceMapping`, which is accepted only where the role may
//! make the reads a poller of the named source makes
//! (`tests/all/event_source_mapping_role_access.rs` owns the rest of that rule, and this suite
//! borrows its harness so the roles are made with IAM's own operations over the same engine).
//! The rule as a CALLER meets it — a role session reading a resource sealed under a customer
//! managed key — belongs to the crates that own that decision and is asserted there:
//! `service-com-amazonaws-kms-trentservice`'s `tests/all/principal_attached_policy_authority.rs`
//! and Secrets Manager's `tests/all/secret_customer_key_caller_authority.rs`.
//!
//! The rows differ along WHERE the role's authority comes from: nothing but its log writes, an
//! attached AWS-published policy that carries the reads, and an attached AWS-published policy
//! that carries none of them.

use serde_json::{json, Value};

use crate::event_source_mapping_role_access::{
    assert_refused, create_body, create_function_as, given_role, iam, router_with_iam,
    QUEUE_READS,
};
use crate::event_source_mappings::{call, create_mapping, get_mapping};

/// The queue a row's mapping is made on, which is the source the role's reads are judged
/// against.
const QUEUE_SOURCE: &str = "arn:aws:sqs:us-east-1:000000000000:attached-authority-queue";

/// Where `DeleteEventSourceMapping` is bound.
const MAPPINGS_URI: &str = "/2015-03-31/event-source-mappings";

/// How AWS words the refusal of a role that may not make one call on a queue — the first read
/// a poller of a queue makes, which is the one the refusal names.
fn queue_refusal() -> String {
    format!(
        "The function execution role does not have permissions to call {} on SQS",
        QUEUE_READS[0]
    )
}

/// The statement every role below carries whatever else it does: the log writes a function
/// makes, which say nothing about any event source.
fn logs_only() -> Value {
    json!({
        "Effect": "Allow",
        "Action": ["logs:CreateLogGroup", "logs:CreateLogStream", "logs:PutLogEvents"],
        "Resource": "*",
    })
}

/// The ARN of one policy AWS publishes, read out of IAM's own catalog of those documents
/// (`service_com_amazonaws_iam_awsidentitymanagementv20100508::state::AWS_MANAGED_POLICIES`),
/// so a row attaches a policy IAM really publishes and the document in force is AWS's own.
fn aws_managed_policy_arn(policy_name: &str) -> String {
    service_com_amazonaws_iam_awsidentitymanagementv20100508::state::aws_managed_policy_named(
        policy_name,
    )
    .unwrap_or_else(|| panic!("IAM publishes the managed policy {policy_name:?}"))
    .arn()
}

/// One row: the AWS-published policy the role carries attached, and whether the mapping is
/// accepted.
struct Row {
    case: &'static str,
    attached: Option<&'static str>,
    accepted: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "no attachment at all, so only the role's own log writes are in force",
        attached: None,
        accepted: false,
    },
    Row {
        // `AmazonSQSFullAccess` carries every call a poller of a queue makes, so the grant
        // arrives entirely through the attachment.
        case: "an attached AWS-published policy that carries the queue's reads",
        attached: Some("AmazonSQSFullAccess"),
        accepted: true,
    },
    Row {
        // The attachment every Lambda execution role carries states the function's own log
        // writes and nothing of any event source.
        case: "an attached AWS-published policy that carries none of them",
        attached: Some("AWSLambdaBasicExecutionRole"),
        accepted: false,
    },
];

/// RULE: a policy ATTACHED to the execution role is in force on it exactly as an inline one
/// is — so the role whose attachment carries the poller's reads is accepted, and the role
/// whose attachment carries none of them is refused with the same sentence a role with no
/// policy earns.
#[tokio::test]
async fn an_attached_policy_is_in_force_on_what_an_execution_role_may_do() {
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let router = router_with_iam();
        let role_name = format!("attached-authority-{index}");
        let role = given_role(&router, case, &role_name, Some(json!([logs_only()]))).await;
        if let Some(policy_name) = row.attached {
            let policy_arn = aws_managed_policy_arn(policy_name);
            let (status, attached) = iam(
                &router,
                "AttachRolePolicy",
                &[("RoleName", &role_name), ("PolicyArn", &policy_arn)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected AttachRolePolicy {policy_arn} to answer 200, got \
                 {status}: {attached}"
            );
        }
        let function = format!("attached-authority-probe-{index}");
        create_function_as(&router, case, &function, &role).await;

        let answer = create_mapping(&router, &create_body(&function, QUEUE_SOURCE)).await;
        if !row.accepted {
            assert_refused(case, &answer, &queue_refusal());
            continue;
        }
        assert_eq!(
            answer.status,
            202,
            "case {case:?}: expected HTTP 202, got {} {}",
            answer.status,
            answer.text()
        );
        let uuid = answer
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", answer.text()));
        // Round trip: the accepted mapping is one this emulator holds, on the source it was
        // made for.
        let reading = get_mapping(&router, &uuid).await;
        assert_eq!(
            (reading.status, reading.string("EventSourceArn")),
            (200, Some(QUEUE_SOURCE.to_string())),
            "case {case:?}: the mapping must read back on {QUEUE_SOURCE}, got {} {}",
            reading.status,
            reading.text()
        );
        // The accepted mapping is given up again: an enabled mapping is polled for as long as
        // it is held, and this suite is about the decision that admitted it rather than about
        // what its poller then does.
        let deleted = call(
            &router,
            "DELETE",
            &format!("{MAPPINGS_URI}/{uuid}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            deleted.status,
            202,
            "case {case:?}: expected HTTP 202 from DeleteEventSourceMapping, got {} {}",
            deleted.status,
            deleted.text()
        );
    }
}
