// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's algorithm lifecycle:
//! `CreateAlgorithm` -> `DescribeAlgorithm` -> `DeleteAlgorithm`.
//!
//! Every case goes over the wire through the shared harness in
//! [`crate::inference_deployment_lifecycle`] (`POST /` with
//! `X-Amz-Target: SageMaker.<Op>`, `awsJson1_1`), so what these tests exercise is what a
//! real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every name shape and region, the create/describe round trip for every
//! optional member, the status the clock owes, `ArnOrName` resolution, the refusals for a
//! duplicate name and an absent algorithm, each published constraint bound, and the
//! store's quota.

use std::time::Duration;

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};
use crate::service_resolved_members::{
    materialised_inference_specification, materialised_training_specification,
};

/// Long enough to pass `state::ALGORITHM_COMPLETED_AFTER` (300 ms).
const PAST_COMPLETED_DUE: Duration = Duration::from_millis(600);

/// The three members `TrainingSpecification` marks required
/// (smithy_query query="shape" shape="TrainingSpecification": `TrainingImage`,
/// `SupportedTrainingInstanceTypes`, `TrainingChannels`), with one channel carrying the
/// members `ChannelSpecification` marks required.
fn training_specification(image: &str) -> Value {
    json!({
        "TrainingImage": image,
        "SupportedTrainingInstanceTypes": ["ml.m5.large"],
        "TrainingChannels": [{
            "Name": "train",
            "SupportedContentTypes": ["text/csv"],
            "SupportedInputModes": ["File"],
        }],
    })
}

/// A `CreateAlgorithm` body naming `name`, with the required members only.
fn minimal_create(name: &str) -> Value {
    json!({
        "AlgorithmName": name,
        "TrainingSpecification": training_specification("demo-training:1"),
    })
}

/// An `AlgorithmValidationSpecification` with the one profile the model allows
/// (`ValidationProfiles` is `len 1..1`), each member it marks required filled.
fn validation_specification(profile: &str) -> Value {
    json!({
        "ValidationRole": "arn:aws:iam::000000000000:role/SMValidationRole",
        "ValidationProfiles": [{
            "ProfileName": profile,
            "TrainingJobDefinition": {
                "TrainingInputMode": "File",
                "InputDataConfig": [{
                    "ChannelName": "train",
                    "DataSource": {
                        "S3DataSource": {
                            "S3DataType": "S3Prefix",
                            "S3Uri": "s3://example-bucket/train/",
                        },
                    },
                }],
                "OutputDataConfig": { "S3OutputPath": "s3://example-bucket/out/" },
                "ResourceConfig": {
                    "InstanceType": "ml.m5.large",
                    "InstanceCount": 1,
                    "VolumeSizeInGB": 10,
                },
                "StoppingCondition": { "MaxRuntimeInSeconds": 3600 },
            },
        }],
    })
}

/// RULE: `CreateAlgorithm` mints `arn:aws:sagemaker:<signed region>:<account>:algorithm/<name>`
/// for whatever name the request carries and whatever region it signed for, falling back
/// to the emulator's default region when the request names none.
#[tokio::test]
async fn created_algorithm_arns_name_the_signed_region_and_the_stored_name() {
    // Rows differ along the name's shape (one character, digits, hyphens, and the
    // 63-character modelled maximum of `EntityName`, len 1..63) and the region.
    let names = [
        "a",
        "Algo9",
        "demo-algorithm-with-hyphens",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    // `AlgorithmArn`'s pattern allows a 9..16-character region, so the rows span that
    // width rather than only the default region.
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");

            let (status, created) = emulator
                .call_in(region, "CreateAlgorithm", &minimal_create(name))
                .await;
            assert_eq!(
                status, 200,
                "case {case}: expected HTTP 200, got {status} {created}"
            );
            assert_eq!(
                str_field(&created, "AlgorithmArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:algorithm/{name}"),
                "case {case}: AlgorithmArn, got {created:?}"
            );

            // The ARN the reader replays is the one the create minted: it is stored, not
            // recomputed from the reading request.
            let (read_status, described) = emulator
                .call_in("us-east-1", "DescribeAlgorithm", &json!({ "AlgorithmName": name }))
                .await;
            assert_eq!(
                read_status, 200,
                "case {case}: DescribeAlgorithm, got {read_status} {described}"
            );
            assert_eq!(
                str_field(&described, "AlgorithmArn"),
                str_field(&created, "AlgorithmArn"),
                "case {case}: the stored ARN must survive a read from another region, \
                 got {described:?}"
            );
        }
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let created = emulator
        .ok("CreateAlgorithm", &minimal_create("unsigned-algorithm"))
        .await;
    assert_eq!(
        str_field(&created, "AlgorithmArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:algorithm/unsigned-algorithm"),
        "case unsigned: AlgorithmArn, got {created:?}"
    );
}

/// RULE: `DescribeAlgorithm` replays every member `CreateAlgorithm` stored, present
/// members and absent ones alike, and reports the `CreationTime` recorded at create.
#[tokio::test]
async fn describe_algorithm_replays_exactly_what_create_algorithm_stored() {
    // Rows differ along which optional members the create carries, the size of the
    // training specification (hyperparameters, channels, metrics) and the tag count.
    let cases: Vec<(&str, Value)> = vec![
        ("required-members-only", json!({})),
        (
            "description-and-certification",
            json!({
                "AlgorithmDescription": "A demo algorithm, with punctuation: commas, colons.",
                "CertifyForMarketplace": true,
            }),
        ),
        (
            "certification-explicitly-off-with-tags",
            json!({
                "CertifyForMarketplace": false,
                "Tags": [
                    { "Key": "team", "Value": "ml" },
                    { "Key": "env", "Value": "test" },
                ],
            }),
        ),
        (
            "inference-specification",
            json!({
                "InferenceSpecification": {
                    "Containers": [
                        { "Image": "inference:1" },
                        { "Image": "inference:2", "ContainerHostname": "second" },
                    ],
                    "SupportedTransformInstanceTypes": ["ml.m5.large"],
                    "SupportedRealtimeInferenceInstanceTypes": ["ml.c5.xlarge"],
                    "SupportedContentTypes": ["text/csv", "application/json"],
                    "SupportedResponseMIMETypes": ["text/csv"],
                },
            }),
        ),
        (
            "validation-specification",
            json!({ "ValidationSpecification": validation_specification("profile1") }),
        ),
        (
            "rich-training-specification",
            json!({
                "TrainingSpecification": {
                    "TrainingImage": "demo-training:2",
                    "TrainingImageDigest":
                        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
                    "SupportedTrainingInstanceTypes": ["ml.m5.large", "ml.p3.2xlarge"],
                    "SupportsDistributedTraining": true,
                    "SupportedHyperParameters": [
                        {
                            "Name": "epochs",
                            "Type": "Integer",
                            "IsTunable": true,
                            "IsRequired": false,
                            "DefaultValue": "10",
                            "Range": {
                                "IntegerParameterRangeSpecification": {
                                    "MinValue": "1",
                                    "MaxValue": "100",
                                },
                            },
                        },
                        { "Name": "mode", "Type": "FreeText", "IsRequired": true },
                    ],
                    "MetricDefinitions": [{ "Name": "loss", "Regex": "loss=(.*?);" }],
                    "TrainingChannels": [
                        {
                            "Name": "train",
                            "SupportedContentTypes": ["text/csv"],
                            "SupportedInputModes": ["File", "Pipe"],
                            "IsRequired": true,
                        },
                        {
                            "Name": "validation",
                            "Description": "Held-out data",
                            "SupportedContentTypes": ["text/csv"],
                            "SupportedInputModes": ["File"],
                            "IsRequired": false,
                        },
                    ],
                    "SupportedTuningJobObjectiveMetrics": [
                        { "Type": "Minimize", "MetricName": "loss" },
                    ],
                },
            }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let mut body = minimal_create(case);
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }

        let created = emulator.ok("CreateAlgorithm", &body).await;
        let described = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;

        assert_eq!(
            str_field(&described, "AlgorithmName"),
            case,
            "case {case}: AlgorithmName, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AlgorithmArn"),
            str_field(&created, "AlgorithmArn"),
            "case {case}: AlgorithmArn, got {described:?}"
        );
        // Every member the request carried comes back byte-for-byte.
        for member in [
            "AlgorithmDescription",
            "ValidationSpecification",
        ] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case}: {member} must replay what the create stored, got {described:?}"
            );
        }
        // The members the SERVICE supplies come back too: the certification flag an
        // uncertified algorithm carries, the digest it resolved for each image, and each
        // training channel's `IsRequired`.
        assert_eq!(
            described.get("CertifyForMarketplace"),
            Some(&json!(body
                .get("CertifyForMarketplace")
                .and_then(Value::as_bool)
                .unwrap_or(false))),
            "case {case}: CertifyForMarketplace, got {described:?}"
        );
        assert_eq!(
            described.get("TrainingSpecification"),
            Some(&materialised_training_specification(
                &body["TrainingSpecification"]
            )),
            "case {case}: TrainingSpecification, got {described:?}"
        );
        assert_eq!(
            described.get("InferenceSpecification"),
            body.get("InferenceSpecification")
                .map(materialised_inference_specification)
                .as_ref(),
            "case {case}: InferenceSpecification, got {described:?}"
        );
        // `Tags` is not a member of `DescribeAlgorithmOutput`, so it is never echoed
        // (smithy_query query="operation" operation="DescribeAlgorithm").
        assert!(
            described.get("Tags").is_none(),
            "case {case}: DescribeAlgorithm models no Tags member, got {described:?}"
        );
        // AWS Marketplace assigns `ProductId` when it lists the algorithm; this emulator
        // lists nothing there, so the member is absent.
        assert!(
            described.get("ProductId").is_none(),
            "case {case}: ProductId must be absent for an unlisted algorithm, \
             got {described:?}"
        );
        let creation_time = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case}: CreationTime, got {described:?}"));
        assert!(
            creation_time > 0.0,
            "case {case}: CreationTime must be the instant of the create, got {creation_time}"
        );

        // A second read reports the same creation time: it is stored, not re-taken.
        let reread = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            reread.get("CreationTime"),
            described.get("CreationTime"),
            "case {case}: CreationTime must be stable across reads, got {reread:?}"
        );
    }
}

/// RULE: a created algorithm is reported in the non-terminal `Pending` status until the
/// transition the clock owes falls due, after which it reports a TERMINAL status it never
/// leaves: `Completed` when there was nothing to validate, and `Failed` with a per-profile
/// `FailureReason` when the create asked for validation jobs this emulator does not run.
#[tokio::test]
async fn algorithm_status_is_pending_until_the_transition_the_clock_owes_falls_due() {
    // Rows: with and without a `ValidationSpecification` — the axis that decides which
    // terminal status is owed — plus the certification flag, which asks for an image scan
    // this emulator does not run either.
    let cases: Vec<(&str, Value, &str, usize)> = vec![
        ("no-validation", json!({}), "Completed", 0),
        (
            "no-validation-certified",
            json!({ "CertifyForMarketplace": true }),
            "Completed",
            0,
        ),
        (
            "with-validation",
            json!({ "ValidationSpecification": validation_specification("profile1") }),
            "Failed",
            1,
        ),
    ];

    for (case, extra, settled_status, validation_entries) in cases {
        let emulator = Emulator::new();
        let mut body = minimal_create(case);
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        emulator.ok("CreateAlgorithm", &body).await;

        // The span is never zero, so a caller reading straight after the create still
        // observes the transient status, with nothing claimed about the validation yet.
        let creating = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            str_field(&creating, "AlgorithmStatus"),
            "Pending",
            "case {case}: a fresh algorithm must report the transient status, got {creating:?}"
        );
        for entry in creating
            .get("AlgorithmStatusDetails")
            .and_then(|details| details.get("ValidationStatuses"))
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            assert_eq!(
                entry.get("Status").and_then(Value::as_str),
                Some("NotStarted"),
                "case {case}: nothing has run while the algorithm is Pending, got {entry:?}"
            );
        }

        tokio::time::sleep(PAST_COMPLETED_DUE).await;

        let settled = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            str_field(&settled, "AlgorithmStatus"),
            settled_status,
            "case {case}: the status past the owed transition, got {settled:?}"
        );

        // The terminal status is terminal: a later read reports the same one.
        let reread = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            str_field(&reread, "AlgorithmStatus"),
            settled_status,
            "case {case}: the terminal status must be stable across reads, got {reread:?}"
        );

        // `AlgorithmStatusDetails` carries one validation entry per requested profile,
        // each reporting the failure and WHY nothing ran, and no image-scan entry at all
        // because nothing scanned a container image.
        let details = settled
            .get("AlgorithmStatusDetails")
            .unwrap_or_else(|| panic!("case {case}: AlgorithmStatusDetails, got {settled:?}"));
        let validations = details
            .get("ValidationStatuses")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: ValidationStatuses, got {details:?}"));
        assert_eq!(
            validations.len(),
            validation_entries,
            "case {case}: one ValidationStatuses entry per profile, got {details:?}"
        );
        for entry in validations {
            assert_eq!(
                entry.get("Name").and_then(Value::as_str),
                Some("profile1"),
                "case {case}: a validation entry names its profile, got {entry:?}"
            );
            assert_eq!(
                entry.get("Status").and_then(Value::as_str),
                Some("Failed"),
                "case {case}: no validation job ran, so the entry cannot report success, \
                 got {entry:?}"
            );
            let reason = entry
                .get("FailureReason")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                reason.contains("runs no algorithm validation"),
                "case {case}: the failure must say what was not run, got {entry:?}"
            );
        }
        assert_eq!(
            details.get("ImageScanStatuses").and_then(Value::as_array),
            Some(&Vec::new()),
            "case {case}: nothing scanned an image, got {details:?}"
        );
    }
}

/// RULE: `DescribeAlgorithm`'s `AlgorithmName` is modelled `ArnOrName`, so the bare name
/// and the algorithm's own ARN resolve to the same stored algorithm.
#[tokio::test]
async fn describe_algorithm_resolves_an_arn_and_a_bare_name_to_the_same_algorithm() {
    // Rows differ along the name's shape and the region whose ARN the reader is handed.
    let cases = [
        ("one", "us-east-1"),
        ("two-with-hyphens", "eu-west-1"),
        ("Mixed9Case", "ap-southeast-2"),
    ];

    for (name, region) in cases {
        let emulator = Emulator::new();
        let created = emulator
            .call_in(region, "CreateAlgorithm", &minimal_create(name))
            .await;
        assert_eq!(created.0, 200, "case {name}: CreateAlgorithm {:?}", created.1);
        let arn = str_field(&created.1, "AlgorithmArn");

        let by_name = emulator
            .call_in(region, "DescribeAlgorithm", &json!({ "AlgorithmName": name }))
            .await;
        let by_arn = emulator
            .call_in(region, "DescribeAlgorithm", &json!({ "AlgorithmName": arn }))
            .await;
        assert_eq!(by_arn.0, 200, "case {name}: describe by ARN {:?}", by_arn.1);
        assert_eq!(
            by_arn.1, by_name.1,
            "case {name}: an ARN and a bare name must describe the same algorithm, \
             got {:?} vs {:?}",
            by_arn.1, by_name.1
        );
    }
}

/// RULE: a create whose name is already in use is refused, and the stored algorithm is
/// left exactly as it was rather than overwritten.
#[tokio::test]
async fn create_algorithm_refuses_a_name_already_in_use_and_keeps_the_stored_one() {
    // Rows: a second create differing from the first along description, certification and
    // training image — none of which may reach the stored algorithm.
    let collisions: Vec<(&str, Value)> = vec![
        ("identical-body", json!({})),
        (
            "different-description",
            json!({ "AlgorithmDescription": "a second description" }),
        ),
        (
            "different-training-image",
            json!({ "TrainingSpecification": training_specification("other-training:9") }),
        ),
        (
            "different-certification",
            json!({ "CertifyForMarketplace": true }),
        ),
    ];

    for (case, extra) in collisions {
        let emulator = Emulator::new();
        emulator
            .ok(
                "CreateAlgorithm",
                &json!({
                    "AlgorithmName": case,
                    "AlgorithmDescription": "the first description",
                    "TrainingSpecification": training_specification("first-training:1"),
                }),
            )
            .await;
        let before = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;

        let mut body = minimal_create(case);
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        let (status, parsed) = emulator.call("CreateAlgorithm", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: a name already in use must be refused, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code, got {parsed:?}"
        );
        assert!(
            message(&parsed).contains(&format!("{ACCOUNT}:algorithm/{case}")),
            "case {case}: the refusal must name the existing algorithm, got {parsed:?}"
        );

        let after = emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            after, before,
            "case {case}: the refused create must not change the stored algorithm, \
             got {after:?}"
        );
    }
}

/// RULE: `DeleteAlgorithm` removes the algorithm, so a later read refuses it and its name
/// is free again; a delete naming an algorithm that is not stored is refused with HTTP 400
/// rather than reported as a delete.
#[tokio::test]
async fn delete_algorithm_removes_the_algorithm_and_frees_its_name() {
    // Rows differ along how much the deleted algorithm carried, so the delete is asserted
    // over the class rather than one minimal record.
    let cases: Vec<(&str, Value)> = vec![
        ("plain", json!({})),
        (
            "with-validation",
            json!({ "ValidationSpecification": validation_specification("profile1") }),
        ),
        (
            "with-tags",
            json!({ "Tags": [{ "Key": "team", "Value": "ml" }] }),
        ),
    ];

    for (case, extra) in cases {
        let emulator = Emulator::new();
        let mut body = minimal_create(case);
        for (key, value) in extra.as_object().expect("a JSON object of extra members") {
            body[key] = value.clone();
        }
        emulator.ok("CreateAlgorithm", &body).await;
        emulator.ok("CreateAlgorithm", &minimal_create("survivor")).await;

        // An algorithm is deleted once its registration has SETTLED, so each delete here
        // waits past the transition the clock owes; the refusal before that instant is
        // pinned by `delete_algorithm_refuses_an_algorithm_whose_registration_is_pending`.
        tokio::time::sleep(PAST_COMPLETED_DUE).await;

        let deleted = emulator
            .ok("DeleteAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            deleted,
            json!({}),
            "case {case}: the modelled output has no members, got {deleted:?}"
        );

        let (status, parsed) = emulator
            .call("DescribeAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: the deleted algorithm must be gone, got {status} {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: error code after delete, got {parsed:?}"
        );

        // A second delete is refused: the entry is really gone, not emptied.
        let (repeat_status, repeat) = emulator
            .call("DeleteAlgorithm", &json!({ "AlgorithmName": case }))
            .await;
        assert_eq!(
            repeat_status, 400,
            "case {case}: deleting twice must be refused, got {repeat_status} {repeat}"
        );
        assert!(
            message(&repeat).contains(&format!("{ACCOUNT}:algorithm/{case}")),
            "case {case}: the refusal must name the algorithm it could not find, \
             got {repeat:?}"
        );

        // The sibling is untouched, and the freed name can be created again.
        emulator
            .ok("DescribeAlgorithm", &json!({ "AlgorithmName": "survivor" }))
            .await;
        let recreated = emulator.ok("CreateAlgorithm", &minimal_create(case)).await;
        assert_eq!(
            str_field(&recreated, "AlgorithmArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:algorithm/{case}"),
            "case {case}: the freed name must be creatable again, got {recreated:?}"
        );
    }

    // A delete naming an algorithm nothing created is refused.
    let emulator = Emulator::new();
    let (status, parsed) = emulator
        .call("DeleteAlgorithm", &json!({ "AlgorithmName": "absent" }))
        .await;
    assert_eq!(
        status, 400,
        "case absent: DeleteAlgorithm must refuse an unknown algorithm, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case absent: error code, got {parsed:?}"
    );
}

/// RULE: a model package's `SourceAlgorithmSpecification` records WHICH algorithm produced
/// that package — provenance of a past training, not a hold on the algorithm resource — so
/// `DeleteAlgorithm` is accepted while such a package exists, and the package keeps the
/// name it recorded afterwards.
#[tokio::test]
async fn delete_algorithm_is_accepted_while_a_model_package_records_it() {
    // Rows differ along how the package names the algorithm — `SourceAlgorithm$AlgorithmName`
    // is modelled `ArnOrName` (smithy_query query="shape" shape="SourceAlgorithm"), so both
    // the bare name and the algorithm's own ARN are spanned — along how many packages record
    // the one algorithm, and along the registration form (`ModelPackageName` registers an
    // unversioned package, `ModelPackageGroupName` a versioned one).
    enum Named {
        ByName,
        ByArn,
    }
    let cases: Vec<(&str, Named, usize, bool)> = vec![
        ("unversioned-by-name", Named::ByName, 1, false),
        ("unversioned-by-arn", Named::ByArn, 1, false),
        ("versioned-by-name", Named::ByName, 1, true),
        ("two-packages-by-arn", Named::ByArn, 2, true),
    ];

    for (case, named, packages, versioned) in cases {
        let emulator = Emulator::new();
        let algorithm = format!("{case}-algo");
        let created = emulator.ok("CreateAlgorithm", &minimal_create(&algorithm)).await;
        let algorithm_arn = str_field(&created, "AlgorithmArn");
        let recorded = match named {
            Named::ByName => algorithm.clone(),
            Named::ByArn => algorithm_arn.clone(),
        };

        let group = format!("{case}-group");
        if versioned {
            emulator
                .ok(
                    "CreateModelPackageGroup",
                    &json!({ "ModelPackageGroupName": group }),
                )
                .await;
        }

        // Each package records the algorithm, through the API that owns the registry.
        let mut package_arns = Vec::new();
        for index in 0..packages {
            let mut body = json!({
                "SourceAlgorithmSpecification": {
                    "SourceAlgorithms": [{
                        "AlgorithmName": recorded,
                        "ModelDataUrl": "s3://example-bucket/model.tar.gz",
                    }],
                },
            });
            if versioned {
                body["ModelPackageGroupName"] = json!(group);
            } else {
                body["ModelPackageName"] = json!(format!("{case}-pkg{index}"));
            }
            let package = emulator.ok("CreateModelPackage", &body).await;
            package_arns.push(str_field(&package, "ModelPackageArn"));
        }

        // The delete is ACCEPTED while every one of those packages still names it, once
        // the algorithm's own registration has settled: this case is about the package's
        // hold, not about the clock.
        tokio::time::sleep(PAST_COMPLETED_DUE).await;
        let (status, deleted) = emulator
            .call("DeleteAlgorithm", &json!({ "AlgorithmName": algorithm }))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: a recorded source algorithm must not block the delete, \
             got {status} {deleted}"
        );

        // The algorithm is gone, and every package still replays the name it recorded:
        // deleting the algorithm rewrites no package.
        let (read_status, read_body) = emulator
            .call("DescribeAlgorithm", &json!({ "AlgorithmName": algorithm }))
            .await;
        assert_eq!(
            read_status, 400,
            "case {case}: the deleted algorithm must be gone, got {read_status} {read_body}"
        );
        for package_arn in &package_arns {
            let described = emulator
                .ok(
                    "DescribeModelPackage",
                    &json!({ "ModelPackageName": package_arn }),
                )
                .await;
            let still_recorded = described
                .get("SourceAlgorithmSpecification")
                .and_then(|specification| specification.get("SourceAlgorithms"))
                .and_then(Value::as_array)
                .and_then(|algorithms| algorithms.first())
                .and_then(|algorithm| algorithm.get("AlgorithmName"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                still_recorded, recorded,
                "case {case}: {package_arn} must keep the recorded source algorithm, \
                 got {described:?}"
            );
        }
    }
}

/// RULE: a request outside a member's modelled constraint is refused with HTTP 400 at the
/// boundary, before any state is written.
#[tokio::test]
async fn requests_outside_the_modelled_algorithm_constraints_are_refused() {
    let emulator = Emulator::new();

    // Rows: one past each published bound these operations state — `AlgorithmName`
    // len 1..63 with pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`, `AlgorithmDescription`
    // len 0..1024, `Tags` len 0..50, `ValidationProfiles` len 1..1, and the two members
    // `CreateAlgorithm` marks required.
    let too_long_name = "a".repeat(64);
    let too_long_description = "d".repeat(1025);
    let too_many_tags: Vec<Value> = (0..51)
        .map(|index| json!({ "Key": format!("k{index}"), "Value": "v" }))
        .collect();
    let two_profiles = json!({
        "ValidationRole": "arn:aws:iam::000000000000:role/SMValidationRole",
        "ValidationProfiles": [
            validation_specification("profile1")["ValidationProfiles"][0].clone(),
            validation_specification("profile2")["ValidationProfiles"][0].clone(),
        ],
    });

    let cases: Vec<(&str, &str, Value)> = vec![
        ("empty-name", "CreateAlgorithm", minimal_create("")),
        (
            "name-one-past-63",
            "CreateAlgorithm",
            minimal_create(&too_long_name),
        ),
        (
            "name-outside-the-pattern",
            "CreateAlgorithm",
            minimal_create("-leading-hyphen"),
        ),
        (
            "description-one-past-1024",
            "CreateAlgorithm",
            json!({
                "AlgorithmName": "bad-description",
                "AlgorithmDescription": too_long_description,
                "TrainingSpecification": training_specification("demo:1"),
            }),
        ),
        (
            "tags-one-past-50",
            "CreateAlgorithm",
            json!({
                "AlgorithmName": "bad-tags",
                "TrainingSpecification": training_specification("demo:1"),
                "Tags": too_many_tags,
            }),
        ),
        (
            "two-validation-profiles",
            "CreateAlgorithm",
            json!({
                "AlgorithmName": "bad-profiles",
                "TrainingSpecification": training_specification("demo:1"),
                "ValidationSpecification": two_profiles,
            }),
        ),
        (
            "missing-training-specification",
            "CreateAlgorithm",
            json!({ "AlgorithmName": "no-training-spec" }),
        ),
        (
            "missing-algorithm-name",
            "CreateAlgorithm",
            json!({ "TrainingSpecification": training_specification("demo:1") }),
        ),
        (
            "describe-empty-name",
            "DescribeAlgorithm",
            json!({ "AlgorithmName": "" }),
        ),
        (
            "delete-empty-name",
            "DeleteAlgorithm",
            json!({ "AlgorithmName": "" }),
        ),
    ];

    for (case, target, body) in cases {
        let (status, parsed) = emulator.call(target, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: {target} must be refused, got {status} {parsed}"
        );
        // The error SHAPE is pinned too, not only the status: a boundary refusal is the
        // framework's `ValidationException`, never the limit error or a server failure.
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: {target} refusal error code, got {parsed:?}"
        );
    }

    // Nothing above reached state: every name they named is still absent.
    for name in [
        "bad-description",
        "bad-tags",
        "bad-profiles",
        "no-training-spec",
    ] {
        let (status, parsed) = emulator
            .call("DescribeAlgorithm", &json!({ "AlgorithmName": name }))
            .await;
        assert_eq!(
            status, 400,
            "case {name}: a refused create must store nothing, got {status} {parsed}"
        );
    }
}

/// RULE: the algorithm store is bounded, so a create past its quota is refused with the
/// modelled `ResourceLimitExceeded` and stores nothing — no caller grows the emulator's
/// state without end by varying the name — and a delete frees a slot again.
#[tokio::test]
async fn a_create_past_the_algorithm_quota_is_refused_with_the_modelled_limit_failure() {
    // The crate's own bound, read from the constant the handler enforces rather than
    // copied, so the test cannot drift from it.
    const QUOTA: usize = service_com_amazonaws_sagemaker_sagemaker::state::ALGORITHM_QUOTA;

    let emulator = Emulator::new();
    for index in 0..QUOTA {
        emulator
            .ok("CreateAlgorithm", &minimal_create(&format!("q{index:04}")))
            .await;
    }

    let overflow = format!("q{QUOTA:04}");
    let (status, parsed) = emulator
        .call("CreateAlgorithm", &minimal_create(&overflow))
        .await;
    assert_eq!(
        status, 400,
        "case overflow: a create past the quota must be refused, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceLimitExceeded",
        "case overflow: the refusal must use the modelled limit error, got {parsed:?}"
    );

    let (read_status, read_body) = emulator
        .call("DescribeAlgorithm", &json!({ "AlgorithmName": overflow }))
        .await;
    assert_eq!(
        read_status, 400,
        "case overflow: the refused record must not be stored, got {read_status} {read_body}"
    );

    // With the store at its bound, a name already in use is still told the specific
    // reason: the duplicate refusal wins over the limit one.
    let (duplicate_status, duplicate) = emulator
        .call("CreateAlgorithm", &minimal_create("q0000"))
        .await;
    assert_eq!(
        duplicate_status, 400,
        "case duplicate-at-the-bound: expected HTTP 400, got {duplicate_status} {duplicate}"
    );
    assert_eq!(
        error_code(&duplicate),
        "ValidationException",
        "case duplicate-at-the-bound: a stored name must be refused as in use, \
         got {duplicate:?}"
    );

    // A delete frees a slot, so the same create then succeeds: the bound is a live count,
    // not a one-way counter. The algorithm it releases has to have settled first.
    tokio::time::sleep(PAST_COMPLETED_DUE).await;
    emulator
        .ok(
            "DeleteAlgorithm",
            &json!({ "AlgorithmName": format!("q{:04}", QUOTA - 1) }),
        )
        .await;
    emulator
        .ok("CreateAlgorithm", &minimal_create(&overflow))
        .await;
}
