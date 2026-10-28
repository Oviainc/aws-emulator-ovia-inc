// SPDX-License-Identifier: CC0-1.0

//! WHICH resources a pipe's execution role lets its stages reach, and what happens to the
//! records when it does not — end to end over the wire, with IAM and Amazon SQS mounted
//! beside pipes on one engine (the harness of `tests/all/pipe_metrics.rs` and
//! `tests/all/pipe_data_plane.rs`, reused rather than copied).
//!
//! `tests/all/pipe_metrics.rs` asserts WHICH STAGE fails when the role bars its call and
//! what the pipe then publishes about itself. The two rules here are the other half of the
//! same rule, and neither is covered there:
//!
//! 1. The decision is about the (action, RESOURCE) pair the call is made on, not about the
//!    action alone: a grant is read with IAM's wildcards, a grant that names another queue
//!    does not reach this one, a deny on the target resource refuses the delivery, and a
//!    call the role may not make leaves what it would have consumed or deleted on the
//!    source. The rows differ along what the rule generalises over — which stage's call the
//!    policy covers (receive, delete, send), and how it names the action and the resource
//!    (exactly, by wildcard, on another queue, by an explicit deny).
//! 2. The role is read from the pipe's record on every cycle, so `UpdatePipe` switches the
//!    identity a RUNNING pipe's deliveries are authorized by: delivery after the update is
//!    authorized by the NEW role, a pipe left on a role that may not reach the target it
//!    was just re-pointed at delivers nothing, and the record it could not move is still
//!    delivered once a role that may reach the target is supplied.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{
    collect_at_least, counted, given_queue, receive_now, send, sqs, wait_for_counted, QUIET,
};
use crate::pipe_lifecycle::{call, Reply, REGION};
use crate::pipe_metrics::{emulator, given_role_stating};

/// `TargetParameters.InputTemplate`, so a delivered message carries the record's own body
/// and a row can attribute it: `<$.body>` selects the `body` of the record envelope.
const BODY_TEMPLATE: &str = "<$.body>";

/// How long a delivery the pipe has to make is given before the row fails. Well above the
/// poll interval and the state-settle span, so a row fails only when the pipe did not
/// deliver at all.
const BUDGET: Duration = Duration::from_secs(25);

/// The visibility timeout the re-role source is set to, so a record the pipe took and then
/// could not deliver comes back within this suite's budget instead of after the Amazon SQS
/// default of 30 seconds.
const SHORT_VISIBILITY: &str = "1";

/// A role trusting EventBridge Pipes and carrying `statements` as its one inline policy;
/// answered with the ARN IAM persisted.
///
/// The role is written through the ONE path `tests/all/pipe_metrics.rs` writes one by;
/// every row here differs only in the statements, because what each asserts is which
/// RESOURCE a statement names.
async fn given_role(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    statements: Value,
) -> String {
    let arn = given_role_stating(router, name, SERVICE_PRINCIPAL, statements).await;
    assert!(
        !arn.is_empty(),
        "case {case:?}: CreateRole answered an empty Arn for {name}"
    );
    arn
}

/// Give the queue at `url` a one-second visibility timeout: "the length of time, in
/// seconds, that a message received from a queue will be invisible to other receiving
/// components" (`SetQueueAttributes$Attributes`).
async fn shorten_visibility(router: &MultiServiceRouter, case: &str, url: &str) {
    let (status, body) = sqs(
        router,
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": { "VisibilityTimeout": SHORT_VISIBILITY },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SetQueueAttributes answered {status} {body}"
    );
}

/// The attributes that together say how many records a queue HOLDS: the ones waiting on it
/// plus the ones a consumer has taken and not deleted ("messages that are in flight",
/// `ApproximateNumberOfMessagesNotVisible`) — which is what a record the pipe could not
/// move is one of.
const HELD_ATTRIBUTES: [&str; 2] = [
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesNotVisible",
];

/// How many records the queue at `url` still holds, through this crate's ONE queue-attribute
/// read.
async fn held(router: &MultiServiceRouter, url: &str) -> i64 {
    counted(router, url, &HELD_ATTRIBUTES).await
}

/// Wait until the queue at `url` holds `want` records, answering what it held last.
async fn wait_for_held(router: &MultiServiceRouter, url: &str, want: i64, budget: Duration) -> i64 {
    wait_for_counted(router, url, &HELD_ATTRIBUTES, want, budget).await
}

/// `CreatePipe`: an SQS source, an SQS target, the role the row names, and the template
/// that makes a delivered message carry the record's body.
async fn create_pipe(
    router: &MultiServiceRouter,
    name: &str,
    source_arn: &str,
    target_arn: &str,
    role_arn: &str,
) -> Reply {
    let body = json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": role_arn,
        "DesiredState": "RUNNING",
        "TargetParameters": { "InputTemplate": BODY_TEMPLATE },
    });
    call(
        router,
        "POST",
        &format!("/v1/pipes/{name}"),
        Some(body),
        REGION,
    )
    .await
}

/// `UpdatePipe` with the `RoleArn` every update carries and, when the caller supplies one,
/// a new `Target`.
async fn update_pipe(
    router: &MultiServiceRouter,
    name: &str,
    role_arn: &str,
    target_arn: Option<&str>,
) -> Reply {
    let mut body = json!({
        "RoleArn": role_arn,
        "TargetParameters": { "InputTemplate": BODY_TEMPLATE },
    });
    if let Some(target_arn) = target_arn {
        body["Target"] = json!(target_arn);
    }
    call(
        router,
        "PUT",
        &format!("/v1/pipes/{name}"),
        Some(body),
        REGION,
    )
    .await
}

async fn describe_pipe(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "GET", &format!("/v1/pipes/{name}"), None, REGION).await
}

/// The queues one row's policy statements are written against.
struct Resources {
    source: String,
    target: String,
    /// A queue the pipe does NOT deliver to, for the row whose grant names the wrong one.
    other: String,
}

/// One row of the resource-by-resource table.
struct Row {
    case: &'static str,
    /// The statements of the role's inline policy, written against the row's own queues.
    policy: fn(&Resources) -> Value,
    /// How many messages the target must receive.
    delivered: usize,
    /// Whether the row's record is delivered AGAIN once its invisibility on the source
    /// lapses, because the role may not delete it: the row then asserts at least
    /// [`Row::delivered`] rather than exactly that many, since how many copies have
    /// arrived by the time it looks depends only on how long the run took.
    redelivers: bool,
    /// How many records the SOURCE must still hold afterwards: a call the role may not
    /// make does not happen, so nothing it would have consumed or deleted leaves the queue.
    left_on_source: i64,
}

/// Every Amazon SQS call a pipe with an SQS source and an SQS target makes, allowed on
/// exactly the resource each is made on.
fn exact_grant(arns: &Resources) -> Value {
    json!([
        {
            "Effect": "Allow",
            "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
            "Resource": arns.source,
        },
        { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.target },
    ])
}

/// RULE: each stage's call is decided for the (action, resource) pair it is made on, read
/// with IAM's own wildcards and its deny-wins order; a call the policies in force on the
/// pipe's role do not allow is not made, so what it would have consumed or deleted stays
/// on the source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stage_reaches_only_the_resources_its_role_allows_it_on() {
    let rows = [
        Row {
            case: "every call allowed on exactly the resource it is made on",
            policy: exact_grant,
            delivered: 1,
            redelivers: false,
            left_on_source: 0,
        },
        Row {
            case: "the send allowed by an action wildcard and the queue by a resource one",
            policy: |arns| {
                json!([
                    {
                        "Effect": "Allow",
                        "Action": "sqs:*Message",
                        "Resource": arns.source,
                    },
                    {
                        "Effect": "Allow",
                        "Action": "sqs:Send*",
                        "Resource": format!("{}*", arns.target),
                    },
                ])
            },
            delivered: 1,
            redelivers: false,
            left_on_source: 0,
        },
        Row {
            case: "the send allowed on another queue only",
            policy: |arns| {
                json!([
                    {
                        "Effect": "Allow",
                        "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
                        "Resource": arns.source,
                    },
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.other },
                ])
            },
            delivered: 0,
            redelivers: false,
            left_on_source: 1,
        },
        Row {
            case: "the send allowed on every queue and then denied on the target",
            policy: |arns| {
                json!([
                    { "Effect": "Allow", "Action": "sqs:*", "Resource": "*" },
                    {
                        "Effect": "Deny",
                        "Action": "sqs:SendMessage",
                        "Resource": arns.target,
                    },
                ])
            },
            delivered: 0,
            redelivers: false,
            left_on_source: 1,
        },
        Row {
            case: "the source receive withheld, with the delivery allowed",
            policy: |arns| {
                json!([
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.target },
                ])
            },
            delivered: 0,
            redelivers: false,
            left_on_source: 1,
        },
        Row {
            case: "the source delete withheld, with the delivery allowed",
            policy: |arns| {
                json!([
                    {
                        "Effect": "Allow",
                        "Action": "sqs:ReceiveMessage",
                        "Resource": arns.source,
                    },
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": arns.target },
                ])
            },
            // Delivered, and then left on the source: the at-least-once a pipe that may
            // not delete its records produces — so the record is delivered AGAIN each time
            // its invisibility lapses, and the row asserts at least one delivery.
            delivered: 1,
            redelivers: true,
            left_on_source: 1,
        },
    ];

    let router = emulator();
    // Every row's pipe is created first and every source then written to, so the rows'
    // settle spans and delivery waits overlap instead of being paid one after another.
    let mut prepared: Vec<(&Row, Resources, String, String, String)> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let (source_url, source_arn) = given_queue(&router, &format!("role-source-{index}")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("role-target-{index}")).await;
        let (_, other_arn) = given_queue(&router, &format!("role-other-{index}")).await;
        let arns = Resources {
            source: source_arn.clone(),
            target: target_arn.clone(),
            other: other_arn,
        };
        let role = given_role(&router, case, &format!("role-{index}"), (row.policy)(&arns)).await;
        let name = format!("role-pipe-{index}");
        let created = create_pipe(&router, &name, &source_arn, &target_arn, &role).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        let marker = format!("record-{index}");
        send(&router, &source_url, std::slice::from_ref(&marker)).await;
        prepared.push((row, arns, source_url, target_url, marker));
    }

    // One instant every row's "nothing was delivered" is decided at, so the quiet window
    // is waited out once for the whole table.
    let quiet_until = Instant::now() + QUIET + QUIET;
    for (row, arns, source_url, target_url, marker) in &prepared {
        let case = row.case;
        if row.delivered == 0 {
            let remaining = quiet_until.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                tokio::time::sleep(remaining).await;
            }
        }
        let got = collect_at_least(&router, target_url, row.delivered, BUDGET).await;
        if row.redelivers {
            assert!(
                got.len() >= row.delivered,
                "case {case:?}: expected at least {} message(s) on the target {}, got {got:?}",
                row.delivered,
                arns.target
            );
        } else {
            assert_eq!(
                got.len(),
                row.delivered,
                "case {case:?}: expected {} message(s) on the target {}, got {got:?}",
                row.delivered,
                arns.target
            );
        }
        for body in &got {
            assert_eq!(
                body, marker,
                "case {case:?}: the target {} received {body:?}, not the record the source \
                 was sent",
                arns.target
            );
        }
        let left = wait_for_held(&router, source_url, row.left_on_source, BUDGET).await;
        assert_eq!(
            left, row.left_on_source,
            "case {case:?}: expected the source {} to hold {} record(s), got {left}",
            arns.source, row.left_on_source
        );
    }
}

/// RULE: the execution role is read from the pipe's record on every cycle, so `UpdatePipe`
/// switches the identity a RUNNING pipe's deliveries are authorized by — a role that may
/// reach the new target delivers, a role that may not delivers nothing, and the records
/// left behind are moved as soon as a role that may reach it is in force.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_pipe_switches_the_role_a_running_pipe_is_authorized_by() {
    let case = "a re-roled pipe";
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, "rerole-source").await;
    shorten_visibility(&router, case, &source_url).await;
    let (first_url, first_arn) = given_queue(&router, "rerole-target-a").await;
    let (second_url, second_arn) = given_queue(&router, "rerole-target-b").await;

    // Two roles, each able to send to ONE of the targets, so which queue a record reaches
    // is decided by which role the pipe is acting as.
    let source_access = json!({
        "Effect": "Allow",
        "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
        "Resource": source_arn,
    });
    let first_role = given_role(
        &router,
        case,
        "rerole-a",
        json!([
            source_access,
            { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": first_arn },
        ]),
    )
    .await;
    let second_role = given_role(
        &router,
        case,
        "rerole-b",
        json!([
            source_access,
            { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": second_arn },
        ]),
    )
    .await;

    let name = "rerole-pipe";
    let created = create_pipe(&router, name, &source_arn, &first_arn, &first_role).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    // The pipe as created: the first role, delivering into the first target.
    send(&router, &source_url, &["before-update".to_string()]).await;
    let got = collect_at_least(&router, &first_url, 1, BUDGET).await;
    assert_eq!(
        got,
        vec!["before-update".to_string()],
        "case {case:?}: expected the record on the first target {first_arn} under the \
         first role, got {got:?}"
    );

    // Re-roled AND re-pointed: the identity and the target of a RUNNING pipe both move.
    let updated = update_pipe(&router, name, &second_role, Some(&second_arn)).await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        updated.status, updated.body
    );
    let described = describe_pipe(&router, name).await;
    assert_eq!(
        described.string("RoleArn"),
        second_role,
        "case {case:?}: DescribePipe reports the role {} after the update, not {second_role}",
        described.string("RoleArn")
    );
    assert_eq!(
        described.string("Target"),
        second_arn,
        "case {case:?}: DescribePipe reports the target {} after the update, not {second_arn}",
        described.string("Target")
    );
    send(&router, &source_url, &["after-update".to_string()]).await;
    let got = collect_at_least(&router, &second_url, 1, BUDGET).await;
    assert_eq!(
        got,
        vec!["after-update".to_string()],
        "case {case:?}: expected the record on the second target {second_arn} under the \
         new role, got {got:?}"
    );

    // The negative control: the pipe is left on the FIRST role while its target stays the
    // second queue, which that role may not send to — so nothing is delivered and the
    // record stays on the source.
    let re_roled = update_pipe(&router, name, &first_role, None).await;
    assert_eq!(
        re_roled.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        re_roled.status, re_roled.body
    );
    send(&router, &source_url, &["under-the-old-role".to_string()]).await;
    tokio::time::sleep(QUIET + QUIET).await;
    let got = receive_now(&router, &second_url).await;
    assert!(
        got.is_empty(),
        "case {case:?}: the first role may not send to {second_arn}, so nothing may be \
         delivered there, got {got:?}"
    );
    let left = held(&router, &source_url).await;
    assert_eq!(
        left, 1,
        "case {case:?}: expected the undelivered record to stay on the source {source_arn}, \
         which held {left}"
    );

    // And back: the record the old role could not move is delivered as soon as the pipe
    // acts as a role that may reach the target, with no record lost in between.
    let re_roled = update_pipe(&router, name, &second_role, None).await;
    assert_eq!(
        re_roled.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        re_roled.status, re_roled.body
    );
    let got = collect_at_least(&router, &second_url, 1, BUDGET).await;
    assert_eq!(
        got,
        vec!["under-the-old-role".to_string()],
        "case {case:?}: expected the record the old role could not move on {second_arn} \
         once the new role is in force, got {got:?}"
    );
}
