// SPDX-License-Identifier: CC0-1.0

//! What becomes of a pipe whose EXECUTION ROLE this service cannot assume at all — end to
//! end over the wire, with IAM and Amazon SQS mounted beside pipes on one engine (the
//! harness of `tests/all/pipe_metrics.rs`, reused rather than copied).
//!
//! The RULE asserted below: a lifecycle call that puts a pipe's execution role in force —
//! one that STORES it (`CreatePipe`, `UpdatePipe`) or one that asks the pipe to run under
//! the role it already holds (`StartPipe`) — completes only if this service can assume that
//! role, so where the account's IAM records show the role's trust policy withholding the
//! assume from this service's principal the pipe settles in the terminal failure state of
//! the move that was in flight — `CREATE_FAILED`, `UPDATE_FAILED`, `START_FAILED` — carrying
//! a `StateReason` that names the role and the assume that failed, and it runs nothing. A
//! move that can be completed leaves no reason behind.
//!
//! `tests/all/pipe_metrics.rs` asserts the OTHER half of the execution role: a role this
//! service CAN assume whose policies bar one stage's call, which is a pipe that runs and
//! fails that stage. This one is about the role that bars every stage at once, which is a
//! pipe that never runs.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::pipe_state::{
    CREATE_FAILED, RUNNING, START_FAILED, UPDATE_FAILED,
};
use service_com_amazonaws_pipes_pipes::state::TRANSIENT_PIPE_STATES;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{collect_at_least, collect_during, given_queue, send, QUIET};
use crate::pipe_lifecycle::{call, describe, Reply, REGION};
use crate::pipe_metrics::{emulator, iam, xml_text};

/// How long a pipe is given to settle out of the state a lifecycle call left it in. Well
/// above the state-settle span, so a row fails only when the pipe never settled at all.
const BUDGET: Duration = Duration::from_secs(10);

/// How long between two looks at a pipe while waiting for it to settle.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// `TargetParameters.InputTemplate`, so a delivered message carries the record's own body
/// and a delivery can be attributed: `<$.body>` selects the `body` of the record envelope.
const BODY_TEMPLATE: &str = "<$.body>";

/// A trust document granting `principal` the assume, as a client sends one.
fn grant(principal: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// A role called `name` whose trust document is `trust` and whose one inline policy allows
/// every Amazon SQS call on every resource — so nothing a row asserts turns on the role's
/// PERMISSIONS, only on whether this service can assume it. Answered with the ARN IAM
/// persisted.
async fn given_role_trusting(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    trust: &Value,
) -> String {
    let document = trust.to_string();
    let (status, created) = iam(
        router,
        "CreateRole",
        &[("RoleName", name), ("AssumeRolePolicyDocument", &document)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateRole answered {status}: {created}"
    );
    let arn = xml_text(&created, "Arn")
        .unwrap_or_else(|| panic!("case {case:?}: no Arn in {created}"));
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "sqs:*", "Resource": "*" }],
    })
    .to_string();
    let (status, written) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name),
            ("PolicyName", "pipe-stages"),
            ("PolicyDocument", &policy),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRolePolicy answered {status}: {written}"
    );
    arn
}

/// `DescribePipe` once the pipe has settled out of whatever move was in flight, or a panic
/// naming the state it was still passing through when the budget ran out.
async fn settled(router: &MultiServiceRouter, case: &str, name: &str) -> Reply {
    let deadline = Instant::now() + BUDGET;
    let mut last = describe(router, name).await;
    loop {
        assert_eq!(
            last.status, 200,
            "case {case:?}: DescribePipe answered {} {}",
            last.status, last.body
        );
        let state = last.string("CurrentState");
        // The states a pipe PASSES THROUGH on its way out of a lifecycle call: a poll that
        // reads one of them has not seen where the pipe landed yet. The crate's own list,
        // so this cannot drift from it.
        if !TRANSIENT_PIPE_STATES.contains(&state.as_str()) {
            return last;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the pipe was still {state} after {BUDGET:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
        last = describe(router, name).await;
    }
}

/// `CreatePipe` for one case: an Amazon SQS source and target, the role `role_arn`, and
/// the `DesiredState` the case asks for when it asks for one.
async fn create_pipe(
    router: &MultiServiceRouter,
    name: &str,
    source_arn: &str,
    target_arn: &str,
    role_arn: &str,
    desired_state: Option<&str>,
) -> Reply {
    let mut body = json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": role_arn,
        "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
        "TargetParameters": { "InputTemplate": BODY_TEMPLATE },
    });
    if let Some(desired_state) = desired_state {
        body["DesiredState"] = json!(desired_state);
    }
    call(router, "POST", &format!("/v1/pipes/{name}"), Some(body), REGION).await
}

/// One row of the create table: the trust document the pipe's role carries, and where the
/// creation lands.
struct Row {
    case: &'static str,
    /// The role's trust document, or `None` when the account holds no role at the ARN the
    /// pipe names at all.
    trust: Option<fn() -> Value>,
    /// The `DesiredState` the creation asks for, or `None` to leave it at the default.
    desired_state: Option<&'static str>,
    /// The `CurrentState` the pipe has to settle in.
    settles_in: &'static str,
}

/// RULE: `CreatePipe` completes only if this service can assume the `RoleArn` it stores —
/// a role whose trust policy withholds `sts:AssumeRole` from `pipes.amazonaws.com` leaves
/// the pipe in `CREATE_FAILED` with a `StateReason` naming the role and the assume, and
/// such a pipe consumes nothing and delivers nothing.
///
/// The rows differ along what the rule generalises over: WHAT a trust document says about
/// the assume (it grants it, it grants it under a wildcard action, it grants it among
/// several principals, it grants another service only, it denies it beside the grant, it
/// names another action, it names no service principal at all), whether the account holds
/// a role at that ARN at all, and the state the creation asked the pipe to be in.
#[tokio::test]
async fn create_pipe_fails_the_creation_of_a_pipe_whose_role_it_cannot_assume() {
    let rows = [
        Row {
            case: "the-grant-as-written",
            trust: Some(|| grant(SERVICE_PRINCIPAL)),
            desired_state: None,
            settles_in: RUNNING,
        },
        Row {
            // `sts:*` covers `sts:AssumeRole`, so the role is assumable.
            case: "a-wildcard-action-covering-the-assume",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "sts:*",
                    }],
                })
            }),
            desired_state: None,
            settles_in: RUNNING,
        },
        Row {
            case: "this-principal-among-several",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {
                            "Service": ["ec2.amazonaws.com", SERVICE_PRINCIPAL],
                        },
                        "Action": ["sts:TagSession", "sts:AssumeRole"],
                    }],
                })
            }),
            desired_state: None,
            settles_in: RUNNING,
        },
        Row {
            // A role the account does not hold is evidence of nothing — it may belong to
            // another account — so the creation completes.
            case: "no-role-held-for-that-arn",
            trust: None,
            desired_state: None,
            settles_in: RUNNING,
        },
        Row {
            case: "another-services-principal",
            trust: Some(|| grant("lambda.amazonaws.com")),
            desired_state: None,
            settles_in: CREATE_FAILED,
        },
        Row {
            case: "an-explicit-deny-beside-the-grant",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "Service": SERVICE_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                        {
                            "Effect": "Deny",
                            "Principal": { "Service": SERVICE_PRINCIPAL },
                            "Action": "sts:AssumeRole",
                        },
                    ],
                })
            }),
            desired_state: None,
            settles_in: CREATE_FAILED,
        },
        Row {
            case: "an-action-that-is-not-the-assume",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": SERVICE_PRINCIPAL },
                        "Action": "s3:GetObject",
                    }],
                })
            }),
            desired_state: None,
            settles_in: CREATE_FAILED,
        },
        Row {
            // A trust document that names no SERVICE principal at all — a role meant to be
            // assumed by an account's own identities — withholds the assume too.
            case: "a-grant-to-an-account-principal-only",
            trust: Some(|| {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "AWS": "arn:aws:iam::000000000000:root" },
                        "Action": "sts:AssumeRole",
                    }],
                })
            }),
            desired_state: None,
            settles_in: CREATE_FAILED,
        },
        Row {
            // The assume is a property of the ROLE, not of the pipe running, so a creation
            // that asked for a stopped pipe fails the same way.
            case: "a-creation-that-asked-for-a-stopped-pipe",
            trust: Some(|| grant("lambda.amazonaws.com")),
            desired_state: Some("STOPPED"),
            settles_in: CREATE_FAILED,
        },
    ];

    for row in rows {
        let router = emulator();
        let (source_url, source_arn) = given_queue(&router, &format!("{}-source", row.case)).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{}-target", row.case)).await;
        let role_arn = match row.trust {
            Some(trust) => {
                given_role_trusting(
                    &router,
                    row.case,
                    &format!("{}-role", row.case),
                    &trust(),
                )
                .await
            }
            // A syntactically good role ARN of another account, which this account holds
            // no record of.
            None => format!("arn:aws:iam::000000000000:role/{}-absent", row.case),
        };

        let created = create_pipe(
            &router,
            row.case,
            &source_arn,
            &target_arn,
            &role_arn,
            row.desired_state,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreatePipe answered {} {}",
            row.case, created.status, created.body
        );
        // The creation is ACCEPTED either way: the failure is a state the pipe settles in,
        // not an answer to this call.
        assert_eq!(
            created.string("CurrentState"),
            "CREATING",
            "case {:?}: CreatePipe must report the pipe CREATING, got {}",
            row.case,
            created.body
        );

        let described = settled(&router, row.case, row.case).await;
        let state = described.string("CurrentState");
        assert_eq!(
            state, row.settles_in,
            "case {:?}: expected the pipe to settle in {}, got {state} ({})",
            row.case, row.settles_in, described.body
        );
        // The failure says WHICH role could not be assumed and WHAT could not be done with
        // it; a creation that completed leaves no reason behind.
        let reason = described.string("StateReason");
        if row.settles_in == CREATE_FAILED {
            assert!(
                reason.contains(&role_arn)
                    && reason.contains("sts:AssumeRole")
                    && reason.contains(SERVICE_PRINCIPAL),
                "case {:?}: expected the StateReason to name {role_arn}, sts:AssumeRole and \
                 {SERVICE_PRINCIPAL}, got {reason:?}",
                row.case
            );
        } else {
            assert!(
                reason.is_empty(),
                "case {:?}: a completed creation leaves no StateReason, got {reason:?}",
                row.case
            );
        }
        // The DesiredState the caller asked for is reported whatever the pipe reached, so
        // the pair is what tells the two apart.
        let wanted = row.desired_state.unwrap_or(RUNNING);
        assert_eq!(
            described.string("DesiredState"),
            wanted,
            "case {:?}: expected DesiredState {wanted}, got {}",
            row.case,
            described.body
        );

        // What a client sees of the data plane: a pipe that reached RUNNING moves the
        // record, and one that did not leaves it on the source and the target empty.
        send(&router, &source_url, &[format!("{}-record", row.case)]).await;
        if state == RUNNING {
            let delivered = collect_at_least(&router, &target_url, 1, BUDGET).await;
            assert_eq!(
                delivered,
                vec![format!("{}-record", row.case)],
                "case {:?}: expected the target to hold the record the pipe moved",
                row.case
            );
        } else {
            let delivered = collect_during(&router, &target_url, QUIET).await;
            assert!(
                delivered.is_empty(),
                "case {:?}: expected the target to hold nothing, got {delivered:?}",
                row.case
            );
            let left = collect_during(&router, &source_url, QUIET).await;
            assert_eq!(
                left,
                vec![format!("{}-record", row.case)],
                "case {:?}: expected the record to be left on the source, got {left:?}",
                row.case
            );
        }
    }
}

/// RULE, the same one at the OTHER call that stores a role: `UpdatePipe` completes only if
/// this service can assume the `RoleArn` it stores, so an update onto a role whose trust
/// policy withholds the assume leaves the pipe in `UPDATE_FAILED` with the reason naming
/// it, and stops the deliveries; an update back onto a role that can be assumed completes,
/// clears the reason and resumes them.
#[tokio::test]
async fn update_pipe_fails_the_update_that_leaves_a_pipe_on_a_role_it_cannot_assume() {
    let case = "re-roled";
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{case}-target")).await;
    let good = given_role_trusting(
        &router,
        case,
        &format!("{case}-trusted"),
        &grant(SERVICE_PRINCIPAL),
    )
    .await;
    let bad = given_role_trusting(
        &router,
        case,
        &format!("{case}-untrusted"),
        &grant("lambda.amazonaws.com"),
    )
    .await;

    let created = create_pipe(&router, case, &source_arn, &target_arn, &good, None).await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );
    let described = settled(&router, case, case).await;
    assert_eq!(
        described.string("CurrentState"),
        RUNNING,
        "case {case:?}: the pipe created on a role this service can assume must run, got {}",
        described.body
    );
    // The differential twin: under the role it was created with, the pipe moves records.
    send(&router, &source_url, &[format!("{case}-before")]).await;
    let delivered = collect_at_least(&router, &target_url, 1, BUDGET).await;
    assert_eq!(
        delivered,
        vec![format!("{case}-before")],
        "case {case:?}: expected the running pipe to move the first record"
    );

    // The update that leaves the pipe on a role this service cannot assume.
    let updated = call(
        &router,
        "PUT",
        &format!("/v1/pipes/{case}"),
        Some(json!({ "RoleArn": bad })),
        REGION,
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        updated.status, updated.body
    );
    let described = settled(&router, case, case).await;
    assert_eq!(
        described.string("CurrentState"),
        UPDATE_FAILED,
        "case {case:?}: expected the pipe to settle in {UPDATE_FAILED}, got {}",
        described.body
    );
    let reason = described.string("StateReason");
    assert!(
        reason.contains(&bad) && reason.contains("sts:AssumeRole"),
        "case {case:?}: expected the StateReason to name {bad} and sts:AssumeRole, got {reason:?}"
    );
    assert_eq!(
        described.string("RoleArn"),
        bad,
        "case {case:?}: DescribePipe must report the role the update stored, got {}",
        described.body
    );
    send(&router, &source_url, &[format!("{case}-during")]).await;
    let delivered = collect_during(&router, &target_url, QUIET).await;
    assert!(
        delivered.is_empty(),
        "case {case:?}: expected a pipe whose update failed to deliver nothing, got {delivered:?}"
    );

    // And the update back onto a role that can be assumed completes, leaves no reason and
    // moves the record that was waiting all along.
    let updated = call(
        &router,
        "PUT",
        &format!("/v1/pipes/{case}"),
        Some(json!({ "RoleArn": good })),
        REGION,
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        updated.status, updated.body
    );
    let described = settled(&router, case, case).await;
    assert_eq!(
        described.string("CurrentState"),
        RUNNING,
        "case {case:?}: expected the re-roled pipe to run again, got {}",
        described.body
    );
    let reason = described.string("StateReason");
    assert!(
        reason.is_empty(),
        "case {case:?}: a completed update leaves no StateReason, got {reason:?}"
    );
    let delivered = collect_at_least(&router, &target_url, 1, BUDGET).await;
    assert_eq!(
        delivered,
        vec![format!("{case}-during")],
        "case {case:?}: expected the record held back by the failed update to be moved now"
    );
}

/// RULE, at the THIRD lifecycle call the role is in force for: `StartPipe` asks the pipe to
/// run under the `RoleArn` it already HOLDS, so a start of a pipe whose role this service
/// cannot assume cannot be completed either — the pipe settles in `START_FAILED` with the
/// reason naming that role, and consumes nothing.
///
/// The pipe is created `STOPPED` on a role that can be assumed and started once (the
/// differential twin: it runs and delivers), then stopped and re-roled onto a role that
/// cannot be assumed. That update stores the role, so it fails its own move
/// (`UPDATE_FAILED`, the rule the suite above asserts); what this one adds is the START that
/// follows, which is a move of its own and fails as `START_FAILED` — so the state a caller
/// sees names the call that could not be completed rather than the previous one.
#[tokio::test]
async fn start_pipe_fails_the_start_of_a_pipe_whose_stored_role_it_cannot_assume() {
    let case = "started-on-an-untrusted-role";
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{case}-source")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{case}-target")).await;
    let good = given_role_trusting(
        &router,
        case,
        &format!("{case}-trusted"),
        &grant(SERVICE_PRINCIPAL),
    )
    .await;
    let bad = given_role_trusting(
        &router,
        case,
        &format!("{case}-untrusted"),
        &grant("lambda.amazonaws.com"),
    )
    .await;

    let created = create_pipe(
        &router,
        case,
        &source_arn,
        &target_arn,
        &good,
        Some("STOPPED"),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );
    let described = settled(&router, case, case).await;
    assert_eq!(
        described.string("CurrentState"),
        "STOPPED",
        "case {case:?}: a pipe created STOPPED must settle STOPPED, got {}",
        described.body
    );

    // Started on the role it was created with: the pipe runs and moves the record.
    let started = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}/start"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartPipe answered {} {}",
        started.status, started.body
    );
    let described = settled(&router, case, case).await;
    assert_eq!(
        described.string("CurrentState"),
        RUNNING,
        "case {case:?}: a pipe started on a role this service can assume must run, got {}",
        described.body
    );
    send(&router, &source_url, &[format!("{case}-before")]).await;
    let delivered = collect_at_least(&router, &target_url, 1, BUDGET).await;
    assert_eq!(
        delivered,
        vec![format!("{case}-before")],
        "case {case:?}: expected the started pipe to move the first record"
    );

    // Stopped, then re-roled while stopped: the update asks for no run, so it completes.
    let stopped = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}/stop"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        stopped.status, 200,
        "case {case:?}: StopPipe answered {} {}",
        stopped.status, stopped.body
    );
    let described = settled(&router, case, case).await;
    assert_eq!(
        described.string("CurrentState"),
        "STOPPED",
        "case {case:?}: expected the pipe to settle STOPPED, got {}",
        described.body
    );
    let updated = call(
        &router,
        "PUT",
        &format!("/v1/pipes/{case}"),
        Some(json!({ "RoleArn": bad, "DesiredState": "STOPPED" })),
        REGION,
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        updated.status, updated.body
    );
    let described = settled(&router, case, case).await;
    assert_eq!(
        described.string("CurrentState"),
        UPDATE_FAILED,
        "case {case:?}: the update that stored the untrusted role must fail its own move, \
         got {}",
        described.body
    );
    assert_eq!(
        described.string("RoleArn"),
        bad,
        "case {case:?}: DescribePipe must report the role the update stored, got {}",
        described.body
    );

    // And the start that asks it to run under that role cannot be completed either — as its
    // OWN move, so the state names the start and not the update.
    let started = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}/start"),
        None,
        REGION,
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartPipe answered {} {}",
        started.status, started.body
    );
    let described = settled(&router, case, case).await;
    assert_eq!(
        described.string("CurrentState"),
        START_FAILED,
        "case {case:?}: expected the pipe to settle in {START_FAILED}, got {}",
        described.body
    );
    let reason = described.string("StateReason");
    assert!(
        reason.contains(&bad) && reason.contains("sts:AssumeRole"),
        "case {case:?}: expected the StateReason to name {bad} and sts:AssumeRole, got {reason:?}"
    );
    send(&router, &source_url, &[format!("{case}-after")]).await;
    let delivered = collect_during(&router, &target_url, QUIET).await;
    assert!(
        delivered.is_empty(),
        "case {case:?}: expected a pipe whose start failed to deliver nothing, got {delivered:?}"
    );
    let left = collect_during(&router, &source_url, QUIET).await;
    assert_eq!(
        left,
        vec![format!("{case}-after")],
        "case {case:?}: expected the record to be left on the source, got {left:?}"
    );
}
