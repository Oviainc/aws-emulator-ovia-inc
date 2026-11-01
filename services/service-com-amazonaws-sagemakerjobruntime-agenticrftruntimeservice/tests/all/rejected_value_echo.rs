// SPDX-License-Identifier: CC0-1.0

//! What a constraint refusal echoes back: the value the caller submitted, byte for byte.
//!
//! RULE: a constraint-validation message is BUILT FROM THE REQUEST —
//! `1 validation error detected: Value '<value>' at '<member path>' failed to satisfy
//! constraint: <requirement>` — so the value between the quotes is the one the request
//! carried, reproduced unmodified up to the ceiling this service states from its own model
//! (the declared maximum of the member a refusal is carried in, less the clause's own
//! prose), and the same holds for this service's own wording of a value that is not an ARN
//! at all (`Input arn <value> is malformed`). The server reports the offending input
//! nowhere else, so a caller that compares the echo against what it sent must see its own
//! bytes and never a shortened, annotated form of them.
//!
//! The rows below span what the rule generalises over, not one recorded request:
//!
//! * the value's SIZE — one character past the modelled maximum, the recorded
//!   300-character case, and twice that;
//! * its ENCODING — ASCII, multibyte UTF-8 (where the `@length` bound counts characters
//!   while the elision counted bytes), and values carrying the quote, brace and space
//!   characters the clause and the JSON envelope have to survive;
//! * its BINDING — `TrajectoryId` arrives in a header on the sampling operations and in
//!   the JSON body on the rollout operations;
//! * the CONSTRAINT CLASS — `@length` maximum, `@length` minimum, `@enum` value set and
//!   `@pattern` (which this service words as a malformed ARN), each on the operations
//!   that declare it;
//! * and the BOUND itself — a value AT the modelled maximum is accepted, so the rows
//!   above are about the echo and not about length in general.
//!
//! Member paths, bounds and value sets come from the model:
//! `smithy_query(query="operation",operation="Sample,SampleWithResponseStream,CompleteRollout,UpdateReward")`
//! — `TrajectoryId` targets `TrajectoryId` (`@length` min 1 max 256), `JobArn` targets
//! `JobArn` (`@length` min 1 max 256 plus the job-ARN `@pattern`), `Status` targets the
//! `CompletionStatus` enum (`ready`, `failed`).

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::service::LARGEST_QUOTED_VALUE_BYTES;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite
// of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    create_job_at_arn, mounted_service, send, Reply, JOB_ARN_HEADER, TRAJECTORY_ID_HEADER,
};

/// `TrajectoryId`'s modelled `@length` bounds (`smithy_query(query="shape",shape="TrajectoryId")`).
const TRAJECTORY_ID_MAX: usize = 256;
const TRAJECTORY_ID_MIN: usize = 1;

/// The marker the rejected value must NOT be shortened with: the elision a renderer
/// appends, which AWS never puts on the wire.
const ELISION_MARKER: &str = " bytes]";

/// The four operations of this service, and where each binds `TrajectoryId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Sample,
    SampleWithResponseStream,
    CompleteRollout,
    UpdateReward,
}

impl Op {
    /// The URI this operation's model binds.
    fn path(self) -> &'static str {
        match self {
            Op::Sample => "/sample",
            Op::SampleWithResponseStream => "/sample-with-response-stream",
            Op::CompleteRollout => "/complete-rollout",
            Op::UpdateReward => "/update-reward",
        }
    }
}

/// A router carrying exactly this service over one shared engine, with the job
/// [`well_formed_job`] names already created.
fn mounted_service_with_job() -> MultiServiceRouter {
    let (engine, router) = mounted_service();
    // The registration the SageMaker control plane's `CreateJob` writes: this data plane
    // resolves the `JobArn` a call carries against it, so the job every row addresses
    // exists and a refusal is about the member under test.
    create_job_at_arn(&engine, &well_formed_job());
    router
}

/// The `JobCategory` and `JobName` of the job every row of this suite addresses.
const JOB_CATEGORY: &str = "AgentRFT";
const JOB_NAME: &str = "echo-run-1";

/// A `JobArn` satisfying every modelled constraint and naming a job the control plane
/// created ([`mounted_service_with_job`]), so a refusal is about the member under test and
/// not about the job ARN.
fn well_formed_job() -> String {
    format!("arn:aws:sagemaker:us-east-1:123456789012:job/{JOB_CATEGORY}/{JOB_NAME}")
}

/// An OpenAI-compatible chat-completions request body.
fn chat_payload() -> Vec<u8> {
    serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": "go" }] }))
        .expect("a serializable payload")
}

/// Call `op` with this `job_arn`, `trajectory_id` and (for `CompleteRollout`) `status`,
/// each operation's way: the sampling operations bind `TrajectoryId` to a header, the
/// rollout operations carry it in the JSON body.
async fn call(
    router: &MultiServiceRouter,
    op: Op,
    job_arn: &str,
    trajectory_id: &str,
    status: &str,
) -> Reply {
    match op {
        Op::Sample | Op::SampleWithResponseStream => {
            send(
                router,
                op.path(),
                &[
                    (JOB_ARN_HEADER, job_arn),
                    (TRAJECTORY_ID_HEADER, trajectory_id),
                    ("content-type", "application/json"),
                ],
                chat_payload(),
            )
            .await
        }
        Op::CompleteRollout => {
            let body = json!({
                "TrajectoryId": trajectory_id,
                "Status": status,
                "ClientToken": "tok",
            });
            send(
                router,
                op.path(),
                &[(JOB_ARN_HEADER, job_arn), ("content-type", "application/json")],
                serde_json::to_vec(&body).expect("a serializable body"),
            )
            .await
        }
        Op::UpdateReward => {
            let body = json!({
                "TrajectoryId": trajectory_id,
                "Rewards": [1.0],
                "ClientToken": "tok",
            });
            send(
                router,
                op.path(),
                &[(JOB_ARN_HEADER, job_arn), ("content-type", "application/json")],
                serde_json::to_vec(&body).expect("a serializable body"),
            )
            .await
        }
    }
}

/// The AWS constraint-validation sentence for one violated member, as the rule states it.
fn one_validation_error(value: &str, member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// Assert the shared half of every row: a `400 ValidationException` in a restJson1
/// document whose `Message` is exactly `expected` and carries no elision marker.
fn assert_refused(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {} {:?} / {}",
        reply.status,
        reply.error_type,
        reply.text()
    );
    assert_eq!(
        reply.content_type.as_deref(),
        Some("application/json"),
        "case {case:?}: expected a restJson1 error document, got {:?}",
        reply.content_type
    );
    let actual = reply.message();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected the message {expected:?}, got {actual:?}"
    );
    assert!(
        !actual.contains(ELISION_MARKER),
        "case {case:?}: expected no elision marker in the message, got {actual:?}"
    );
}

/// RULE: the `@length`-maximum clause echoes the submitted value unmodified, whatever its
/// size, encoding or binding.
#[tokio::test]
async fn an_over_long_value_is_echoed_in_full() {
    let job_arn = well_formed_job();
    let requirement = format!("Member must have length less than or equal to {TRAJECTORY_ID_MAX}");

    // Rows differ along the value's size (one past the bound, the recorded 300-character
    // case, a kilobyte), its encoding (ASCII, multibyte UTF-8, quote/brace/space
    // characters) and the binding the value arrives on (header vs JSON body).
    let rows: [(&str, Op, String); 8] = [
        (
            "one character past the maximum, header-bound",
            Op::Sample,
            "t".repeat(TRAJECTORY_ID_MAX + 1),
        ),
        (
            "the recorded 300-character value, header-bound and streaming",
            Op::SampleWithResponseStream,
            "t".repeat(300),
        ),
        (
            "twice the recorded case, header-bound",
            Op::Sample,
            "a".repeat(600),
        ),
        (
            "a header value carrying the quote the clause delimits with",
            Op::SampleWithResponseStream,
            format!("{}'{}", "x".repeat(200), "y".repeat(100)),
        ),
        (
            "a header value carrying a double quote the JSON envelope must escape",
            Op::Sample,
            format!("{}\"{}", "p".repeat(200), "q".repeat(100)),
        ),
        (
            "one character past the maximum, body-bound",
            Op::CompleteRollout,
            "t".repeat(TRAJECTORY_ID_MAX + 1),
        ),
        (
            // 300 characters is 600 bytes: the modelled bound counts characters, so a
            // renderer that shortened at a BYTE cap cut this value mid-way through.
            "300 multibyte characters, body-bound",
            Op::UpdateReward,
            "é".repeat(300),
        ),
        (
            "a body value carrying braces and spaces",
            Op::CompleteRollout,
            format!("{} {{brace}} {}", "m".repeat(150), "n".repeat(150)),
        ),
    ];

    for (case, op, value) in rows {
        let router = mounted_service_with_job();
        let reply = call(&router, op, &job_arn, &value, "ready").await;
        assert_refused(
            case,
            &reply,
            &one_validation_error(&value, "trajectoryId", &requirement),
        );
    }
}

/// RULE: the same echo holds for every constraint class this service's model declares —
/// the `@length` minimum, the `@enum` value set, and the `@pattern` this service words as
/// a malformed ARN — and a value short enough to need no shortening is echoed identically.
#[tokio::test]
async fn every_constraint_class_echoes_the_submitted_value() {
    let job_arn = well_formed_job();

    // The `@length` MINIMUM: an empty value, on both bindings. Nothing is shortened here,
    // so these rows pin that the fix did not change a message that was already right.
    let minimum = format!("Member must have length greater than or equal to {TRAJECTORY_ID_MIN}");
    for (case, op) in [
        ("an empty trajectory id, header-bound", Op::Sample),
        ("an empty trajectory id, body-bound", Op::UpdateReward),
    ] {
        let router = mounted_service_with_job();
        let reply = call(&router, op, &job_arn, "", "ready").await;
        assert_refused(
            case,
            &reply,
            &one_validation_error("", "trajectoryId", &minimum),
        );
    }

    // The `@enum` value set on `CompleteRollout`'s `Status`. Rows differ along the
    // rejected value's size: one no renderer would shorten, and one well past any cap.
    let value_set = "Member must satisfy enum value set: [ready, failed]";
    for (case, status) in [
        ("a status of its own", "bogus-status".to_string()),
        ("a 300-character status", "s".repeat(300)),
        ("a multibyte status", "ré".repeat(200)),
    ] {
        let router = mounted_service_with_job();
        let reply = call(&router, Op::CompleteRollout, &job_arn, "traj-1", &status).await;
        assert_refused(
            &format!("{case} on CompleteRollout"),
            &reply,
            &one_validation_error(&status, "status", value_set),
        );
    }

    // The job-ARN `@pattern`, which this service words as a malformed ARN naming the
    // value it was given. Rows differ along the value's size, and the long one also
    // breaches `JobArn`'s own 256 maximum — the ARN wording still states the whole value.
    for (case, bad_arn) in [
        ("a short value that is not an ARN", "nope".to_string()),
        (
            "a 300-character value that is not an ARN",
            format!("not-an-arn-{}", "z".repeat(289)),
        ),
        (
            "an ARN-shaped value with a bad account id",
            "arn:aws:sagemaker:us-east-1:12345:job/AgentRFT/run-1".to_string(),
        ),
    ] {
        for op in [
            Op::Sample,
            Op::SampleWithResponseStream,
            Op::CompleteRollout,
            Op::UpdateReward,
        ] {
            let router = mounted_service_with_job();
            let reply = call(&router, op, &bad_arn, "traj-1", "ready").await;
            assert_refused(
                &format!("{case} on {op:?}"),
                &reply,
                &format!("Input arn {bad_arn} is malformed"),
            );
        }
    }
}

/// RULE (the bound itself): a value AT the modelled maximum satisfies the constraint, so
/// the rows above are about what a refusal echoes and not about length in general.
#[tokio::test]
async fn a_value_at_the_maximum_is_not_refused() {
    let router = mounted_service_with_job();
    let at_max = "t".repeat(TRAJECTORY_ID_MAX);
    let reply = call(&router, Op::Sample, &well_formed_job(), &at_max, "ready").await;
    assert_eq!(
        reply.status,
        200,
        "case \"a trajectory id at the 256 maximum\": expected 200, got {} / {}",
        reply.status,
        reply.text()
    );
}

/// RULE (the echo's own bound): the echo is restored up to the ceiling this service states
/// from its model — `LARGEST_QUOTED_VALUE_BYTES`, the declared maximum of the member a
/// refusal is carried in less the clause's own prose — and past it the message stays inside
/// that member rather than being request-sized: the clause keeps a shortened rendering
/// naming the submitted byte count, and the refusal is still a `400 ValidationException`
/// about the right member.
#[tokio::test]
async fn the_echo_is_restored_up_to_the_stated_bound_and_bounded_past_it() {
    /// The maximum the model declares for the member a refusal is carried in: `Message`
    /// targets `FailureReason`, `@length` min 0 max 1024
    /// (`smithy_query(query="shape",shape="FailureReason")`).
    const LARGEST_MESSAGE_BYTES: usize = 1024;

    let job_arn = well_formed_job();
    let requirement = format!("Member must have length less than or equal to {TRAJECTORY_ID_MAX}");

    // AT the bound: restored in full. Carried in the JSON body, the binding with no
    // per-header size convention of its own.
    let at_bound = "b".repeat(LARGEST_QUOTED_VALUE_BYTES);
    let router = mounted_service_with_job();
    let reply = call(&router, Op::CompleteRollout, &job_arn, &at_bound, "ready").await;
    assert_refused(
        "a value at the stated echo bound",
        &reply,
        &one_validation_error(&at_bound, "trajectoryId", &requirement),
    );

    // PAST the bound: bounded, and still the right refusal about the right member.
    for (case, value) in [
        (
            "one byte past the echo bound",
            "b".repeat(LARGEST_QUOTED_VALUE_BYTES + 1),
        ),
        (
            "far past the echo bound",
            "b".repeat(64 * LARGEST_QUOTED_VALUE_BYTES),
        ),
    ] {
        let router = mounted_service_with_job();
        let reply = call(&router, Op::CompleteRollout, &job_arn, &value, "ready").await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.error_type
        );
        let message = reply.message();
        assert!(
            message.contains("at 'trajectoryId'") && message.contains(&requirement),
            "case {case:?}: expected the length clause for trajectoryId, got {message:?}"
        );
        assert!(
            message.len() <= LARGEST_MESSAGE_BYTES,
            "case {case:?}: expected a message within the {LARGEST_MESSAGE_BYTES} byte \
             maximum the carrying member declares for a {} byte value, got {} bytes",
            value.len(),
            message.len()
        );
        assert!(
            message.contains(&format!("[{} bytes]", value.len())),
            "case {case:?}: expected the shortened rendering to name the submitted byte \
             count, got {message:?}"
        );
    }
}
