// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT A GRANT PAST THE POLICY STATEMENT LIMIT SAYS, driven over the
//! wire (`POST /` with `X-Amz-Target: AmazonSQS.<OperationName>`) through the shared harness
//! in [`super::support`].
//!
//! The rule under test: **`AddPermission` refuses a grant that would take the queue's policy
//! past the statements one policy may hold with error code `OverLimit` and the message
//! `<n> Statements were found, maximum allowed is <limit>. ` — where `<n>` is the number of
//! statements the REQUEST would leave the policy with and `<limit>` the published ceiling —
//! and the queue keeps the policy it had.**
//!
//! The pair of numbers is the whole of what the refusal reports: a client that classifies
//! this refusal by its message, or shows it to an operator, has the attempted count and the
//! ceiling and nothing about which queue was asked, which is what AWS reports.
//!
//! The table spans what the rule generalises over rather than one example: how the statements
//! the policy already holds got there (one grant at a time, or one uploaded document), and
//! HOW MANY it holds — at the ceiling and well past it — so the count the message reports is
//! the request's own and not the ceiling restated.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::state::MAX_POLICY_STATEMENTS;
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    all_attributes, call, create_ok, emulator, error_code, expected_arn, ok, OTHER_ACCOUNT,
};

/// The statements one queue policy may hold, spelled out here rather than imported, so a
/// wrong constant in the service cannot make a row pass. Source: the ceiling AWS publishes
/// for a queue policy — "The maximum limit is 8,192 bytes, 20 statements, 50 principals, or
/// 10 conditions" — which is what `OverLimit` means for this call ("AddPermission returns
/// this error if the maximum number of permissions for the queue is reached").
const PUBLISHED_STATEMENT_LIMIT: usize = 20;

/// The refusal class a published limit on what a queue may hold carries, and its status.
const OVER_LIMIT: (u16, &str) = (403, "OverLimit");

/// The policy language version a queue policy declares.
const POLICY_VERSION: &str = "2012-10-17";

/// The one sentence Amazon SQS refuses an over-large policy with, spelled out here rather
/// than imported — including the space it ends with, which is part of its fixed text.
fn over_limit_message(would_hold: usize) -> String {
    format!("{would_hold} Statements were found, maximum allowed is {PUBLISHED_STATEMENT_LIMIT}. ")
}

/// The `message` member of a refusal, as awsJson renders it.
fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A policy document for the queue at `arn` carrying `count` statements, each labelled
/// apart.
fn policy_with(arn: &str, count: usize) -> String {
    let statements: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "Sid": format!("uploaded-{index}"),
                "Effect": "Allow",
                "Principal": {"AWS": format!("arn:aws:iam::{OTHER_ACCOUNT}:root")},
                "Action": "SQS:SendMessage",
                "Resource": arn,
            })
        })
        .collect();
    json!({
        "Version": POLICY_VERSION,
        "Id": format!("{arn}/uploaded"),
        "Statement": statements,
    })
    .to_string()
}

/// Grant one permission on the queue at `url`, answering `(status, body)`.
async fn grant(router: &MultiServiceRouter, url: &str, label: &str) -> (u16, Value) {
    call(
        router,
        "AddPermission",
        &json!({
            "QueueUrl": url,
            "Label": label,
            "AWSAccountIds": [OTHER_ACCOUNT],
            "Actions": ["SendMessage"],
        }),
    )
    .await
}

/// The `Policy` the queue at `url` reports, or `None` when it carries none.
async fn policy_of(router: &MultiServiceRouter, url: &str) -> Option<String> {
    all_attributes(router, url).await.get("Policy").cloned()
}

/// A grant onto a policy already holding the statements it may hold is refused with the
/// count the request would have produced and the published ceiling, whichever way the policy
/// reached that size — and the policy is left exactly as it was.
#[tokio::test]
async fn a_grant_past_the_statement_limit_reports_the_attempted_count_and_the_limit() {
    assert_eq!(
        MAX_POLICY_STATEMENTS, PUBLISHED_STATEMENT_LIMIT,
        "case the published ceiling: expected the limit AWS publishes, got \
         {MAX_POLICY_STATEMENTS}"
    );
    // (case, how many statements the policy already holds, whether they were uploaded as one
    // document rather than granted one at a time). A policy may be made larger than the
    // ceiling by an upload, which is what makes the attempted count vary.
    let rows: &[(&str, usize, bool)] = &[
        ("a policy grown to the ceiling one grant at a time", PUBLISHED_STATEMENT_LIMIT, false),
        ("a policy uploaded at the ceiling", PUBLISHED_STATEMENT_LIMIT, true),
        ("a policy uploaded one statement past the ceiling", PUBLISHED_STATEMENT_LIMIT + 1, true),
        ("a policy uploaded well past the ceiling", PUBLISHED_STATEMENT_LIMIT + 5, true),
    ];

    for (index, (case, held, uploaded)) in rows.iter().enumerate() {
        let router = emulator();
        let name = format!("statement-limit-{index}");
        let url = create_ok(&router, &name, &json!({})).await;
        let arn = expected_arn(&name);

        if *uploaded {
            let _ = ok(
                &router,
                "SetQueueAttributes",
                json!({"QueueUrl": url, "Attributes": {"Policy": policy_with(&arn, *held)}}),
            )
            .await;
        } else {
            for statement in 0..*held {
                let (status, parsed) = grant(&router, &url, &format!("granted-{statement}")).await;
                assert_eq!(
                    status, 200,
                    "case {case}: expected grant {statement} accepted, got {status} with {parsed}"
                );
            }
        }
        let before = policy_of(&router, &url).await;

        let (status, parsed) = grant(&router, &url, "one-past-the-limit").await;
        assert_eq!(
            (status, error_code(&parsed).as_str(), message_of(&parsed)),
            (
                OVER_LIMIT.0,
                OVER_LIMIT.1,
                over_limit_message(held + 1)
            ),
            "case {case}: expected the attempted count and the limit, got {status} with {parsed}"
        );
        assert!(
            !message_of(&parsed).contains(&name),
            "case {case}: the refusal must not name the queue, got {:?}",
            message_of(&parsed)
        );
        assert_eq!(
            policy_of(&router, &url).await,
            before,
            "case {case}: a refused grant must leave the policy as it was"
        );
    }
}

/// A grant that takes the policy exactly TO the ceiling is served: the refusal is about the
/// statement the request would add, not about the policy being near the limit.
#[tokio::test]
async fn a_grant_that_fills_the_statement_limit_is_served() {
    let router = emulator();
    let url = create_ok(&router, "statement-limit-filled", &json!({})).await;
    for statement in 0..PUBLISHED_STATEMENT_LIMIT {
        let (status, parsed) = grant(&router, &url, &format!("granted-{statement}")).await;
        assert_eq!(
            status, 200,
            "case the grant that fills the limit: expected grant {statement} accepted, got \
             {status} with {parsed}"
        );
    }
    let policy = policy_of(&router, &url)
        .await
        .expect("case the grant that fills the limit: the queue reports no Policy");
    let statements = serde_json::from_str::<Value>(&policy)
        .ok()
        .and_then(|policy| policy.get("Statement").and_then(Value::as_array).cloned())
        .map(|statements| statements.len());
    assert_eq!(
        statements,
        Some(PUBLISHED_STATEMENT_LIMIT),
        "case the grant that fills the limit: expected a policy at the ceiling, got {policy}"
    );
}
