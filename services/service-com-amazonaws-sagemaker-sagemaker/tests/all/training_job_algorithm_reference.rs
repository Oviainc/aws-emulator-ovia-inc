// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the algorithm REFERENCE a training job records:
//! `CreateAlgorithm` -> `CreateTrainingJob` -> `DescribeTrainingJob`, across the
//! algorithm's deletion.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets. The harness and the
//! `CreateTrainingJob` body builders are the ones this target's training-job suites
//! already define.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: a job
//! that names a registered algorithm records the ARN that reference RESOLVES to —
//! whichever `ArnOrName` form the request spelled, and whatever Region it signed for — so
//! the reference still identifies the algorithm after the algorithm is deleted; while a
//! job that names a `TrainingImage` records no algorithm reference at all.

use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{str_field, Emulator, ACCOUNT};
use crate::training_job_admission::{image_specification, training_body};

/// Long enough to pass `state::ALGORITHM_COMPLETED_AFTER` (300 ms), so an algorithm has
/// left the `Pending` status a delete is refused in.
const PAST_ALGORITHM_COMPLETED_DUE: Duration =
    service_com_amazonaws_sagemaker_sagemaker::state::ALGORITHM_COMPLETED_AFTER
        .saturating_mul(2)
        .saturating_add(Duration::from_millis(100));

/// Register `name` in `region` and answer its ARN, so a job can reference an algorithm
/// that exists.
async fn create_algorithm(emulator: &Emulator, region: &str, name: &str) -> String {
    let (status, created) = emulator
        .call_in(
            region,
            "CreateAlgorithm",
            &json!({
                "AlgorithmName": name,
                "TrainingSpecification": {
                    "TrainingImage":
                        format!("{ACCOUNT}.dkr.ecr.us-east-1.amazonaws.com/trainer:1"),
                    "SupportedTrainingInstanceTypes": ["ml.m5.large"],
                    "TrainingChannels": [{
                        "Name": "train",
                        "SupportedContentTypes": ["text/csv"],
                        "SupportedInputModes": ["File"],
                    }],
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "seeding {region}/{name}: CreateAlgorithm, got {status} with {created}"
    );
    str_field(&created, "AlgorithmArn")
}

/// `DescribeTrainingJob`'s `AlgorithmSpecification.AlgorithmName`, or `None` when the
/// member is absent.
fn reported_algorithm(described: &Value) -> Option<String> {
    described
        .pointer("/AlgorithmSpecification/AlgorithmName")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// RULE: a training job that names a registered algorithm records the ARN that reference
/// resolves to, so `DescribeTrainingJob` reports the algorithm's ARN whichever
/// `ArnOrName` form the create spelled — and keeps reporting it after the algorithm is
/// deleted.
#[tokio::test]
async fn a_training_job_records_the_arn_of_the_algorithm_it_names() {
    // Rows differ along what the rule generalises over: the Region signed for (so the ARN
    // is the one that Region mints), the algorithm name's shape, and the `ArnOrName` form
    // the request spells the reference with — the bare name or the full ARN.
    let regions = ["us-east-1", "eu-west-1"];
    let names = ["a", "my-algo-7", "9algo"];

    for region in regions {
        for name in names {
            let expected_arn =
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:algorithm/{name}");
            for (form, reference) in [
                ("bare-name", name.to_string()),
                ("arn", expected_arn.clone()),
            ] {
                let case = format!("{region}/{name}/{form}");
                // One emulator per row, so each job is the only one in its store and the
                // algorithm it names was registered in the same Region.
                let emulator = Emulator::new();
                let algorithm_arn = create_algorithm(&emulator, region, name).await;
                assert_eq!(
                    algorithm_arn, expected_arn,
                    "case {case}: the algorithm's own ARN names the signed Region"
                );
                let job = format!("job-{form}");
                let (status, created) = emulator
                    .call_in(
                        region,
                        "CreateTrainingJob",
                        &training_body(
                            &job,
                            json!({ "AlgorithmName": reference, "TrainingInputMode": "File" }),
                            "s3://demo/output/",
                        ),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case}: CreateTrainingJob, got {status} with {created}"
                );

                let described = emulator
                    .call_in(region, "DescribeTrainingJob", &json!({ "TrainingJobName": &job }))
                    .await
                    .1;
                assert_eq!(
                    reported_algorithm(&described),
                    Some(algorithm_arn.clone()),
                    "case {case}: the job must report the algorithm's ARN, got {described:?}"
                );

                // The reference survives the algorithm: the job still identifies what it
                // ran once `DeleteAlgorithm` has released the algorithm itself, which it
                // may do once the algorithm's validation has reached a settled status.
                tokio::time::sleep(PAST_ALGORITHM_COMPLETED_DUE).await;
                let (delete_status, deleted) = emulator
                    .call_in(region, "DeleteAlgorithm", &json!({ "AlgorithmName": name }))
                    .await;
                assert_eq!(
                    delete_status, 200,
                    "case {case}: DeleteAlgorithm, got {delete_status} with {deleted}"
                );
                let after = emulator
                    .call_in(region, "DescribeTrainingJob", &json!({ "TrainingJobName": &job }))
                    .await
                    .1;
                assert_eq!(
                    reported_algorithm(&after),
                    Some(algorithm_arn),
                    "case {case}: the deleted algorithm's ARN must still be reported, \
                     got {after:?}"
                );
            }
        }
    }
}

/// RULE: a training job that names its algorithm by a `TrainingImage` records no
/// algorithm reference, so `DescribeTrainingJob` reports the image and no
/// `AlgorithmName`.
#[tokio::test]
async fn a_training_job_that_names_an_image_records_no_algorithm_reference() {
    for region in ["us-east-1", "ap-southeast-2"] {
        let emulator = Emulator::new();
        let job = "image-job";
        let (status, created) = emulator
            .call_in(
                region,
                "CreateTrainingJob",
                &training_body(job, image_specification(), "s3://demo/output/"),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {region}: CreateTrainingJob, got {status} with {created}"
        );

        let described = emulator
            .call_in(region, "DescribeTrainingJob", &json!({ "TrainingJobName": job }))
            .await
            .1;
        assert_eq!(
            reported_algorithm(&described),
            None,
            "case {region}: a job naming an image must report no AlgorithmName, \
             got {described:?}"
        );
        assert!(
            described
                .pointer("/AlgorithmSpecification/TrainingImage")
                .and_then(Value::as_str)
                .is_some(),
            "case {region}: the image the request named must be reported, got {described:?}"
        );
    }
}
