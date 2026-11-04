// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the not-found gate of SageMaker's processing-job API:
//! `StopProcessingJob` and `DescribeProcessingJob` against a `ProcessingJobName` the
//! account holds no job under.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses.
//!
//! The tests assert the RULE over a TABLE of inputs rather than one recorded call: a
//! processing-job call resolves `ProcessingJobName` before it answers, and a name that
//! resolves to nothing is refused with HTTP 400 and the error code `ValidationException`,
//! whose message is the fixed template for that call class plus the name the request
//! presented — the stop-target template for the stop, the read template for the describe —
//! with nothing else in it varying. Rows differ along the shape and length of the name
//! (both published bounds of `ProcessingJobName` and in between), the Region the caller
//! signs for, how the name misses (never created, a prefix, a suffix, a case shift, a
//! job stored in another Region) and whether the store holds other jobs at all.

use std::time::Duration;

use serde_json::{json, Value};

// The crate's wire-test harness, shared by every suite in this one test binary.
use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator};

/// Long enough to pass `state::PROCESSING_JOB_ENDS_AFTER` (400 ms), after which a job
/// left to run has reached its terminal status and no later read moves it.
const PAST_JOB_END_DUE: Duration = Duration::from_millis(700);

/// A `RoleArn` matching the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`
/// (smithy_query operation=CreateProcessingJob).
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/SageMakerProcessingRole";

/// The two processing-job calls that take a `ProcessingJobName` as their target, with the
/// fixed text each one's refusal carries: the service words an unresolvable target
/// differently for a call that would change the job than for one that only reports it.
const CALLS: [(&str, &str); 2] = [
    ("StopProcessingJob", "Could not find job to update with name: "),
    (
        "DescribeProcessingJob",
        "Could not find requested job with name: ",
    ),
];

/// A minimal `CreateProcessingJob` body: every required member and nothing else.
///
/// `pub(crate)` so the sibling suite that exercises the stop target against a store this
/// one does not build keeps using this one body rather than a second copy of it.
pub(crate) fn minimal_create(name: &str) -> Value {
    json!({
        "ProcessingJobName": name,
        "RoleArn": ROLE_ARN,
        "AppSpecification": { "ImageUri": "000000000000.dkr.ecr.us-east-1.amazonaws.com/proc:1" },
        "ProcessingResources": {
            "ClusterConfig": {
                "InstanceCount": 1,
                // A member of `ProcessingInstanceType` (smithy_query shape
                // ProcessingClusterConfig -> ProcessingInstanceType).
                "InstanceType": "ml.m5.large",
                "VolumeSizeInGB": 30,
            },
        },
    })
}

/// One row of the miss table: how the name misses, the Region the call is signed for, and
/// the name itself.
struct Miss {
    case: &'static str,
    region: &'static str,
    name: &'static str,
}

/// The names a processing-job call cannot resolve, against a store holding `stored-proc-job`
/// in `us-east-1` (see [`seed`]). `ProcessingJobName` is `len 1..63` with pattern
/// `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` (smithy_query operation=StopProcessingJob), so the
/// rows sit on both published bounds and in between.
fn misses() -> Vec<Miss> {
    vec![
        Miss {
            case: "never created",
            region: "us-east-1",
            name: "absent-proc-job",
        },
        Miss {
            // `ProcessingJobName`'s minimum length of 1.
            case: "shortest name the model allows",
            region: "us-east-1",
            name: "z",
        },
        Miss {
            // `ProcessingJobName`'s maximum length of 63; the guard in the first test
            // asserts the table really reaches both bounds.
            case: "longest name the model allows",
            region: "eu-west-2",
            name: "a0123456789012345678901234567890123456789012345678901234567890b",
        },
        Miss {
            case: "a prefix of a stored name",
            region: "us-east-1",
            name: "stored-proc",
        },
        Miss {
            case: "a suffix added to a stored name",
            region: "us-east-1",
            name: "stored-proc-job2",
        },
        Miss {
            case: "a case shift of a stored name",
            region: "us-east-1",
            name: "STORED-PROC-JOB",
        },
        Miss {
            case: "a teardown name whose create never ran, in a third Region",
            region: "eu-central-1",
            name: "teardown-of-a-job-that-was-never-created",
        },
        Miss {
            case: "a single-digit name",
            region: "us-west-2",
            name: "7",
        },
    ]
}

/// Seed the one job every miss is judged against, so a refusal is about the name as sent
/// rather than about an empty store.
async fn seed(emulator: &Emulator) -> String {
    let created = emulator
        .call_in("us-east-1", "CreateProcessingJob", &minimal_create("stored-proc-job"))
        .await;
    assert_eq!(
        created.0, 200,
        "seeding the stored job must answer 200, got {} with {:?}",
        created.0, created.1
    );
    str_field(&created.1, "ProcessingJobArn")
}

/// RULE: a processing-job call whose `ProcessingJobName` resolves to no job is refused
/// with HTTP 400 `ValidationException`, whose message is that call class's fixed template
/// followed by the name the request presented and nothing else.
#[tokio::test]
async fn an_unresolvable_processing_job_name_is_refused_with_the_recorded_envelope() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The boundary rows must really sit on `ProcessingJobName`'s published bounds
    // (len 1..63), so a typo cannot quietly move the table off them.
    let lengths: Vec<usize> = misses().iter().map(|miss| miss.name.len()).collect();
    assert_eq!(
        (lengths.iter().min().copied(), lengths.iter().max().copied()),
        (Some(1), Some(63)),
        "expected the table to span both published bounds of ProcessingJobName, \
         got the lengths {lengths:?}"
    );

    for (operation, fixed_text) in CALLS {
        for miss in misses() {
            let case = format!("{operation} / {}", miss.case);
            let (status, refused) = emulator
                .call_in(
                    miss.region,
                    operation,
                    &json!({ "ProcessingJobName": miss.name }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a name that resolves to no job, \
                 got {status} with {refused:?}"
            );
            assert_eq!(
                error_code(&refused),
                "ValidationException",
                "case {case:?}: expected the error code ValidationException, \
                 got {refused:?}"
            );
            let expected = format!("{fixed_text}{}", miss.name);
            assert_eq!(
                message(&refused),
                expected,
                "case {case:?}: expected the refusal to read {expected:?}, got {refused:?}"
            );
        }
    }
}

/// RULE: the refusal's only variable part is the name, so two calls that differ only in
/// the name they present answer messages that differ only there — and the same name
/// answers the same message whatever Region it is signed for and however many jobs the
/// store holds.
#[tokio::test]
async fn only_the_requested_name_varies_in_the_refusal() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for (operation, fixed_text) in CALLS {
        // Same name, three Regions and a growing store: the message cannot move.
        let mut answered = Vec::new();
        for (index, region) in ["us-east-1", "eu-west-2", "ap-southeast-1"]
            .into_iter()
            .enumerate()
        {
            let (_, refused) = emulator
                .call_in(region, operation, &json!({ "ProcessingJobName": "one-name" }))
                .await;
            answered.push(message(&refused));
            // Another job in the store before the next Region's call. The name carries
            // the operation, so the store really grows on each pass of the outer loop
            // instead of the create being refused as a name already in use.
            let filler = format!("filler-{}-{index}", operation.to_lowercase());
            let (status, created) = emulator
                .call_in("us-east-1", "CreateProcessingJob", &minimal_create(&filler))
                .await;
            assert_eq!(
                status, 200,
                "case {operation:?}: expected the filler create {filler} to answer 200, \
                 got {status} with {created:?}"
            );
        }
        let expected = format!("{fixed_text}one-name");
        for (index, got) in answered.iter().enumerate() {
            assert_eq!(
                got, &expected,
                "case {operation:?} region #{index}: expected {expected:?}, got {got:?}"
            );
        }

        // Two different names: the messages differ by exactly the name.
        let (_, first) = emulator
            .call_in("us-east-1", operation, &json!({ "ProcessingJobName": "name-a" }))
            .await;
        let (_, second) = emulator
            .call_in("us-east-1", operation, &json!({ "ProcessingJobName": "name-bb" }))
            .await;
        assert_eq!(
            (message(&first), message(&second)),
            (format!("{fixed_text}name-a"), format!("{fixed_text}name-bb")),
            "case {operation:?}: expected the name to be the only variable part, got {:?} \
             and {:?}",
            message(&first),
            message(&second)
        );
    }
}

/// RULE: the refusal is a refusal — it stores nothing and changes nothing. A name the
/// stop could not resolve still resolves to no job afterwards, the create of that very
/// name then succeeds, and the job the store did hold keeps the status and ARN it had.
#[tokio::test]
async fn a_refused_call_leaves_the_store_as_it_found_it() {
    let emulator = Emulator::new();
    let stored_arn = seed(&emulator).await;
    // Read the stored job only once the clock has run its status out, so what the
    // refusals below are judged against is a status nothing is owed any more.
    tokio::time::sleep(PAST_JOB_END_DUE).await;

    let before = emulator
        .call_in(
            "us-east-1",
            "DescribeProcessingJob",
            &json!({ "ProcessingJobName": "stored-proc-job" }),
        )
        .await;
    assert_eq!(
        before.0, 200,
        "the stored job must read back, got {} with {:?}",
        before.0, before.1
    );

    for (operation, _) in CALLS {
        let name = format!("absent-for-{}", operation.to_lowercase());
        let (status, _) = emulator
            .call_in("us-east-1", operation, &json!({ "ProcessingJobName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected the unresolvable name to be refused, got {status}"
        );

        // Nothing was created for it: the read still refuses the same name.
        let (status, described) = emulator
            .call_in(
                "us-east-1",
                "DescribeProcessingJob",
                &json!({ "ProcessingJobName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {operation:?}: expected the name to still resolve to no job, got {status} \
             with {described:?}"
        );

        // And the name is still free, so a create under it is accepted.
        let (status, created) = emulator
            .call_in("us-east-1", "CreateProcessingJob", &minimal_create(&name))
            .await;
        assert_eq!(
            status, 200,
            "case {operation:?}: expected the refused name to still be free for a create, \
             got {status} with {created:?}"
        );
    }

    let after = emulator
        .call_in(
            "us-east-1",
            "DescribeProcessingJob",
            &json!({ "ProcessingJobName": "stored-proc-job" }),
        )
        .await;
    assert_eq!(
        str_field(&after.1, "ProcessingJobArn"),
        stored_arn,
        "expected the stored job to keep the ARN its create minted, got {:?}",
        after.1
    );
    assert_eq!(
        str_field(&after.1, "ProcessingJobStatus"),
        str_field(&before.1, "ProcessingJobStatus"),
        "expected the refusals to leave the stored job's status alone, got {:?}",
        after.1
    );
}
