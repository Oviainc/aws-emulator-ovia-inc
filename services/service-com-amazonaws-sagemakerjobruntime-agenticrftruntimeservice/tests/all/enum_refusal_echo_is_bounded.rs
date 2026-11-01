// SPDX-License-Identifier: CC0-1.0

//! The CEILING a refusal clause quotes a submitted value up to, on the one member of this
//! service whose value the model leaves unbounded.
//!
//! RULE: a refusal clause quotes the submitted value up to the ceiling this service states
//! from its OWN model — the declared `@length` maximum of the member a refusal is carried in
//! (`Message`, targeting `FailureReason`, max 1024 on every error shape of this service),
//! less what the clause states around the value — and past it keeps a
//! character-boundary-safe prefix followed by the marker naming the whole value's byte
//! count, whichever refusal path of the service words the clause. So no response is sized by
//! the request, and no refusal overruns the member that has to hold it.
//!
//! `CompleteRollout`'s `Status` is where the size of a refused value is the CALLER's to
//! choose: it targets the `CompletionStatus` enum
//! (`smithy_query(query="shape",shape="CompletionStatus")` — `ready`, `failed`) and declares
//! no `@length`, so a value the model does not declare is as large as the body carrying it,
//! while every other member a refusal of this service quotes (`TrajectoryId`, `JobArn`)
//! declares `@length` max 256. The rows below therefore drive the enum member, which
//! `tests/all/rejected_value_echo.rs` only reaches with values far under the ceiling.
//!
//! The rows span what the rule generalises over:
//!
//! * the value's SIZE — a handful of bytes, half the ceiling, exactly at the ceiling, one
//!   byte past it, and the megabyte a request can carry;
//! * its ENCODING — ASCII, two-byte and three-byte UTF-8, the last offset by one byte so the
//!   ceiling falls INSIDE a character;
//! * its CONTENT — values carrying the quote character the clause delimits with;
//! * and the MEMBER — the same oversized value on the enum member and on a length-bounded
//!   one, so the bound belongs to the service rather than to one member.

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::service::LARGEST_QUOTED_VALUE_BYTES;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    create_job_at_arn, mounted_service, send, Reply, COMPLETE_ROLLOUT_PATH, JOB_ARN_HEADER,
};

/// The maximum the model declares for the member every refusal of this service is carried
/// in: `Message` targets `FailureReason`, `@length` min 0 max 1024
/// (`smithy_query(query="shape",shape="FailureReason")`). The ceiling above is stated from
/// it, so no message any row below produces may pass it.
const LARGEST_MESSAGE_BYTES: usize = 1024;

/// The prose the `CompletionStatus` clause states its requirement with, in the model's own
/// declaration order (`smithy_query(query="shape",shape="CompletionStatus")`).
const VALUE_SET: &str = "Member must satisfy enum value set: [ready, failed]";

/// The `JobCategory` / `JobName` of the job every row addresses.
const JOB_CATEGORY: &str = "AgentRFT";
const JOB_NAME: &str = "echo-run-1";

/// A `JobArn` satisfying every modelled constraint and naming a job the control plane
/// created, so a refusal is about `Status` and not about the job.
fn well_formed_job() -> String {
    format!("arn:aws:sagemaker:us-east-1:123456789012:job/{JOB_CATEGORY}/{JOB_NAME}")
}

/// A router carrying exactly this service, with that job already registered.
fn mounted_service_with_job() -> MultiServiceRouter {
    let (engine, router) = mounted_service();
    create_job_at_arn(&engine, &well_formed_job());
    router
}

/// `CompleteRollout` with this `Status`, on a trajectory id the model accepts.
async fn complete_rollout_with_status(router: &MultiServiceRouter, status: &str) -> Reply {
    let body = json!({
        "TrajectoryId": "traj-1",
        "Status": status,
        "ClientToken": "tok",
    });
    send(
        router,
        COMPLETE_ROLLOUT_PATH,
        &[
            (JOB_ARN_HEADER, well_formed_job().as_str()),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

/// The AWS constraint-validation sentence for one violated member.
fn one_validation_error(value: &str, member: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// Assert the shared half of every row — a `400 ValidationException` in a restJson1
/// document whose message fits the member that carries it — and hand back the value the
/// clause quoted.
///
/// The quoted value is read by stripping the clause's fixed halves, never by splitting on a
/// quote, so a value carrying the delimiter itself is read whole.
fn quoted_value(case: &str, reply: &Reply) -> String {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.error_type
    );
    assert_eq!(
        reply.content_type.as_deref(),
        Some("application/json"),
        "case {case:?}: expected a restJson1 error document, got {:?}",
        reply.content_type
    );
    let message = reply.message();
    assert!(
        message.len() <= LARGEST_MESSAGE_BYTES,
        "case {case:?}: expected a message within the {LARGEST_MESSAGE_BYTES} byte maximum \
         the carrying member declares, got {} bytes",
        message.len()
    );
    let head = "1 validation error detected: Value '";
    let tail = format!("' at 'status' failed to satisfy constraint: {VALUE_SET}");
    let quoted = message
        .strip_prefix(head)
        .and_then(|rest| rest.strip_suffix(tail.as_str()));
    let Some(quoted) = quoted else {
        panic!(
            "case {case:?}: expected the enum clause for 'status' ({head}…{tail}), got a \
             {} byte message starting {:?}",
            message.len(),
            message.chars().take(120).collect::<String>()
        );
    };
    quoted.to_string()
}

/// RULE: a value within the ceiling is quoted byte for byte, whatever its size or encoding.
#[tokio::test]
async fn a_status_within_the_ceiling_is_quoted_in_full() {
    // Rows differ along size (a handful of bytes up to exactly the ceiling) and encoding
    // (ASCII, two-byte and three-byte UTF-8, and the quote character the clause delimits
    // with).
    let rows: [(&str, String); 6] = [
        ("a dozen bytes of ASCII", "bogus-status".to_string()),
        (
            "half the ceiling in ASCII",
            "s".repeat(LARGEST_QUOTED_VALUE_BYTES / 2),
        ),
        (
            "a status carrying the quote the clause delimits with",
            format!(
                "{}'{}",
                "x".repeat(LARGEST_QUOTED_VALUE_BYTES / 4),
                "y".repeat(LARGEST_QUOTED_VALUE_BYTES / 4)
            ),
        ),
        (
            "two-byte characters filling half the ceiling",
            "é".repeat(LARGEST_QUOTED_VALUE_BYTES / 4),
        ),
        (
            "three-byte characters just under the ceiling",
            "€".repeat((LARGEST_QUOTED_VALUE_BYTES / 3) - 1),
        ),
        (
            "ASCII exactly at the ceiling",
            "s".repeat(LARGEST_QUOTED_VALUE_BYTES),
        ),
    ];

    for (case, status) in rows {
        assert!(
            status.len() <= LARGEST_QUOTED_VALUE_BYTES,
            "case {case:?}: this row is meant to sit within the ceiling, but it is {} bytes",
            status.len()
        );
        let router = mounted_service_with_job();
        let reply = complete_rollout_with_status(&router, &status).await;
        let actual = reply.message();
        let expected = one_validation_error(&status, "status", VALUE_SET);
        assert_eq!(
            quoted_value(case, &reply),
            status,
            "case {case:?}: expected the submitted status quoted byte for byte"
        );
        assert_eq!(
            actual, expected,
            "case {case:?}: expected a {} byte message, got {} bytes",
            expected.len(),
            actual.len()
        );
    }
}

/// RULE: past the ceiling the clause keeps a character-boundary-safe prefix of the submitted
/// value plus the marker naming its whole byte count, so the response stays within the
/// member that carries it instead of being sized by the request.
#[tokio::test]
async fn a_status_past_the_ceiling_is_quoted_bounded() {
    // Rows differ along how far past the ceiling the value reaches and along the encoding:
    // the last row offsets three-byte characters by one ASCII byte, so the ceiling falls
    // INSIDE a character and the prefix has to step back to a character boundary.
    let rows: [(&str, String); 5] = [
        (
            "one byte past the ceiling",
            "s".repeat(LARGEST_QUOTED_VALUE_BYTES + 1),
        ),
        (
            "the kilobyte the carrying member could not hold",
            "s".repeat(1024),
        ),
        ("a megabyte of ASCII", "s".repeat(1024 * 1024)),
        (
            "a megabyte of two-byte characters",
            "é".repeat(512 * 1024),
        ),
        (
            "three-byte characters past a ceiling falling mid-character",
            format!("x{}", "€".repeat(LARGEST_QUOTED_VALUE_BYTES)),
        ),
    ];

    for (case, status) in rows {
        assert!(
            status.len() > LARGEST_QUOTED_VALUE_BYTES,
            "case {case:?}: this row is meant to pass the ceiling, but it is {} bytes",
            status.len()
        );
        let router = mounted_service_with_job();
        let reply = complete_rollout_with_status(&router, &status).await;
        let quoted = quoted_value(case, &reply);

        let marker = format!("... [{} bytes]", status.len());
        let Some(prefix) = quoted.strip_suffix(marker.as_str()) else {
            panic!(
                "case {case:?}: expected the clause to name the submitted {} bytes with \
                 {marker:?}, got a {} byte quoted value ending {:?}",
                status.len(),
                quoted.len(),
                quoted
                    .chars()
                    .rev()
                    .take(40)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            );
        };
        assert!(
            status.starts_with(prefix),
            "case {case:?}: expected the quoted {} bytes to be a prefix of the submitted \
             value",
            prefix.len()
        );
        // Never more than the ceiling, and never empty: the clause still reports what was
        // refused.
        assert!(
            !prefix.is_empty() && prefix.len() <= LARGEST_QUOTED_VALUE_BYTES,
            "case {case:?}: expected a non-empty prefix no longer than the \
             {LARGEST_QUOTED_VALUE_BYTES} byte ceiling, got {} bytes",
            prefix.len()
        );
    }
}

/// RULE (the ceiling is the SERVICE's, not one member's): the same oversized value refused
/// on the enum member with no declared length and on the length-bounded member both come
/// back inside the member that carries a refusal — no refusal of this service answers a
/// request-sized message.
#[tokio::test]
async fn every_refused_member_stays_bounded_for_the_same_value() {
    let oversized = "z".repeat(512 * 1024);

    // The enum member, whose model declares no length at all.
    let router = mounted_service_with_job();
    let by_enum_clause = complete_rollout_with_status(&router, &oversized)
        .await
        .message();

    // The length-bounded member: an over-long trajectory id.
    let router = mounted_service_with_job();
    let body = json!({
        "TrajectoryId": oversized,
        "Status": "ready",
        "ClientToken": "tok",
    });
    let by_length_clause = send(
        &router,
        COMPLETE_ROLLOUT_PATH,
        &[
            (JOB_ARN_HEADER, well_formed_job().as_str()),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
    .message();

    for (case, message) in [
        ("the enum clause on Status", &by_enum_clause),
        ("the length clause on TrajectoryId", &by_length_clause),
    ] {
        assert!(
            message.contains(&format!("[{} bytes]", oversized.len())),
            "case {case:?}: expected the clause to name the submitted byte count, got \
             {} bytes of message",
            message.len()
        );
        assert!(
            message.len() <= LARGEST_MESSAGE_BYTES,
            "case {case:?}: expected a message within the {LARGEST_MESSAGE_BYTES} byte \
             maximum the carrying member declares for a {} byte value, got {} bytes",
            oversized.len(),
            message.len()
        );
    }
}
