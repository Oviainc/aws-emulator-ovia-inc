// SPDX-License-Identifier: CC0-1.0

//! How Comprehend WORDS a refusal that the resource or the job a request named does
//! not exist, across the three planes that raise one: the custom-model plane, the
//! endpoint plane and the analysis-job plane.
//!
//! RULE: such a refusal is served with HTTP 400 — the status this frontend gives a
//! modelled client error — and carries the FIXED sentence the operation words it
//! with: the SAME text whatever identifier the request named, with no part of the
//! request in it (no ARN, name, version, region, account id or job id). Which
//! sentence it is follows the CLASS of thing the operation resolves, not the
//! identifier it was handed: the model-deletion path states that the MODEL could not
//! be found, every other path that resolves a resource by ARN states that the
//! RESOURCE could not be found, and the job plane states that the JOB could not be
//! found.
//!
//! [`every_not_found_refusal_carries_its_operations_fixed_sentence`] is a TABLE whose
//! rows differ along the operation, the plane it belongs to, the error shape it
//! raises and the sentence it words — and every row is tried over a SET of
//! identifiers that differ along what makes them name nothing: an unknown name, a
//! version of a model that has none, another Region, another account, another
//! partition, the shortest name the member's pattern admits, and for a job id its
//! length and character set. The recorded units are three rows of that table.
//!
//! [`the_sentence_follows_the_operation_not_the_identifier`] then asks two of those
//! operations about the SAME ARN — one that named a real model until a completed
//! deletion removed it — so the two sentences are proved to come from the operation
//! rather than from anything about the ARN, and a resource that DOES exist is still
//! served.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background.
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// The sentence every path that resolves a RESOURCE by ARN refuses an absent one
/// with: the service's own reason code and fixed text, carrying nothing from the
/// request. Written out here rather than imported from the crate, so the suite pins
/// the text instead of whatever the code currently believes.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "RESOURCE_NOT_FOUND: Could not find specified resource.";

/// The sentence the model-deletion path refuses an absent model with — the same kind
/// of fixed text, worded for the class of resource that operation deletes.
const MODEL_NOT_FOUND_MESSAGE: &str = "Could not find specified model.";

/// The sentence the analysis-job plane refuses an absent job with.
const JOB_NOT_FOUND_MESSAGE: &str = "Could not find specified job.";

/// Longest this suite waits for the clock to owe a classifier its next status. A
/// resource walks at most two phase spans (`crate::state::LIFECYCLE_PHASE_SPAN`
/// each) to a terminal status; this leaves head-room on a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Which set of absent identifiers a row is tried over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Plane {
    /// A `DocumentClassifierArn` / `ComprehendArn` naming a custom model.
    Model,
    /// A `ComprehendEndpointArn` naming an inference endpoint.
    Endpoint,
    /// A `ComprehendFlywheelArn` naming a flywheel.
    Flywheel,
    /// A `JobId` naming an asynchronous analysis job.
    Job,
}

/// One refusal: the operation that answers it, the request it answers, the error
/// shape it raises and the sentence that shape carries.
struct Row {
    operation: &'static str,
    plane: Plane,
    code: &'static str,
    message: &'static str,
    /// The request body, built from one absent identifier.
    body: fn(&str) -> Value,
}

/// Absent model ARNs, each with the reason it names nothing. Every value satisfies
/// `DocumentClassifierArn`'s own pattern
/// (`smithy_query(query="shape", shape="DocumentClassifierArn")`), so the boundary
/// validator admits it and the handler is what refuses it.
fn absent_model_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "an unknown name",
            format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/never-created"),
        ),
        (
            "a version of a model that has none",
            format!(
                "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/never-created/\
                 version/v3"
            ),
        ),
        (
            "another Region",
            format!("arn:aws:comprehend:us-east-2:{ACCOUNT_ID}:document-classifier/never-created"),
        ),
        (
            "another account",
            format!("arn:aws:comprehend:{REGION}:111122223333:document-classifier/never-created"),
        ),
        (
            "another partition",
            format!(
                "arn:aws-us-gov:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/never-created"
            ),
        ),
        (
            "the shortest name the pattern admits",
            format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/a"),
        ),
        (
            "a long hyphenated name",
            format!(
                "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/{}",
                "model-".repeat(20) + "z"
            ),
        ),
    ]
}

/// Absent endpoint ARNs, satisfying `ComprehendEndpointArn`'s pattern
/// (`smithy_query(query="shape", shape="ComprehendEndpointArn")`) — which admits both
/// resource types an endpoint can serve.
fn absent_endpoint_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "a classifier endpoint nobody created",
            format!(
                "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier-endpoint/\
                 never-created"
            ),
        ),
        (
            "a recognizer endpoint nobody created",
            format!(
                "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:entity-recognizer-endpoint/never-created"
            ),
        ),
        (
            "another Region",
            format!(
                "arn:aws:comprehend:ap-south-1:{ACCOUNT_ID}:document-classifier-endpoint/\
                 never-created"
            ),
        ),
        (
            "another account",
            format!(
                "arn:aws:comprehend:{REGION}:111122223333:document-classifier-endpoint/\
                 never-created"
            ),
        ),
        (
            "another partition",
            format!(
                "arn:aws-cn:comprehend:{REGION}:{ACCOUNT_ID}:entity-recognizer-endpoint/\
                 never-created"
            ),
        ),
        (
            "the shortest name the pattern admits",
            format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier-endpoint/a"),
        ),
    ]
}

/// Absent job ids, differing along length and character set inside what `JobId`
/// declares (`smithy_query(query="shape", shape="JobId")`: length 1..32, pattern
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$`) — including both published length bounds.
fn absent_job_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the member admits", "7".to_string()),
        (
            "the longest id the member admits",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        ("a readable name", "never-created-job".to_string()),
        (
            "the punctuation the pattern admits",
            "Job.id:with/odd=chars+%@_".to_string(),
        ),
        ("letters outside ASCII", "trabajó-inexistente".to_string()),
    ]
}

/// Absent flywheel ARNs, satisfying `ComprehendFlywheelArn`'s pattern
/// (`smithy_query(query="shape", shape="ComprehendFlywheelArn")`).
fn absent_flywheel_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "a flywheel nobody created",
            format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:flywheel/never-created"),
        ),
        (
            "another Region",
            format!("arn:aws:comprehend:eu-central-1:{ACCOUNT_ID}:flywheel/never-created"),
        ),
        (
            "another account",
            format!("arn:aws:comprehend:{REGION}:111122223333:flywheel/never-created"),
        ),
        (
            "the shortest name the pattern admits",
            format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:flywheel/a"),
        ),
    ]
}

/// The one policy document a custom-model resource policy can carry, so a row about
/// `PutResourcePolicy` is refused for the ARN it named and not for its document.
fn import_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
            "Action": "comprehend:ImportModel",
            "Resource": "*",
        }],
    })
    .to_string()
}

/// Every not-found refusal this service answers, one row per operation.
fn rows() -> Vec<Row> {
    vec![
        // The custom-model plane. Every one of these resolves a resource by ARN and
        // says so; the deletion is the one that names the model class instead.
        Row {
            operation: "DescribeDocumentClassifier",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "DocumentClassifierArn": arn }),
        },
        Row {
            operation: "StopTrainingDocumentClassifier",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "DocumentClassifierArn": arn }),
        },
        Row {
            operation: "DeleteDocumentClassifier",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: MODEL_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "DocumentClassifierArn": arn }),
        },
        Row {
            operation: "ListTagsForResource",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "ResourceArn": arn }),
        },
        Row {
            operation: "TagResource",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| {
                json!({ "ResourceArn": arn, "Tags": [{ "Key": "team", "Value": "research" }] })
            },
        },
        Row {
            operation: "UntagResource",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "ResourceArn": arn, "TagKeys": ["team"] }),
        },
        Row {
            operation: "DescribeResourcePolicy",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "ResourceArn": arn }),
        },
        Row {
            operation: "PutResourcePolicy",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "ResourceArn": arn, "ResourcePolicy": import_policy() }),
        },
        Row {
            operation: "DeleteResourcePolicy",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "ResourceArn": arn }),
        },
        // The endpoint plane. A creation resolves the model (or the flywheel that
        // owns one) the endpoint would serve from, so it answers the same not-found
        // as the reads do.
        Row {
            operation: "DescribeEndpoint",
            plane: Plane::Endpoint,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| json!({ "EndpointArn": arn }),
        },
        Row {
            operation: "CreateEndpoint",
            plane: Plane::Model,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| {
                json!({
                    "EndpointName": "not-found-endpoint",
                    "ModelArn": arn,
                    "DesiredInferenceUnits": 1,
                })
            },
        },
        Row {
            operation: "CreateEndpoint",
            plane: Plane::Flywheel,
            code: "ResourceNotFoundException",
            message: RESOURCE_NOT_FOUND_MESSAGE,
            body: |arn| {
                json!({
                    "EndpointName": "not-found-flywheel-endpoint",
                    "FlywheelArn": arn,
                    "DesiredInferenceUnits": 1,
                })
            },
        },
        // The analysis-job plane: a job is its own class of thing, and the reads and
        // the cancellations of every family word it the same way.
        Row {
            operation: "DescribeDominantLanguageDetectionJob",
            plane: Plane::Job,
            code: "JobNotFoundException",
            message: JOB_NOT_FOUND_MESSAGE,
            body: |job| json!({ "JobId": job }),
        },
        Row {
            operation: "StopDominantLanguageDetectionJob",
            plane: Plane::Job,
            code: "JobNotFoundException",
            message: JOB_NOT_FOUND_MESSAGE,
            body: |job| json!({ "JobId": job }),
        },
        Row {
            operation: "DescribeSentimentDetectionJob",
            plane: Plane::Job,
            code: "JobNotFoundException",
            message: JOB_NOT_FOUND_MESSAGE,
            body: |job| json!({ "JobId": job }),
        },
        Row {
            operation: "StopSentimentDetectionJob",
            plane: Plane::Job,
            code: "JobNotFoundException",
            message: JOB_NOT_FOUND_MESSAGE,
            body: |job| json!({ "JobId": job }),
        },
        Row {
            operation: "DescribeKeyPhrasesDetectionJob",
            plane: Plane::Job,
            code: "JobNotFoundException",
            message: JOB_NOT_FOUND_MESSAGE,
            body: |job| json!({ "JobId": job }),
        },
        Row {
            operation: "DescribePiiEntitiesDetectionJob",
            plane: Plane::Job,
            code: "JobNotFoundException",
            message: JOB_NOT_FOUND_MESSAGE,
            body: |job| json!({ "JobId": job }),
        },
    ]
}

#[tokio::test]
async fn every_not_found_refusal_carries_its_operations_fixed_sentence() {
    let comprehend = Comprehend::new();
    // One model that exists, so every refusal below is about the identifier it named
    // and not about an empty store.
    let live = create_classifier(&comprehend, "not-found-seed").await;

    for row in rows() {
        let operation = row.operation;
        let identifiers = match row.plane {
            Plane::Model => absent_model_arns(),
            Plane::Endpoint => absent_endpoint_arns(),
            Plane::Flywheel => absent_flywheel_arns(),
            Plane::Job => absent_job_ids(),
        };
        for (reason, identifier) in identifiers {
            let case = format!("{operation}, {reason}");
            let (status, body) = comprehend.call(operation, &(row.body)(&identifier)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: a modelled client refusal is served with 400, got {status}: \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                row.code,
                "case {case:?}: expected {}, got {body}",
                row.code
            );
            let message = error_message(&body);
            assert_eq!(
                message, row.message,
                "case {case:?}: expected {:?}, got {message:?}",
                row.message
            );
            // Nothing of the request may reach the sentence, so neither the whole
            // identifier nor any part of it a caller chose appears in it.
            let parts = identifier
                .split('/')
                .chain(std::iter::once(identifier.as_str()))
                .filter(|part| part.len() > 2);
            for part in parts {
                assert!(
                    !message.contains(part),
                    "case {case:?}: the refusal must echo nothing of the request, but \
                     {message:?} carries {part:?}"
                );
            }
        }
    }

    // The wording changed, the resolution did not: the model that DOES exist is still
    // reported.
    let reported = comprehend
        .ok(
            "DescribeDocumentClassifier",
            &json!({ "DocumentClassifierArn": &live }),
        )
        .await;
    assert_eq!(
        reported["DocumentClassifierProperties"]["DocumentClassifierArn"].as_str(),
        Some(live.as_str()),
        "case \"the model that exists\": expected it to still be reported, got {reported}"
    );
}

#[tokio::test]
async fn the_sentence_follows_the_operation_not_the_identifier() {
    let comprehend = Comprehend::new();
    let arn = create_classifier(&comprehend, "sentence-per-operation").await;

    // A model is deleted only once its training has ended, and the record is gone
    // once the clock owes its removal — at which point the SAME ARN, which named a
    // real model a moment ago, names nothing.
    let started = Instant::now();
    loop {
        let (status, body) = comprehend
            .call(
                "DeleteDocumentClassifier",
                &json!({ "DocumentClassifierArn": &arn }),
            )
            .await;
        if status == 200 {
            break;
        }
        assert_eq!(
            error_code(&body),
            "ResourceInUseException",
            "case \"delete a training model\": a model still training is in use, got {body}"
        );
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case \"delete a training model\": the training never ended"
        );
        sleep(Duration::from_millis(40));
    }
    let started = Instant::now();
    loop {
        let (status, _) = comprehend
            .call(
                "DescribeDocumentClassifier",
                &json!({ "DocumentClassifierArn": &arn }),
            )
            .await;
        if status == 400 {
            break;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case \"wait for the removal\": the deleted model was never removed"
        );
        sleep(Duration::from_millis(40));
    }

    // One ARN, two operations, two sentences: the model-deletion path words the
    // refusal for the class of resource it deletes, the read for a resource in
    // general.
    for (operation, expected) in [
        ("DeleteDocumentClassifier", MODEL_NOT_FOUND_MESSAGE),
        ("DescribeDocumentClassifier", RESOURCE_NOT_FOUND_MESSAGE),
        ("StopTrainingDocumentClassifier", RESOURCE_NOT_FOUND_MESSAGE),
    ] {
        let (status, body) = comprehend
            .call(operation, &json!({ "DocumentClassifierArn": &arn }))
            .await;
        assert_eq!(
            status, 400,
            "case {operation:?} after the removal: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_message(&body),
            expected,
            "case {operation:?} after the removal: expected {expected:?}, got {body}"
        );
    }
}

/// Create a custom model and return the ARN its creation minted.
///
/// The training itself ends in error here (this suite's emulator mounts no object
/// storage), which is beside the point: the record exists either way, and these
/// suites are about how an ABSENT one is worded.
async fn create_classifier(comprehend: &Comprehend, name: &str) -> String {
    let output = comprehend
        .ok(
            "CreateDocumentClassifier",
            &json!({
                "DocumentClassifierName": name,
                "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
                "LanguageCode": "en",
                "InputDataConfig": { "S3Uri": "s3://not-found-bucket/train.csv" },
            }),
        )
        .await;
    output["DocumentClassifierArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDocumentClassifier {name}: expected an ARN, got {output}"))
        .to_string()
}
