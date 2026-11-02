// SPDX-License-Identifier: CC0-1.0

//! Comprehend's PII entity detection jobs: `StartPiiEntitiesDetectionJob` and
//! `DescribePiiEntitiesDetectionJob`.
//!
//! RULE: a `Start*Job` call is accepted only when the submission satisfies every
//! precondition its model documents — for this family, the `Mode` that redacts needs
//! a `RedactionConfig` naming the entity types to redact, and the language must be
//! one the job supports — and a submission the service refuses mints NO job; an
//! accepted one is persisted whole and reported back by the Describe that reads it.
//!
//! The suites assert that over a table of modes and configurations; the issue's
//! `ONLY_REDACTION`-without-`RedactionConfig` request is ONE row of it.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background.
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    JOB_STATUS_VALUES, LANGUAGE_CODE_VALUES, PII_ENTITIES_DETECTION_MODE_VALUES,
};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// The text `InvalidRequestException` carries when the mode that redacts is
/// submitted without a `RedactionConfig`.
///
/// Amazon Comprehend's own fixed sentence for the `INPUT_CONFIG_INVALID_FOR_MODE`
/// reason code, remediation clause included: nothing in it is derived from the
/// request, so every refused row below carries exactly this. Written out here rather
/// than imported from the service, so the suite pins the documented text instead of
/// whatever the code currently believes.
const REDACTION_CONFIG_REQUIRED_MESSAGE: &str = "INPUT_CONFIG_INVALID_FOR_MODE: A \
    RedactionConfig definition is required for PII detection requests that set the Mode \
    parameter to ONLY_REDACTION. Add a RedactionConfig definition to your request, \
    including a non-empty list of PII entity types to redact, and try again.";

/// Longest a suite waits for the clock to owe a job its terminal status. The spans
/// in `crate::state` total 600 ms; this leaves ample head-room on a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// A `StartPiiEntitiesDetectionJob` body carrying `mode`, and `redaction_config` when
/// one is given.
fn submission(mode: &str, redaction_config: Option<Value>) -> Value {
    let mut body = json!({
        "JobName": "pii-demo-job",
        "LanguageCode": "en",
        "Mode": mode,
        "InputDataConfig": {
            "S3Uri": "s3://bucket/in/",
            "InputFormat": "ONE_DOC_PER_LINE",
        },
        "OutputDataConfig": {
            "S3Uri": "s3://bucket/out/",
            "KmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/abcd-1234",
        },
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
    });
    if let Some(config) = redaction_config {
        body["RedactionConfig"] = config;
    }
    body
}

/// A full redaction configuration: the entity types to redact, and how.
fn redaction_config() -> Value {
    json!({
        "PiiEntityTypes": ["NAME", "EMAIL"],
        "MaskMode": "MASK",
        "MaskCharacter": "*",
    })
}

/// The properties `DescribePiiEntitiesDetectionJob` reports for `job_id`.
async fn describe(comprehend: &Comprehend, job_id: &str) -> Value {
    let output = comprehend
        .ok(
            "DescribePiiEntitiesDetectionJob",
            &json!({ "JobId": job_id }),
        )
        .await;
    output["PiiEntitiesDetectionJobProperties"].clone()
}

#[tokio::test]
async fn the_mode_decides_whether_a_redaction_configuration_is_required() {
    let comprehend = Comprehend::new();

    // Rows differing along the mode and along what the configuration carries. The
    // rule: `ONLY_REDACTION` needs a `RedactionConfig` that includes
    // `PiiEntityTypes`; `ONLY_OFFSETS` needs none and is not refused for carrying
    // one.
    let rows: &[(&str, &str, Option<Value>, bool)] = &[
        ("offsets without a configuration", "ONLY_OFFSETS", None, true),
        (
            "offsets with a configuration",
            "ONLY_OFFSETS",
            Some(redaction_config()),
            true,
        ),
        ("redaction without a configuration", "ONLY_REDACTION", None, false),
        (
            "redaction with a configuration naming no entity types",
            "ONLY_REDACTION",
            Some(json!({ "MaskMode": "MASK", "MaskCharacter": "*" })),
            false,
        ),
        (
            "redaction with an empty entity-type list",
            "ONLY_REDACTION",
            Some(json!({ "PiiEntityTypes": [] })),
            false,
        ),
        (
            "redaction with the entity types alone",
            "ONLY_REDACTION",
            Some(json!({ "PiiEntityTypes": ["SSN"] })),
            true,
        ),
        (
            "redaction with a full configuration",
            "ONLY_REDACTION",
            Some(redaction_config()),
            true,
        ),
    ];

    // Every row covers a modelled `PiiEntitiesDetectionMode` value.
    for mode in PII_ENTITIES_DETECTION_MODE_VALUES {
        assert!(
            rows.iter().any(|(_, row_mode, _, _)| row_mode == mode),
            "the table must cover the modelled mode {mode:?}"
        );
    }

    for (case, mode, config, accepted) in rows {
        let mut body = submission(mode, config.clone());
        // The same token on the refused and the accepted call: a refusal that had
        // minted a job would resolve the token to it, so the accepted call proves
        // nothing was minted.
        let token = format!("tok-{}", case.replace(' ', "-"));
        body["ClientRequestToken"] = json!(token);
        let (status, response) = comprehend
            .call("StartPiiEntitiesDetectionJob", &body)
            .await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {response}"
            );
            assert_eq!(
                response["JobStatus"].as_str(),
                Some("SUBMITTED"),
                "case {case:?}: a freshly started job must be SUBMITTED, got {response}"
            );
            continue;
        }

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {response}"
        );
        assert_eq!(
            error_message(&response),
            REDACTION_CONFIG_REQUIRED_MESSAGE,
            "case {case:?}: expected the service's fixed INPUT_CONFIG_INVALID_FOR_MODE \
             text, got {response}"
        );

        // No job was minted: the token is still free, so a compliant submission
        // carrying it starts a job whose mode is the compliant one.
        let mut retry = submission("ONLY_OFFSETS", None);
        retry["ClientRequestToken"] = json!(token);
        let accepted = comprehend.ok("StartPiiEntitiesDetectionJob", &retry).await;
        let job_id = accepted["JobId"].as_str().unwrap_or_default();
        let properties = describe(&comprehend, job_id).await;
        assert_eq!(
            properties["Mode"].as_str(),
            Some("ONLY_OFFSETS"),
            "case {case:?}: a refused submission must mint no job, but the token resolved \
             to one: {properties}"
        );
    }
}

#[tokio::test]
async fn a_started_job_is_persisted_and_described_from_the_store() {
    let comprehend = Comprehend::new();

    for (case, mode, config) in [
        ("offsets", "ONLY_OFFSETS", None),
        ("redaction", "ONLY_REDACTION", Some(redaction_config())),
    ] {
        let body = submission(mode, config);
        let output = comprehend.ok("StartPiiEntitiesDetectionJob", &body).await;
        let job_id = output["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JobId, got {output}"))
            .to_string();
        let job_arn = output["JobArn"].as_str().unwrap_or_default().to_string();

        assert!(
            job_id.len() == 32 && job_id.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected a 32-character hex JobId, got {job_id:?}"
        );
        // The ARN form the output member's own documentation states.
        assert_eq!(
            job_arn,
            format!(
                "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:pii-entities-detection-job/{job_id}"
            ),
            "case {case:?}: unexpected JobArn"
        );

        let properties = describe(&comprehend, &job_id).await;
        assert_eq!(
            properties["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: Describe reported a different JobId: {properties}"
        );
        assert_eq!(
            properties["JobArn"].as_str(),
            Some(job_arn.as_str()),
            "case {case:?}: Describe reported a different JobArn: {properties}"
        );

        // Every member Start was called with comes back unchanged.
        for member in [
            "JobName",
            "LanguageCode",
            "Mode",
            "InputDataConfig",
            "OutputDataConfig",
            "RedactionConfig",
            "DataAccessRoleArn",
        ] {
            let submitted = body.get(member).cloned().unwrap_or(Value::Null);
            let reported = properties.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported, submitted,
                "case {case:?}: Describe must report the {member} Start was called with, \
                 got {reported} for {submitted}"
            );
        }

        assert!(
            properties["SubmitTime"].as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: expected a populated SubmitTime, got {properties}"
        );
        assert!(
            JOB_STATUS_VALUES.contains(&properties["JobStatus"].as_str().unwrap_or_default()),
            "case {case:?}: unexpected JobStatus in {properties}"
        );
    }
}

#[tokio::test]
async fn only_the_languages_the_job_supports_are_accepted() {
    let comprehend = Comprehend::new();

    // The documented set (`LanguageCode`: "Enter the language code for English (en)
    // or Spanish (es)"), asserted over EVERY modelled `LanguageCode` value.
    for code in LANGUAGE_CODE_VALUES {
        let mut body = submission("ONLY_OFFSETS", None);
        body["LanguageCode"] = json!(code);
        let (status, response) = comprehend
            .call("StartPiiEntitiesDetectionJob", &body)
            .await;

        if matches!(*code, "en" | "es") {
            assert_eq!(
                status, 200,
                "case {code:?}: expected 200, got {status}: {response}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {code:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {code:?}: expected ValidationException, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn repeating_a_client_request_token_returns_the_first_job() {
    let comprehend = Comprehend::new();
    let mut body = submission("ONLY_OFFSETS", None);
    body["ClientRequestToken"] = json!("repeated-token");

    let first = comprehend.ok("StartPiiEntitiesDetectionJob", &body).await;
    let repeated = comprehend.ok("StartPiiEntitiesDetectionJob", &body).await;
    assert_eq!(
        repeated["JobId"], first["JobId"],
        "case \"repeated token\": expected the first JobId, got {repeated}"
    );

    // And a submission without a token mints its own job every time.
    let plain = submission("ONLY_OFFSETS", None);
    let one = comprehend.ok("StartPiiEntitiesDetectionJob", &plain).await;
    let two = comprehend.ok("StartPiiEntitiesDetectionJob", &plain).await;
    assert_ne!(
        one["JobId"], two["JobId"],
        "case \"no token\": each submission must mint its own JobId"
    );
}

#[tokio::test]
async fn describe_refuses_a_job_id_this_family_never_minted() {
    let comprehend = Comprehend::new();
    // One real PII job, so a refusal is about the id and not about an empty store.
    comprehend
        .ok(
            "StartPiiEntitiesDetectionJob",
            &submission("ONLY_OFFSETS", None),
        )
        .await;

    // A job of ANOTHER family is not a PII job: each family has its own id space.
    let sentiment = comprehend
        .ok(
            "StartSentimentDetectionJob",
            &json!({
                "LanguageCode": "en",
                "InputDataConfig": { "S3Uri": "s3://bucket/in/" },
                "OutputDataConfig": { "S3Uri": "s3://bucket/out/" },
                "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
            }),
        )
        .await;
    let foreign = sentiment["JobId"].as_str().unwrap_or_default().to_string();

    for job_id in [
        "doesnotexist".to_string(),
        "00000000000000000000000000000000".to_string(),
        "a".to_string(),
        foreign,
    ] {
        let (status, body) = comprehend
            .call(
                "DescribePiiEntitiesDetectionJob",
                &json!({ "JobId": job_id }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {job_id:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "JobNotFoundException",
            "case {job_id:?}: expected JobNotFoundException, got {body}"
        );
    }
}

#[tokio::test]
async fn a_started_job_ends_in_the_modelled_failure_naming_what_was_unreadable() {
    let comprehend = Comprehend::new();
    let output = comprehend
        .ok(
            "StartPiiEntitiesDetectionJob",
            &submission("ONLY_OFFSETS", None),
        )
        .await;
    let job_id = output["JobId"].as_str().unwrap_or_default().to_string();

    // The run this emulator does not perform for this family, so the job ends in the
    // modelled failure state with a Message that names both locations and says the
    // analysis was not performed, rather than reporting a COMPLETED run.
    let started = Instant::now();
    loop {
        let properties = describe(&comprehend, &job_id).await;
        let status = properties["JobStatus"].as_str().unwrap_or_default();
        assert!(
            matches!(status, "SUBMITTED" | "IN_PROGRESS" | "FAILED"),
            "case \"lifecycle\": unexpected JobStatus in {properties}"
        );
        if status == "FAILED" {
            let message = properties["Message"].as_str().unwrap_or_default();
            for expected in ["s3://bucket/in/", "s3://bucket/out/", "not performed"] {
                assert!(
                    message.contains(expected),
                    "case \"lifecycle\": expected the failure Message to mention \
                     {expected:?}, got {message:?}"
                );
            }
            assert!(
                properties["EndTime"].as_f64().is_some_and(|t| t > 0.0),
                "case \"lifecycle\": a terminal job carries an EndTime, got {properties}"
            );
            break;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case \"lifecycle\": the job never reached FAILED"
        );
        sleep(Duration::from_millis(40));
    }
}
