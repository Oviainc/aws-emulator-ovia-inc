// SPDX-License-Identifier: CC0-1.0

//! Comprehend's asynchronous key-phrases analysis job lifecycle:
//! `StartKeyPhrasesDetectionJob` and `DescribeKeyPhrasesDetectionJob`.
//!
//! The rule under test is not about one family: a `Start*DetectionJob` persists the
//! submission under a freshly minted `JobId`, answers `SUBMITTED` with the ARN
//! `arn:aws:comprehend:<region>:<account>:<family-resource-type>/<JobId>`, and the
//! family's `Describe*` reports that record back unchanged. So every suite below is
//! a TABLE over the implemented families × a set of submissions, and each family
//! keeps its own id and idempotency-token space.
//!
//! `sentiment_jobs.rs` covers the sentiment family's `List*`/`Stop*` pair; this
//! suite is the family-parametrised half, with key phrases as its first row.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background — if a suite in this file ever spawns concurrent work,
// switch to a timer rather than blocking the runtime thread.
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    JOB_STATUS_VALUES, LANGUAGE_CODE_VALUES,
};

use crate::comprehend_client::{error_code, Comprehend, ACCOUNT_ID, REGION};

/// One `Start*DetectionJob`/`Describe*DetectionJob` family, as the model names it.
struct Family {
    /// The operation that submits a job.
    start: &'static str,
    /// The operation that reports a submitted job.
    describe: &'static str,
    /// The output member `describe` reports the record under.
    properties: &'static str,
    /// The resource type this family's `JobArn` carries, from the `JobArn`
    /// documentation of `start` in the Smithy model (e.g.
    /// `arn::comprehend:::key-phrases-detection-job/`).
    arn_resource_type: &'static str,
}

/// Every analysis-job family this emulator implements. The rule holds for each.
const FAMILIES: &[Family] = &[
    Family {
        start: "StartKeyPhrasesDetectionJob",
        describe: "DescribeKeyPhrasesDetectionJob",
        properties: "KeyPhrasesDetectionJobProperties",
        arn_resource_type: "key-phrases-detection-job",
    },
    Family {
        start: "StartSentimentDetectionJob",
        describe: "DescribeSentimentDetectionJob",
        properties: "SentimentDetectionJobProperties",
        arn_resource_type: "sentiment-detection-job",
    },
];

/// The members `Start*DetectionJob` persists and `Describe*` reports back.
const PERSISTED_MEMBERS: &[&str] = &[
    "JobName",
    "LanguageCode",
    "InputDataConfig",
    "OutputDataConfig",
    "DataAccessRoleArn",
    "VolumeKmsKeyId",
    "VpcConfig",
];

/// The lifecycle order a job's status may only move forward along.
///
/// A started job of either family ends `FAILED` in THIS suite, whose emulator mounts
/// no object storage: running the job means reading the documents from the Amazon S3
/// location `InputDataConfig` names and writing analysed output back, and nothing
/// answers those calls here, so the run reports what it could not read.
/// `key_phrases_job_run` is the suite that mounts object storage and asserts the
/// COMPLETED run and the archive it wrote.
const LIFECYCLE: &[&str] = &["SUBMITTED", "IN_PROGRESS", "FAILED"];

/// The terminal status a started job reaches, as `JOB_STATUS_VALUES` spells it.
const TERMINAL_STATUS: &str = "FAILED";

/// Longest a suite waits for the clock to owe a job its terminal status. A job walks
/// two phase spans (`crate::state::LIFECYCLE_PHASE_SPAN` each) to a terminal status;
/// this leaves ample head-room on a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The most tags a Comprehend resource can hold, from the modelled
/// `TooManyTagsException` documentation: "50 tags per resource". Written out here
/// rather than imported from the service, so the suite pins the documented bound
/// instead of whatever the code currently believes.
const MAX_TAGS: usize = 50;

/// A submission body that differs from the others along every member a
/// `Start*DetectionJob` persists: which optional members are present, the language,
/// the input format, the data locations and the tags.
fn submission(case: &str) -> Value {
    match case {
        "every optional member set" => json!({
            "JobName": "key-phrases-demo",
            "LanguageCode": "en",
            "InputDataConfig": {
                "S3Uri": "s3://kp-bucket/in/",
                "InputFormat": "ONE_DOC_PER_LINE",
            },
            "OutputDataConfig": {
                "S3Uri": "s3://kp-bucket/out/",
                "KmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/9876-5432",
            },
            "DataAccessRoleArn": "arn:aws:iam::123456789012:role/KeyPhrasesRole",
            "VolumeKmsKeyId": "abcd1234-12ab-34cd-56ef-1234567890ab",
            "VpcConfig": {
                "SecurityGroupIds": ["sg-00112233445566778", "sg-0aabbccddeeff0011"],
                "Subnets": ["subnet-00112233445566778"],
            },
            "Tags": [{ "Key": "team", "Value": "nlp" }],
        }),
        "only the required members" => json!({
            "LanguageCode": "ko",
            "InputDataConfig": { "S3Uri": "s3://kp-other/corpus/" },
            "OutputDataConfig": { "S3Uri": "s3://kp-other/results/" },
            "DataAccessRoleArn": "arn:aws:iam::210987654321:role/nested/path/OtherRole",
        }),
        "a document-reader input and a punctuated job name" => json!({
            "JobName": "pt.batch:2026/04=final+1%",
            "LanguageCode": "pt",
            "InputDataConfig": {
                "S3Uri": "s3://kp-third/in",
                "InputFormat": "ONE_DOC_PER_FILE",
                "DocumentReaderConfig": {
                    "DocumentReadAction": "TEXTRACT_ANALYZE_DOCUMENT",
                    "DocumentReadMode": "FORCE_DOCUMENT_READ_ACTION",
                    "FeatureTypes": ["TABLES", "FORMS"],
                },
            },
            "OutputDataConfig": { "S3Uri": "s3://kp-third/out" },
            "DataAccessRoleArn": "arn:aws:iam::000000000000:role/Third",
        }),
        "a job name at the modelled length ceiling" => json!({
            // The modelled `JobName` ceiling is 256 characters.
            "JobName": "n".repeat(256),
            "LanguageCode": "zh-TW",
            "InputDataConfig": { "S3Uri": "s3://kp-fourth/in/deep/prefix/" },
            "OutputDataConfig": { "S3Uri": "s3://kp-fourth/out/deep/prefix/" },
            "DataAccessRoleArn": "arn:aws-cn:iam::111122223333:role/Fourth",
            "Tags": [],
        }),
        // A case name no arm above builds is a typo in SUBMISSION_CASES, never a
        // silent fallback to some other body.
        other => panic!("unknown submission case {other:?}"),
    }
}

/// The cases [`submission`] builds.
const SUBMISSION_CASES: &[&str] = &[
    "every optional member set",
    "only the required members",
    "a document-reader input and a punctuated job name",
    "a job name at the modelled length ceiling",
];

/// Submit `body` to `family`, returning `(JobId, JobArn)`.
async fn start(comprehend: &Comprehend, family: &Family, case: &str, body: &Value) -> (String, String) {
    let output = comprehend.ok(family.start, body).await;
    let job_id = output["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}/{case:?}: expected a JobId, got {output}", family.start))
        .to_string();
    let job_arn = output["JobArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}/{case:?}: expected a JobArn, got {output}", family.start))
        .to_string();
    assert_eq!(
        output["JobStatus"].as_str(),
        Some("SUBMITTED"),
        "case {}/{case:?}: a freshly started job must be SUBMITTED, got {output}",
        family.start
    );
    (job_id, job_arn)
}

/// The job properties `family`'s Describe reports for `job_id`.
async fn describe(comprehend: &Comprehend, family: &Family, job_id: &str) -> Value {
    let output = comprehend
        .ok(family.describe, &json!({ "JobId": job_id }))
        .await;
    output[family.properties].clone()
}

#[tokio::test]
async fn a_started_job_is_persisted_and_described_from_the_store() {
    let comprehend = Comprehend::new();

    for family in FAMILIES {
        for case in SUBMISSION_CASES {
            let body = submission(case);
            let (job_id, job_arn) = start(&comprehend, family, case, &body).await;

            // The minted id and ARN: 32 hex characters, and the modelled ARN form
            // with this family's resource type and the signed-for region.
            assert!(
                job_id.len() == 32 && job_id.chars().all(|c| c.is_ascii_hexdigit()),
                "case {}/{case:?}: expected a 32-character hex JobId, got {job_id:?}",
                family.start
            );
            assert_eq!(
                job_arn,
                format!(
                    "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:{}/{job_id}",
                    family.arn_resource_type
                ),
                "case {}/{case:?}: unexpected JobArn",
                family.start
            );

            let properties = describe(&comprehend, family, &job_id).await;
            assert_eq!(
                properties["JobId"].as_str(),
                Some(job_id.as_str()),
                "case {}/{case:?}: Describe reported a different JobId: {properties}",
                family.describe
            );
            assert_eq!(
                properties["JobArn"].as_str(),
                Some(job_arn.as_str()),
                "case {}/{case:?}: Describe reported a different JobArn: {properties}",
                family.describe
            );

            // Every member Start was called with comes back unchanged.
            for member in PERSISTED_MEMBERS {
                let submitted = body.get(member).cloned().unwrap_or(Value::Null);
                let reported = properties.get(member).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    reported, submitted,
                    "case {}/{case:?}: Describe must report the {member} Start was called \
                     with, got {reported} for {submitted}",
                    family.describe
                );
            }

            assert!(
                properties["SubmitTime"].as_f64().is_some_and(|t| t > 0.0),
                "case {}/{case:?}: expected a populated SubmitTime, got {properties}",
                family.describe
            );
            assert!(
                JOB_STATUS_VALUES.contains(&properties["JobStatus"].as_str().unwrap_or_default()),
                "case {}/{case:?}: unexpected JobStatus in {properties}",
                family.describe
            );
            // `Tags` is not a member of the properties shape, so it must not appear.
            assert!(
                properties.get("Tags").is_none(),
                "case {}/{case:?}: {} models no Tags member, got {properties}",
                family.describe,
                family.properties
            );
        }
    }
}

#[tokio::test]
async fn every_modelled_language_code_is_persisted_as_submitted() {
    let comprehend = Comprehend::new();

    // The catalog is the model's own `LanguageCode` enum, so this covers the
    // codes the issue never cited as well as the one it did.
    for family in FAMILIES {
        for language in LANGUAGE_CODE_VALUES {
            let mut body = submission("only the required members");
            body["LanguageCode"] = json!(language);
            let (job_id, _) = start(&comprehend, family, language, &body).await;
            let properties = describe(&comprehend, family, &job_id).await;
            assert_eq!(
                properties["LanguageCode"].as_str(),
                Some(*language),
                "case {}/{language:?}: Describe must report the submitted LanguageCode, \
                 got {properties}",
                family.describe
            );
        }
    }
}

#[tokio::test]
async fn repeating_a_client_request_token_returns_the_first_job() {    let comprehend = Comprehend::new();

    for family in FAMILIES {
        for case in SUBMISSION_CASES {
            let mut body = submission(case);
            body["ClientRequestToken"] = json!(format!("tok-{}", case.replace(' ', "-")));

            let (first_id, first_arn) = start(&comprehend, family, case, &body).await;
            let repeated = comprehend.ok(family.start, &body).await;
            assert_eq!(
                repeated["JobId"].as_str(),
                Some(first_id.as_str()),
                "case {}/{case:?}: a repeated ClientRequestToken must return the first \
                 JobId, got {repeated}",
                family.start
            );
            assert_eq!(
                repeated["JobArn"].as_str(),
                Some(first_arn.as_str()),
                "case {}/{case:?}: a repeated ClientRequestToken must return the first \
                 JobArn, got {repeated}",
                family.start
            );

            // A token reused with DIFFERENT parameters is a different submission,
            // not a retry, so it gets a job of its own — and the first job's
            // record is left exactly as it was submitted.
            for (member, value) in [
                ("JobName", json!("renamed-on-retry")),
                ("LanguageCode", json!("de")),
                (
                    "OutputDataConfig",
                    json!({ "S3Uri": "s3://elsewhere/out/" }),
                ),
                ("Tags", json!([{ "Key": "extra", "Value": "tag" }])),
            ] {
                let mut mismatched = body.clone();
                mismatched[member] = value.clone();
                let again = comprehend.ok(family.start, &mismatched).await;
                let again_id = again["JobId"].as_str().unwrap_or_default();
                assert_ne!(
                    again_id, first_id,
                    "case {}/{case:?}: a token reused with a different {member} must create \
                     a second job, got {again}",
                    family.start
                );
                // And that second job really is the mismatched submission.
                let second = describe(&comprehend, family, again_id).await;
                if member != "Tags" {
                    assert_eq!(
                        second.get(member).cloned().unwrap_or(Value::Null),
                        value,
                        "case {}/{case:?}: the second job must carry the {member} it was \
                         submitted with, got {second}",
                        family.describe
                    );
                }
            }
            let properties = describe(&comprehend, family, &first_id).await;
            for member in PERSISTED_MEMBERS {
                let submitted = body.get(member).cloned().unwrap_or(Value::Null);
                let reported = properties.get(member).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    reported, submitted,
                    "case {}/{case:?}: a reused token must not rewrite the first job's \
                     {member}, got {reported} for {submitted}",
                    family.describe
                );
            }
        }
    }

    // Each family keeps its own token space: one token, one job per family.
    let mut body = submission("only the required members");
    body["ClientRequestToken"] = json!("shared-across-families");
    let mut ids = Vec::new();
    for family in FAMILIES {
        let (job_id, _) = start(&comprehend, family, "shared token", &body).await;
        ids.push(job_id);
    }
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        FAMILIES.len(),
        "case \"shared token\": each family must mint its own job, got {ids:?}"
    );
}

#[tokio::test]
async fn a_submission_without_a_token_creates_a_new_job_every_time() {
    let comprehend = Comprehend::new();
    let body = submission("only the required members");

    for family in FAMILIES {
        let (first, _) = start(&comprehend, family, "first", &body).await;
        let (second, _) = start(&comprehend, family, "second", &body).await;
        assert_ne!(
            first, second,
            "case {}/\"no ClientRequestToken\": each submission must mint its own JobId",
            family.start
        );
    }
}

#[tokio::test]
async fn a_submission_carrying_more_tags_than_a_resource_can_hold_is_refused() {
    let comprehend = Comprehend::new();

    // The bound the `TooManyTagsException` documentation states ("50 tags per
    // resource"): below it, at it, and one past it.
    for family in FAMILIES {
        for count in [0usize, 1, MAX_TAGS - 1, MAX_TAGS, MAX_TAGS + 1] {
            let mut body = submission("only the required members");
            body["Tags"] = Value::Array(
                (0..count)
                    .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
                    .collect(),
            );
            let (status, response) = comprehend.call(family.start, &body).await;

            if count > MAX_TAGS {
                assert_eq!(
                    status, 400,
                    "case {}/{count} tags: expected 400, got {status}: {response}",
                    family.start
                );
                assert_eq!(
                    error_code(&response),
                    "TooManyTagsException",
                    "case {}/{count} tags: expected TooManyTagsException, got {response}",
                    family.start
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {}/{count} tags: expected 200, got {status}: {response}",
                    family.start
                );
            }
        }
    }
}

#[tokio::test]
async fn describe_refuses_a_job_id_this_family_never_minted() {
    let comprehend = Comprehend::new();

    // One real job per family, so a refusal is about the id and not an empty store.
    let mut minted = Vec::new();
    for family in FAMILIES {
        let (job_id, _) = start(
            &comprehend,
            family,
            "seed",
            &submission("only the required members"),
        )
        .await;
        minted.push(job_id);
    }

    for (index, family) in FAMILIES.iter().enumerate() {
        // An unknown id, and — the family-scoping half of the rule — the id of a
        // job another family really did mint.
        let foreign: Vec<String> = minted
            .iter()
            .enumerate()
            .filter(|(other, _)| *other != index)
            .map(|(_, id)| id.clone())
            .collect();
        let mut unknown: Vec<String> = vec![
            "doesnotexist".to_string(),
            "00000000000000000000000000000000".to_string(),
            "a".to_string(),
            "Job.Name:with/odd=chars".to_string(),
        ];
        unknown.extend(foreign);

        for job_id in &unknown {
            let (status, body) = comprehend
                .call(family.describe, &json!({ "JobId": job_id }))
                .await;
            assert_eq!(
                status, 400,
                "case {}/{job_id:?}: expected 400, got {status}: {body}",
                family.describe
            );
            assert_eq!(
                error_code(&body),
                "JobNotFoundException",
                "case {}/{job_id:?}: expected JobNotFoundException, got {body}",
                family.describe
            );
        }
    }
}

#[tokio::test]
async fn a_job_status_only_moves_forward_and_ends_in_the_modelled_failure() {
    let comprehend = Comprehend::new();

    for family in FAMILIES {
        let body = submission("only the required members");
        let (job_id, _) = start(&comprehend, family, "lifecycle", &body).await;

        // The first read, taken straight after the call, still sees the transient
        // state: the queue span is deliberately non-zero.
        let first = describe(&comprehend, family, &job_id).await;
        assert_eq!(
            first["JobStatus"].as_str(),
            Some("SUBMITTED"),
            "case {}/\"first read\": expected SUBMITTED, got {first}",
            family.describe
        );
        assert!(
            first["EndTime"].is_null(),
            "case {}/\"first read\": a non-terminal job has no EndTime, got {first}",
            family.describe
        );

        let mut seen: Vec<String> = Vec::new();
        let mut highest = 0usize;
        let started = Instant::now();
        loop {
            let properties = describe(&comprehend, family, &job_id).await;
            let status = properties["JobStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let rank = LIFECYCLE
                .iter()
                .position(|known| *known == status)
                .unwrap_or_else(|| {
                    panic!(
                        "case {}/\"lifecycle\": unexpected status {status:?}",
                        family.describe
                    )
                });
            assert!(
                rank >= highest,
                "case {}/\"lifecycle\": status went backwards to {status:?} after {seen:?}",
                family.describe
            );
            highest = rank;
            if seen.last().map(String::as_str) != Some(status.as_str()) {
                seen.push(status.clone());
            }
            if status == TERMINAL_STATUS {
                assert!(
                    properties["EndTime"].as_f64().is_some_and(|t| t > 0.0),
                    "case {}/\"lifecycle\": a terminal job must carry an EndTime, got \
                     {properties}",
                    family.describe
                );
                // The reason the run could not be carried out, naming the location
                // the input documents were to be read from. No object storage
                // answers this suite's emulator, so the run's own listing is what
                // failed: the job reports that, rather than a COMPLETED run nothing
                // performed. `key_phrases_job_run` is the suite that mounts object
                // storage and asserts the COMPLETED run.
                let message = properties["Message"].as_str().unwrap_or_default();
                for expected in [
                    body["InputDataConfig"]["S3Uri"].as_str().unwrap_or_default(),
                    "Could not list the input documents",
                ] {
                    assert!(
                        message.contains(expected),
                        "case {}/\"lifecycle\": expected the failure Message to mention \
                         {expected:?}, got {message:?}",
                        family.describe
                    );
                }
                break;
            }
            assert!(
                properties["Message"].is_null(),
                "case {}/\"lifecycle\": a non-terminal job carries no Message, got \
                 {properties}",
                family.describe
            );
            assert!(
                started.elapsed() < SETTLE_TIMEOUT,
                "case {}/\"lifecycle\": the job never reached {TERMINAL_STATUS}; statuses \
                 seen: {seen:?}",
                family.describe
            );
            sleep(Duration::from_millis(40));
        }

        assert_eq!(
            seen.first().map(String::as_str),
            Some("SUBMITTED"),
            "case {}/\"lifecycle\": expected the first status to be SUBMITTED, saw {seen:?}",
            family.describe
        );
        assert_eq!(
            seen.last().map(String::as_str),
            Some(TERMINAL_STATUS),
            "case {}/\"lifecycle\": expected the last status to be {TERMINAL_STATUS}, saw \
             {seen:?}",
            family.describe
        );

        // A read never moves a settled job on: two reads of a terminal job agree.
        let once = describe(&comprehend, family, &job_id).await;
        let twice = describe(&comprehend, family, &job_id).await;
        assert_eq!(
            once, twice,
            "case {}/\"terminal reads\": reading a finished job changed it: {once} then \
             {twice}",
            family.describe
        );
    }
}
