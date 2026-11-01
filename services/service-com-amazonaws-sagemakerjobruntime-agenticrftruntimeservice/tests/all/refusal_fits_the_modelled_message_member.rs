// SPDX-License-Identifier: CC0-1.0

//! Every refusal this service answers fits the member the model declares it in.
//!
//! RULE: the prose a service answers an error with is carried by a MODELLED member, so what
//! a refusal may say is bounded by that member's own declared `@length` maximum: every error
//! shape of this service (`AccessDeniedException`, `ConflictException`,
//! `InternalServiceError`, `ResourceNotFoundException`, `ServiceQuotaExceededException`,
//! `ThrottlingException`, `ValidationException` —
//! `smithy_query(query="closure",operation="CompleteRollout")`) declares exactly one member,
//! `Message`, targeting `FailureReason`, whose `@length` is min 0 max 1024. The only part of
//! a refusal the caller sizes is the values its clauses quote, so those are what the bound
//! falls on: no single value is quoted beyond a ceiling stated from that maximum, and where
//! a sentence states SEVERAL clauses they are held to what its prose leaves them TOGETHER —
//! past which a clause keeps a character-boundary-safe prefix plus the marker naming the
//! submitted byte count. So no refusal overruns the member that has to hold it and none is
//! sized by the request.
//!
//! The rows span what the rule generalises over rather than one request:
//!
//! * the MEMBER and its binding — `Status` in the JSON body (an enum with no `@length` at
//!   all, so the refused value's size is the caller's to choose), `TrajectoryId` in the body
//!   and in a header (`@length` 1..256), and the `JobArn` header (`@length` 1..256 plus the
//!   job-ARN `@pattern`, which this service words as a malformed ARN);
//! * the CONSTRAINT CLASS — the enum value set, the `@length` maximum, the `@length`
//!   minimum, an absent `@required` member, and the `@pattern`;
//! * the OPERATION — one of each family the model binds, since `Sample` and
//!   `SampleWithResponseStream` bind `TrajectoryId` to a header while `CompleteRollout` and
//!   `UpdateReward` carry it in the body;
//! * the value's SIZE — under the ceiling, at it, one byte past it, the kilobyte that is the
//!   carrying member's own maximum, the issue's 8 KiB instance, and a megabyte;
//! * its ENCODING — ASCII and multibyte UTF-8, including a value whose characters straddle
//!   the ceiling;
//! * and the CLAUSE COUNT — a request breaking two and three constraints at once, where the
//!   values are held to what the sentence leaves them rather than to a per-clause ceiling.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`].

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::service::LARGEST_QUOTED_VALUE_BYTES;
use smithy_server_runtime::MultiServiceRouter;

// The shared harness: the wire-answer reduction and the job-directory setup every suite of
// this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    create_job_at_arn, mounted_service, send, Reply, COMPLETE_ROLLOUT_PATH, JOB_ARN_HEADER,
    SAMPLE_PATH, SAMPLE_STREAM_PATH, TRAJECTORY_ID_HEADER, UPDATE_REWARD_PATH,
};

/// The maximum the model declares for the member every refusal of this service is carried
/// in: `Message` targets `FailureReason`, `@length` min 0 max 1024
/// (`smithy_query(query="shape",shape="FailureReason")`). No message below may pass it.
const LARGEST_MESSAGE_BYTES: usize = 1024;

/// `TrajectoryId`'s modelled `@length` maximum
/// (`smithy_query(query="shape",shape="TrajectoryId")`).
const TRAJECTORY_ID_MAX: usize = 256;

/// The `JobCategory` / `JobName` of the job every row addresses.
const JOB_CATEGORY: &str = "AgentRFT";
const JOB_NAME: &str = "echo-run-1";

/// A `JobArn` satisfying every modelled constraint and naming a job the control plane
/// created, so a refusal is about the member a row is driving.
fn well_formed_job() -> String {
    format!("arn:aws:sagemaker:us-east-1:123456789012:job/{JOB_CATEGORY}/{JOB_NAME}")
}

/// A router carrying exactly this service, with that job already registered.
fn mounted_service_with_job() -> MultiServiceRouter {
    let (engine, router) = mounted_service();
    create_job_at_arn(&engine, &well_formed_job());
    router
}

/// One call, built the way the operation bound to `path` binds its members: the sampling
/// operations carry `TrajectoryId` in a header, the rollout operations in the JSON body, and
/// `Status` is `CompleteRollout`'s alone.
///
/// `trajectory_id` is `None` for the row that omits the `@required` member altogether.
async fn call(
    router: &MultiServiceRouter,
    path: &str,
    job_arn: &str,
    trajectory_id: Option<&str>,
    status: &str,
) -> Reply {
    match path {
        SAMPLE_PATH | SAMPLE_STREAM_PATH => {
            let mut headers = vec![
                (JOB_ARN_HEADER, job_arn),
                ("content-type", "application/json"),
            ];
            if let Some(trajectory_id) = trajectory_id {
                headers.push((TRAJECTORY_ID_HEADER, trajectory_id));
            }
            send(
                router,
                path,
                &headers,
                serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": "go" }] }))
                    .expect("a serializable payload"),
            )
            .await
        }
        COMPLETE_ROLLOUT_PATH => {
            let mut body = json!({ "Status": status, "ClientToken": "tok" });
            if let Some(trajectory_id) = trajectory_id {
                body["TrajectoryId"] = json!(trajectory_id);
            }
            send(
                router,
                path,
                &[
                    (JOB_ARN_HEADER, job_arn),
                    ("content-type", "application/json"),
                ],
                serde_json::to_vec(&body).expect("a serializable body"),
            )
            .await
        }
        _ => {
            let mut body = json!({ "Rewards": [1.0], "ClientToken": "tok" });
            if let Some(trajectory_id) = trajectory_id {
                body["TrajectoryId"] = json!(trajectory_id);
            }
            send(
                router,
                path,
                &[
                    (JOB_ARN_HEADER, job_arn),
                    ("content-type", "application/json"),
                ],
                serde_json::to_vec(&body).expect("a serializable body"),
            )
            .await
        }
    }
}

/// Assert the half every row shares: a `400 ValidationException` carrying prose that fits
/// the member the model declares it in, and hand the message back.
fn refusal_within_the_member(case: &str, reply: &Reply) -> String {
    assert_eq!(
        (reply.status, reply.error_code().as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {} {:?} / {}",
        reply.status,
        reply.error_type,
        reply.text().chars().take(200).collect::<String>()
    );
    let message = reply.message();
    assert!(
        !message.is_empty(),
        "case {case:?}: expected the modelled Message member to carry the refusal, got {:?}",
        reply.text().chars().take(200).collect::<String>()
    );
    assert!(
        message.len() <= LARGEST_MESSAGE_BYTES,
        "case {case:?}: expected a refusal within the {LARGEST_MESSAGE_BYTES} byte maximum \
         the carrying member declares, got {} bytes",
        message.len()
    );
    message
}

/// What a row expects of the value its refusal is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Echo {
    /// Within the stated ceiling: the clause quotes the submitted bytes whole.
    Whole,
    /// Past it: the clause keeps a prefix plus the marker naming the submitted byte count.
    Marked,
}

/// Assert how `message` states `value`, and that a marked echo names the submitted byte
/// count rather than carrying the value itself.
fn assert_echo(case: &str, message: &str, value: &str, echo: Echo) {
    match echo {
        Echo::Whole => assert!(
            message.contains(value),
            "case {case:?}: expected the {} byte value quoted whole, got the {} byte \
             message {:?}",
            value.len(),
            message.len(),
            message.chars().take(160).collect::<String>()
        ),
        Echo::Marked => {
            let marker = format!("... [{} bytes]", value.len());
            assert!(
                message.contains(marker.as_str()),
                "case {case:?}: expected the clause to name the submitted {} bytes with \
                 {marker:?}, got the {} byte message {:?}",
                value.len(),
                message.len(),
                message.chars().take(160).collect::<String>()
            );
            assert!(
                !message.contains(value),
                "case {case:?}: expected the {} byte value NOT to be carried whole",
                value.len()
            );
        }
    }
}

/// RULE: the enum member, whose model declares no `@length` at all, is refused inside the
/// carrying member whatever the caller sends — quoted whole up to the stated ceiling and
/// marked past it.
#[tokio::test]
async fn a_refused_status_fits_the_carrying_member() {
    // Rows differ along size (a handful of bytes, at the ceiling, one past it, the carrying
    // member's own 1024 maximum, the reported 8 KiB instance, a megabyte) and encoding.
    let rows: [(&str, String, Echo); 8] = [
        ("a dozen bytes", "bogus-status".to_string(), Echo::Whole),
        (
            "half the ceiling",
            "s".repeat(LARGEST_QUOTED_VALUE_BYTES / 2),
            Echo::Whole,
        ),
        (
            "exactly at the ceiling",
            "s".repeat(LARGEST_QUOTED_VALUE_BYTES),
            Echo::Whole,
        ),
        (
            "multibyte characters within the ceiling",
            "€".repeat((LARGEST_QUOTED_VALUE_BYTES / 3) - 1),
            Echo::Whole,
        ),
        (
            "one byte past the ceiling",
            "s".repeat(LARGEST_QUOTED_VALUE_BYTES + 1),
            Echo::Marked,
        ),
        (
            "the carrying member's own maximum",
            "s".repeat(LARGEST_MESSAGE_BYTES),
            Echo::Marked,
        ),
        // The size the issue reported: a status of 8192 characters, which answered a
        // message of roughly 8.3 KB before the ceiling was stated from the model.
        ("eight kilobytes", "s".repeat(8 * 1024), Echo::Marked),
        (
            "a megabyte of multibyte characters straddling the ceiling",
            format!("x{}", "€".repeat(350 * 1024)),
            Echo::Marked,
        ),
    ];

    for (case, status, echo) in rows {
        let router = mounted_service_with_job();
        let reply = call(
            &router,
            COMPLETE_ROLLOUT_PATH,
            &well_formed_job(),
            Some("traj-1"),
            &status,
        )
        .await;
        let message = refusal_within_the_member(case, &reply);
        assert!(
            message.contains("at 'status'")
                && message.contains("Member must satisfy enum value set: [ready, failed]"),
            "case {case:?}: expected the enum clause for 'status', got {:?}",
            message.chars().take(160).collect::<String>()
        );
        assert_echo(case, &message, &status, echo);
    }
}

/// RULE: the same bound holds for the length-constrained member, on either binding and on
/// every operation that declares it, and for the `@length` minimum and an absent
/// `@required` member — whose clauses quote no caller-sized value at all and are therefore
/// already inside the carrying member.
#[tokio::test]
async fn a_refused_trajectory_id_fits_the_carrying_member() {
    // Rows differ along the value's size relative to both bounds, its encoding, the binding
    // (header on the sampling operations, JSON body on the rollout operations) and the
    // operation.
    let rows: [(&str, &str, String, Echo); 8] = [
        (
            "one character past the modelled maximum, header-bound",
            SAMPLE_PATH,
            "t".repeat(TRAJECTORY_ID_MAX + 1),
            Echo::Whole,
        ),
        (
            "one character past the modelled maximum, body-bound",
            COMPLETE_ROLLOUT_PATH,
            "t".repeat(TRAJECTORY_ID_MAX + 1),
            Echo::Whole,
        ),
        (
            "at the stated ceiling, body-bound",
            UPDATE_REWARD_PATH,
            "t".repeat(LARGEST_QUOTED_VALUE_BYTES),
            Echo::Whole,
        ),
        (
            "at the stated ceiling, header-bound and streaming",
            SAMPLE_STREAM_PATH,
            "t".repeat(LARGEST_QUOTED_VALUE_BYTES),
            Echo::Whole,
        ),
        (
            "one byte past the ceiling, header-bound",
            SAMPLE_PATH,
            "t".repeat(LARGEST_QUOTED_VALUE_BYTES + 1),
            Echo::Marked,
        ),
        (
            "the carrying member's own maximum, body-bound",
            COMPLETE_ROLLOUT_PATH,
            "t".repeat(LARGEST_MESSAGE_BYTES),
            Echo::Marked,
        ),
        (
            "multibyte characters past the ceiling, body-bound",
            UPDATE_REWARD_PATH,
            "é".repeat(LARGEST_MESSAGE_BYTES),
            Echo::Marked,
        ),
        (
            "a megabyte, body-bound",
            COMPLETE_ROLLOUT_PATH,
            "t".repeat(1024 * 1024),
            Echo::Marked,
        ),
    ];

    for (case, path, trajectory_id, echo) in rows {
        let router = mounted_service_with_job();
        let reply = call(
            &router,
            path,
            &well_formed_job(),
            Some(&trajectory_id),
            "ready",
        )
        .await;
        let message = refusal_within_the_member(case, &reply);
        assert!(
            message.contains("at 'trajectoryId'")
                && message.contains(&format!(
                    "Member must have length less than or equal to {TRAJECTORY_ID_MAX}"
                )),
            "case {case:?}: expected the length clause for 'trajectoryId', got {:?}",
            message.chars().take(160).collect::<String>()
        );
        assert_echo(case, &message, &trajectory_id, echo);
    }

    // The clauses with no caller-sized value: the `@length` minimum on an empty value, and
    // the member omitted altogether.
    for (case, path, trajectory_id, requirement) in [
        (
            "an empty trajectory id, header-bound",
            SAMPLE_PATH,
            Some(""),
            "Member must have length greater than or equal to 1",
        ),
        (
            "an empty trajectory id, body-bound",
            UPDATE_REWARD_PATH,
            Some(""),
            "Member must have length greater than or equal to 1",
        ),
        (
            "the required member omitted, body-bound",
            COMPLETE_ROLLOUT_PATH,
            None,
            "Member must not be null",
        ),
    ] {
        let router = mounted_service_with_job();
        let reply = call(&router, path, &well_formed_job(), trajectory_id, "ready").await;
        let message = refusal_within_the_member(case, &reply);
        assert!(
            message.contains("at 'trajectoryId'") && message.contains(requirement),
            "case {case:?}: expected {requirement:?} for 'trajectoryId', got {:?}",
            message.chars().take(160).collect::<String>()
        );
    }
}

/// RULE: this service's own wording of a value that is not an ARN at all is held to the same
/// ceiling, because the value reaches it off the same violation — so the malformed-ARN
/// sentence fits the carrying member for a header value of any size.
#[tokio::test]
async fn a_malformed_job_arn_fits_the_carrying_member() {
    // Rows differ along the value's size relative to the ceiling and along its shape: a
    // value that is nothing like an ARN, and ARN-shaped ones the pattern still refuses.
    let rows: [(&str, String, Echo); 6] = [
        ("a short value", "nope".to_string(), Echo::Whole),
        (
            "an ARN-shaped value with a bad account id",
            "arn:aws:sagemaker:us-east-1:12345:job/AgentRFT/run-1".to_string(),
            Echo::Whole,
        ),
        (
            "a value at the stated ceiling",
            format!("not-an-arn-{}", "z".repeat(LARGEST_QUOTED_VALUE_BYTES - 11)),
            Echo::Whole,
        ),
        (
            "one byte past the ceiling",
            format!("not-an-arn-{}", "z".repeat(LARGEST_QUOTED_VALUE_BYTES - 10)),
            Echo::Marked,
        ),
        (
            "the carrying member's own maximum",
            format!("not-an-arn-{}", "z".repeat(LARGEST_MESSAGE_BYTES)),
            Echo::Marked,
        ),
        (
            "eight kilobytes",
            format!("not-an-arn-{}", "z".repeat(8 * 1024)),
            Echo::Marked,
        ),
    ];

    for (case, bad_arn, echo) in rows {
        for path in [
            SAMPLE_PATH,
            SAMPLE_STREAM_PATH,
            COMPLETE_ROLLOUT_PATH,
            UPDATE_REWARD_PATH,
        ] {
            let case = &format!("{case} on {path}");
            let router = mounted_service_with_job();
            let reply = call(&router, path, &bad_arn, Some("traj-1"), "ready").await;
            let message = refusal_within_the_member(case, &reply);
            assert!(
                message.starts_with("Input arn ") && message.ends_with(" is malformed"),
                "case {case:?}: expected the malformed-ARN wording, got {:?}",
                message.chars().take(160).collect::<String>()
            );
            assert_echo(case, &message, &bad_arn, echo);
        }
    }
}

/// RULE (several clauses, one message): the clauses of a refusal are joined into ONE
/// `Message`, so what they quote is bounded TOGETHER — a request breaking two or three
/// constraints at once is still answered inside the maximum the carrying member declares,
/// with every clause stating which member it is about and how much was sent.
///
/// Rows span the clause count (two, and the three a request to this service can reach — its
/// four input members, with the ARN pattern reworded on its own) and the size of the values
/// quoted: small enough that the whole sentence fits with nothing held back, at the
/// per-value ceiling, at the carrying member's own maximum, and a megabyte each, where the
/// sentence the framework words is several times the maximum before the values are held to
/// what it leaves them.
#[tokio::test]
async fn a_multi_clause_refusal_fits_the_carrying_member() {
    /// A `JobArn` the job-ARN `@pattern` ACCEPTS — the category segment is
    /// `[a-zA-Z0-9_\-]+`, which the model leaves unbounded — and whose length still breaks
    /// `JobArn`'s `@length` max 256, so the clause is a length clause and not the
    /// malformed-ARN wording that replaces a pattern clause.
    fn over_long_but_well_formed_arn(category_bytes: usize) -> String {
        format!(
            "arn:aws:sagemaker:us-east-1:123456789012:job/{}/{JOB_NAME}",
            "c".repeat(category_bytes)
        )
    }

    // Each row: how big a value every broken member carries. The third element drives the
    // job ARN too, for the three-clause rows.
    let rows: [(&str, usize, bool); 8] = [
        ("a value no bound touches, two clauses", 300, false),
        ("a value no bound touches, three clauses", 300, true),
        (
            "the per-value ceiling, two clauses",
            LARGEST_QUOTED_VALUE_BYTES,
            false,
        ),
        (
            "the per-value ceiling, three clauses",
            LARGEST_QUOTED_VALUE_BYTES,
            true,
        ),
        (
            "the carrying member's own maximum, two clauses",
            LARGEST_MESSAGE_BYTES,
            false,
        ),
        (
            "the carrying member's own maximum, three clauses",
            LARGEST_MESSAGE_BYTES,
            true,
        ),
        ("a megabyte each, two clauses", 1024 * 1024, false),
        ("a megabyte each, three clauses", 1024 * 1024, true),
    ];

    for (case, size, three) in rows {
        let trajectory_id = "t".repeat(size);
        let status = "s".repeat(size);
        // A header value stays well inside what a header carries; the ARN clause is about
        // the length bound, which 300 bytes of category already breaks.
        let job_arn = if three {
            over_long_but_well_formed_arn(300)
        } else {
            well_formed_job()
        };
        let clauses = if three { 3 } else { 2 };

        let router = mounted_service_with_job();
        let reply = call(
            &router,
            COMPLETE_ROLLOUT_PATH,
            &job_arn,
            Some(&trajectory_id),
            &status,
        )
        .await;
        let message = refusal_within_the_member(case, &reply);
        assert!(
            message.starts_with(&format!("{clauses} validation errors detected: ")),
            "case {case:?}: expected all {clauses} constraints in one envelope, got {:?}",
            message.chars().take(200).collect::<String>()
        );
        // Every clause still names its member and how much was sent: either the value
        // itself, or the marker naming its byte count.
        for (member, value) in [("trajectoryId", &trajectory_id), ("status", &status)] {
            assert!(
                message.contains(&format!("at '{member}'")),
                "case {case:?}: expected a clause for {member:?}, got {:?}",
                message.chars().take(200).collect::<String>()
            );
            assert!(
                message.contains(value.as_str())
                    || message.contains(&format!("... [{} bytes]", value.len())),
                "case {case:?}: expected the clause for {member:?} to state the submitted \
                 {} bytes, whole or by its count, got {:?}",
                value.len(),
                message.chars().take(200).collect::<String>()
            );
        }
        if three {
            assert!(
                message.contains("at 'jobArn'"),
                "case {case:?}: expected a clause for the over-long job ARN, got {:?}",
                message.chars().take(200).collect::<String>()
            );
        }
    }
}
