// SPDX-License-Identifier: CC0-1.0

//! Comprehend's asynchronous sentiment-analysis job lifecycle:
//! `StartSentimentDetectionJob`, `DescribeSentimentDetectionJob`,
//! `ListSentimentDetectionJobs` and `StopSentimentDetectionJob`.
//!
//! Each suite asserts the rule over a SET of submissions: whatever a job was
//! started with is what Describe and List report back, the job's status only ever
//! moves forward along the modelled lifecycle, a page resumes exactly where the
//! previous one ended, and a stop is persisted.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background — if a suite in this file ever spawns concurrent work,
// switch to a timer rather than blocking the runtime thread.
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::JOB_STATUS_VALUES;
use service_com_amazonaws_comprehend_comprehend_20171127::state::{
    JOB_ALREADY_TERMINAL_MESSAGE, JOB_NOT_FOUND_MESSAGE, MAX_ANALYSIS_JOB_RECORDS,
    TERMINAL_JOB_STATUSES,
};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// The lifecycle order a job's status may only move forward along.
///
/// A started sentiment-detection job ends `FAILED` in THIS suite, whose emulator
/// mounts no object storage: running it means reading the documents from the Amazon
/// S3 location `InputDataConfig` names, scoring each one's sentiment and writing the
/// scored output back, and nothing answers those calls here, so the run reports what
/// it could not read. `key_phrases_job_run` is the suite that mounts object storage
/// and asserts a run that completes and the archive it wrote.
const LIFECYCLE: &[&str] = &["SUBMITTED", "IN_PROGRESS", "FAILED"];

/// The terminal status a started job reaches, as `JOB_STATUS_VALUES` spells it.
const TERMINAL_STATUS: &str = "FAILED";

/// Longest a suite waits for the clock to owe a job its terminal status. A job walks
/// two phase spans (`crate::state::LIFECYCLE_PHASE_SPAN` each) to a terminal status;
/// this leaves ample head-room on a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

/// A `StartSentimentDetectionJob` body that differs from the others along every
/// member the operation persists: the optional members present, the language, the
/// data locations and the tags.
fn submission(case: &str) -> Value {
    match case {
        "every optional member set" => json!({
            "JobName": "demo-job",
            "LanguageCode": "en",
            "InputDataConfig": {
                "S3Uri": "s3://bucket/in/",
                "InputFormat": "ONE_DOC_PER_FILE",
            },
            "OutputDataConfig": {
                "S3Uri": "s3://bucket/out/",
                "KmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/abcd-1234",
            },
            "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
            "VolumeKmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            "VpcConfig": {
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
                "Subnets": ["subnet-0123456789abcdef0", "subnet-0fedcba9876543210"],
            },
            "Tags": [{ "Key": "team", "Value": "research" }, { "Key": "cost-centre" }],
        }),
        "only the required members" => json!({
            "LanguageCode": "ja",
            "InputDataConfig": { "S3Uri": "s3://other-bucket/corpus/" },
            "OutputDataConfig": { "S3Uri": "s3://other-bucket/results/" },
            "DataAccessRoleArn": "arn:aws:iam::210987654321:role/path/to/OtherRole",
        }),
        _ => json!({
            "JobName": "zh-tw-batch.2026/04",
            "LanguageCode": "zh-TW",
            "InputDataConfig": {
                "S3Uri": "s3://third/in",
                "DocumentReaderConfig": { "DocumentReadAction": "TEXTRACT_DETECT_DOCUMENT_TEXT" },
            },
            "OutputDataConfig": { "S3Uri": "s3://third/out" },
            "DataAccessRoleArn": "arn:aws:iam::000000000000:role/Third",
        }),
    }
}

/// The cases [`submission`] builds.
const SUBMISSION_CASES: &[&str] = &[
    "every optional member set",
    "only the required members",
    "a document-reader input and a punctuated job name",
];

/// Start a job and return `(JobId, JobArn)`.
async fn start(comprehend: &Comprehend, case: &str, body: &Value) -> (String, String) {
    let output = comprehend.ok("StartSentimentDetectionJob", body).await;
    let job_id = output["JobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a JobId, got {output}"))
        .to_string();
    let job_arn = output["JobArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a JobArn, got {output}"))
        .to_string();
    assert_eq!(
        output["JobStatus"].as_str(),
        Some("SUBMITTED"),
        "case {case:?}: a freshly started job must be SUBMITTED, got {output}"
    );
    (job_id, job_arn)
}

/// The job properties `DescribeSentimentDetectionJob` reports for `job_id`.
async fn describe(comprehend: &Comprehend, job_id: &str) -> Value {
    let output = comprehend
        .ok(
            "DescribeSentimentDetectionJob",
            &json!({ "JobId": job_id }),
        )
        .await;
    output["SentimentDetectionJobProperties"].clone()
}

/// Poll `DescribeSentimentDetectionJob` until the job reports `status`, and
/// return the properties reported then.
async fn wait_for_status(comprehend: &Comprehend, job_id: &str, status: &str, case: &str) -> Value {
    let started = Instant::now();
    loop {
        let properties = describe(comprehend, job_id).await;
        if properties["JobStatus"].as_str() == Some(status) {
            return properties;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: the job never reached {status}, last read {properties}"
        );
        sleep(Duration::from_millis(40));
    }
}

/// Poll `DescribeSentimentDetectionJob` until the job reports one of the statuses
/// it can never leave, and return the properties reported then.
async fn wait_for_terminal(comprehend: &Comprehend, job_id: &str, case: &str) -> Value {
    let started = Instant::now();
    loop {
        let properties = describe(comprehend, job_id).await;
        if TERMINAL_JOB_STATUSES.contains(&properties["JobStatus"].as_str().unwrap_or_default()) {
            return properties;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: the job never reached a terminal status, last read {properties}"
        );
        sleep(Duration::from_millis(40));
    }
}

#[tokio::test]
async fn a_started_job_is_persisted_and_described_from_the_store() {
    let comprehend = Comprehend::new();

    for case in SUBMISSION_CASES {
        let body = submission(case);
        let (job_id, job_arn) = start(&comprehend, case, &body).await;

        // The minted id and ARN: 32 hex characters, and the modelled ARN form
        // `arn:aws:comprehend:<region>:<account>:sentiment-detection-job/<JobId>`
        // with the region the request was signed for.
        assert!(
            job_id.len() == 32 && job_id.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected a 32-character hex JobId, got {job_id:?}"
        );
        assert_eq!(
            job_arn,
            format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:sentiment-detection-job/{job_id}"),
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
            "InputDataConfig",
            "OutputDataConfig",
            "DataAccessRoleArn",
            "VolumeKmsKeyId",
            "VpcConfig",
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
        // `Tags` is not a member of the properties shape, so it must not appear.
        assert!(
            properties.get("Tags").is_none(),
            "case {case:?}: SentimentDetectionJobProperties models no Tags member, \
             got {properties}"
        );
    }
}

#[tokio::test]
async fn repeating_a_client_request_token_returns_the_first_job() {
    let comprehend = Comprehend::new();

    for case in SUBMISSION_CASES {
        let mut body = submission(case);
        body["ClientRequestToken"] = json!(format!("tok-{}", case.replace(' ', "-")));

        let (first_id, first_arn) = start(&comprehend, case, &body).await;
        let repeated = comprehend.ok("StartSentimentDetectionJob", &body).await;
        assert_eq!(
            repeated["JobId"].as_str(),
            Some(first_id.as_str()),
            "case {case:?}: a repeated ClientRequestToken must return the first JobId, \
             got {repeated}"
        );
        assert_eq!(
            repeated["JobArn"].as_str(),
            Some(first_arn.as_str()),
            "case {case:?}: a repeated ClientRequestToken must return the first JobArn, \
             got {repeated}"
        );
    }

    // One job per distinct token, not two per submission.
    let listed = comprehend
        .ok("ListSentimentDetectionJobs", &json!({ "MaxResults": 500 }))
        .await;
    let count = listed["SentimentDetectionJobPropertiesList"]
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        count,
        SUBMISSION_CASES.len(),
        "case \"one job per token\": expected {} jobs, got {count} in {listed}",
        SUBMISSION_CASES.len()
    );
}

#[tokio::test]
async fn a_submission_carrying_more_tags_than_a_resource_can_hold_is_refused() {
    let comprehend = Comprehend::new();

    // The bound the `TooManyTagsException` documentation states ("50 tags per
    // resource"): at it, and one past it.
    for count in [0usize, 1, 50, 51] {
        let mut body = submission("only the required members");
        body["Tags"] = Value::Array(
            (0..count)
                .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
                .collect(),
        );
        let (status, response) = comprehend
            .call("StartSentimentDetectionJob", &body)
            .await;

        if count > 50 {
            assert_eq!(
                status, 400,
                "case {count} tags: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "TooManyTagsException",
                "case {count} tags: expected TooManyTagsException, got {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {count} tags: expected 200, got {status}: {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_submission_without_a_token_creates_a_new_job_every_time() {
    let comprehend = Comprehend::new();
    let body = submission("only the required members");

    let (first, _) = start(&comprehend, "first", &body).await;
    let (second, _) = start(&comprehend, "second", &body).await;
    assert_ne!(
        first, second,
        "case \"no ClientRequestToken\": each submission must mint its own JobId"
    );
}

#[tokio::test]
async fn describe_and_stop_refuse_an_unknown_job_id() {
    let comprehend = Comprehend::new();
    // One real job, so the refusal is about the id and not about an empty store.
    start(
        &comprehend,
        "seed",
        &submission("only the required members"),
    )
    .await;

    for operation in ["DescribeSentimentDetectionJob", "StopSentimentDetectionJob"] {
        for job_id in [
            "doesnotexist",
            "00000000000000000000000000000000",
            "a",
            "Job.Name:with/odd=chars",
        ] {
            let (status, body) = comprehend
                .call(operation, &json!({ "JobId": job_id }))
                .await;
            assert_eq!(
                status, 400,
                "case {operation} with {job_id:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "JobNotFoundException",
                "case {operation} with {job_id:?}: expected JobNotFoundException, got {body}"
            );
            // The refusal states that no such job exists and nothing else: the
            // sentence is the same for every operation and every id, and echoes
            // none of them.
            assert_eq!(
                error_message(&body),
                JOB_NOT_FOUND_MESSAGE,
                "case {operation} with {job_id:?}: expected the fixed not-found \
                 sentence, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_job_status_only_moves_forward_and_ends_in_the_modelled_failure() {
    let comprehend = Comprehend::new();
    let (job_id, _) = start(
        &comprehend,
        "lifecycle",
        &submission("only the required members"),
    )
    .await;

    // The first read, taken straight after the call, still sees the transient state.
    let first = describe(&comprehend, &job_id).await;
    assert_eq!(
        first["JobStatus"].as_str(),
        Some("SUBMITTED"),
        "case \"first read\": expected SUBMITTED, got {first}"
    );
    assert!(
        first["EndTime"].is_null(),
        "case \"first read\": a non-terminal job has no EndTime, got {first}"
    );

    let mut seen: Vec<String> = Vec::new();
    let mut highest = 0usize;
    let started = Instant::now();
    loop {
        let properties = describe(&comprehend, &job_id).await;
        let status = properties["JobStatus"].as_str().unwrap_or_default().to_string();
        let rank = LIFECYCLE
            .iter()
            .position(|known| *known == status)
            .unwrap_or_else(|| panic!("case \"lifecycle\": unexpected status {status:?}"));
        assert!(
            rank >= highest,
            "case \"lifecycle\": status went backwards to {status:?} after {seen:?}"
        );
        highest = rank;
        if seen.last().map(String::as_str) != Some(status.as_str()) {
            seen.push(status.clone());
        }
        if status == TERMINAL_STATUS {
            assert!(
                properties["EndTime"].as_f64().is_some_and(|t| t > 0.0),
                "case \"lifecycle\": a terminal job must carry an EndTime, got {properties}"
            );
            // The reason the run could not be carried out, naming the location the
            // input documents were to be read from. No object storage answers this
            // suite's emulator, so the run's own listing is what failed: the job
            // reports that, rather than a COMPLETED run nothing performed.
            let message = properties["Message"].as_str().unwrap_or_default();
            for expected in [
                "s3://other-bucket/corpus/",
                "Could not list the input documents",
            ] {
                assert!(
                    message.contains(expected),
                    "case \"lifecycle\": expected the failure Message to mention \
                     {expected:?}, got {message:?}"
                );
            }
            break;
        }
        assert!(
            properties["Message"].is_null(),
            "case \"lifecycle\": a non-terminal job carries no Message, got {properties}"
        );
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case \"lifecycle\": the job never reached {TERMINAL_STATUS}; statuses seen: \
             {seen:?}"
        );
        sleep(Duration::from_millis(40));
    }

    // The sequence of distinct statuses observed must be a prefix-ordered
    // SUBSEQUENCE of the lifecycle: every status seen is a lifecycle status, it
    // never goes backwards, it starts at SUBMITTED and it ends at the terminal
    // status. Whether the poller catches IN_PROGRESS is a scheduling matter, not a
    // behavioural one, so it is not required.
    assert_eq!(
        seen.first().map(String::as_str),
        Some("SUBMITTED"),
        "case \"lifecycle\": expected the first status to be SUBMITTED, saw {seen:?}"
    );
    assert_eq!(
        seen.last().map(String::as_str),
        Some(TERMINAL_STATUS),
        "case \"lifecycle\": expected the last status to be {TERMINAL_STATUS}, saw {seen:?}"
    );
    assert!(
        seen.iter().all(|status| LIFECYCLE.contains(&status.as_str())),
        "case \"lifecycle\": saw a status outside the lifecycle in {seen:?}"
    );

    // A read never moves a settled job on: two reads of a terminal job agree.
    let once = describe(&comprehend, &job_id).await;
    let twice = describe(&comprehend, &job_id).await;
    assert_eq!(
        once, twice,
        "case \"terminal reads\": reading a completed job changed it: {once} then {twice}"
    );
}

#[tokio::test]
async fn the_record_store_is_bounded_and_evicts_oldest_first() {
    let comprehend = Comprehend::new();
    let overflow = 5usize;

    // Submit past the bound. Each submission carries its own ClientRequestToken, so
    // the idempotency index grows with the store and must be trimmed with it.
    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_ANALYSIS_JOB_RECORDS + overflow {
        let mut body = submission("only the required members");
        body["ClientRequestToken"] = json!(format!("bound-{index}"));
        let output = comprehend.ok("StartSentimentDetectionJob", &body).await;
        ids.push(output["JobId"].as_str().unwrap_or_default().to_string());
    }

    // The store never passes the bound, and a submission is never refused for it.
    let mut listed = 0usize;
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "MaxResults": 500 });
        if let Some(token) = &next_token {
            request["NextToken"] = json!(token);
        }
        let output = comprehend.ok("ListSentimentDetectionJobs", &request).await;
        listed += output["SentimentDetectionJobPropertiesList"]
            .as_array()
            .map_or(0, Vec::len);
        match output["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    assert_eq!(
        listed, MAX_ANALYSIS_JOB_RECORDS,
        "case \"past the bound\": expected the store to hold {MAX_ANALYSIS_JOB_RECORDS} \
         jobs, got {listed}"
    );

    // Oldest-first: the first `overflow` submissions are gone, the rest are kept.
    for (index, job_id) in ids.iter().enumerate().take(overflow) {
        let (status, body) = comprehend
            .call("DescribeSentimentDetectionJob", &json!({ "JobId": job_id }))
            .await;
        assert_eq!(
            status, 400,
            "case \"evicted job {index}\": expected 400, got {status}: {body}"
        );
        // Its idempotency-index entry went with it, so the token mints a new job.
        let mut body = submission("only the required members");
        body["ClientRequestToken"] = json!(format!("bound-{index}"));
        let reused = comprehend.ok("StartSentimentDetectionJob", &body).await;
        assert_ne!(
            reused["JobId"].as_str(),
            Some(job_id.as_str()),
            "case \"evicted job {index}\": the index still points at an evicted job"
        );
    }
    let newest = ids.last().expect("at least one job was submitted");
    let (status, body) = comprehend
        .call("DescribeSentimentDetectionJob", &json!({ "JobId": newest }))
        .await;
    assert_eq!(
        status, 200,
        "case \"newest job\": the most recent submission must be retained, got \
         {status}: {body}"
    );
}

#[tokio::test]
async fn list_pages_through_every_job_in_submission_order() {
    let comprehend = Comprehend::new();

    let mut expected: Vec<String> = Vec::new();
    for (index, case) in SUBMISSION_CASES.iter().enumerate() {
        let mut body = submission(case);
        body["JobName"] = json!(format!("page-job-{index}"));
        let (job_id, _) = start(&comprehend, case, &body).await;
        expected.push(job_id);
    }

    for page_size in [1usize, 2, SUBMISSION_CASES.len(), 500] {
        let mut collected: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let output = comprehend
                .ok("ListSentimentDetectionJobs", &request)
                .await;
            let page = output["SentimentDetectionJobPropertiesList"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case MaxResults={page_size}: a page returned {} items, got {output}",
                page.len()
            );
            for item in &page {
                collected.push(item["JobId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case MaxResults={page_size}: paging did not terminate after {pages} pages"
            );
            match output["NextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }

        assert_eq!(
            collected, expected,
            "case MaxResults={page_size}: expected every job once, oldest to newest"
        );
    }
}

/// `SubmitTimeAfter`'s own documentation states the opposite order to every other
/// member's — "Jobs are returned in descending order, newest to oldest" against
/// `SubmitTimeBefore`'s "ascending order, oldest to newest" — and only one filter
/// may be set at a time, so each page is served in the order its filter names.
#[tokio::test]
async fn list_serves_a_submit_time_after_page_newest_first() {
    let comprehend = Comprehend::new();

    let mut oldest_first: Vec<String> = Vec::new();
    for (index, case) in SUBMISSION_CASES.iter().enumerate() {
        let mut body = submission(case);
        body["JobName"] = json!(format!("order-job-{index}"));
        let (job_id, _) = start(&comprehend, case, &body).await;
        oldest_first.push(job_id);
    }
    let newest_first: Vec<String> = oldest_first.iter().rev().cloned().collect();

    for (case, filter, expected) in [
        (
            "SubmitTimeAfter",
            json!({ "SubmitTimeAfter": 0.0 }),
            &newest_first,
        ),
        (
            "SubmitTimeBefore",
            json!({ "SubmitTimeBefore": 4_102_444_800_f64 }),
            &oldest_first,
        ),
    ] {
        // Whole page, then one job at a time through the cursor: the order and the
        // membership must be the same either way.
        for page_size in [1usize, SUBMISSION_CASES.len()] {
            let mut collected: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut request = json!({ "MaxResults": page_size, "Filter": filter });
                if let Some(token) = &next_token {
                    request["NextToken"] = json!(token);
                }
                let output = comprehend.ok("ListSentimentDetectionJobs", &request).await;
                for item in output["SentimentDetectionJobPropertiesList"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                {
                    collected.push(item["JobId"].as_str().unwrap_or_default().to_string());
                }
                pages += 1;
                assert!(
                    pages <= oldest_first.len() + 1,
                    "case {case} with MaxResults={page_size}: paging did not terminate"
                );
                match output["NextToken"].as_str() {
                    Some(token) => next_token = Some(token.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                &collected, expected,
                "case {case} with MaxResults={page_size}: wrong order or membership"
            );
        }
    }
}

#[tokio::test]
async fn list_honours_each_filter_and_refuses_more_than_one() {
    let comprehend = Comprehend::new();

    let mut ids: Vec<String> = Vec::new();
    for (index, case) in SUBMISSION_CASES.iter().enumerate() {
        let mut body = submission(case);
        body["JobName"] = json!(format!("filter-job-{index}"));
        let (job_id, _) = start(&comprehend, case, &body).await;
        ids.push(job_id);
    }

    // JobName selects exactly the job that carries it; a name no job carries
    // selects nothing and offers no NextToken.
    for (index, job_id) in ids.iter().enumerate() {
        let output = comprehend
            .ok(
                "ListSentimentDetectionJobs",
                &json!({ "Filter": { "JobName": format!("filter-job-{index}") } }),
            )
            .await;
        let page = output["SentimentDetectionJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            page.len(),
            1,
            "case Filter.JobName=filter-job-{index}: expected exactly one job, got {output}"
        );
        assert_eq!(
            page[0]["JobId"].as_str(),
            Some(job_id.as_str()),
            "case Filter.JobName=filter-job-{index}: wrong job, got {output}"
        );
    }

    for absent in ["nope", "filter-job"] {
        let output = comprehend
            .ok(
                "ListSentimentDetectionJobs",
                &json!({ "Filter": { "JobName": absent } }),
            )
            .await;
        let page = output["SentimentDetectionJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            page.is_empty() && output["NextToken"].is_null(),
            "case Filter.JobName={absent:?}: expected an empty last page, got {output}"
        );
    }

    // JobStatus selects on the status the clock has already made due, so every
    // modelled status either selects jobs that carry it or selects none.
    for status in JOB_STATUS_VALUES {
        let output = comprehend
            .ok(
                "ListSentimentDetectionJobs",
                &json!({ "Filter": { "JobStatus": status } }),
            )
            .await;
        let page = output["SentimentDetectionJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for item in &page {
            assert_eq!(
                item["JobStatus"].as_str(),
                Some(*status),
                "case Filter.JobStatus={status:?}: returned a job with another status: {item}"
            );
        }
    }

    // And the positive case, made fail-able: once the clock owes every job its
    // terminal status, that status must select all of them and no other status any.
    let started = Instant::now();
    loop {
        let output = comprehend
            .ok(
                "ListSentimentDetectionJobs",
                &json!({ "Filter": { "JobStatus": TERMINAL_STATUS } }),
            )
            .await;
        let page = output["SentimentDetectionJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if page.len() == ids.len() {
            break;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case Filter.JobStatus={TERMINAL_STATUS:?}: expected {} jobs once the clock \
             owed them their terminal status, got {output}",
            ids.len()
        );
        sleep(Duration::from_millis(40));
    }
    for status in ["SUBMITTED", "IN_PROGRESS", "COMPLETED", "STOPPED"] {
        let output = comprehend
            .ok(
                "ListSentimentDetectionJobs",
                &json!({ "Filter": { "JobStatus": status } }),
            )
            .await;
        let page = output["SentimentDetectionJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            page.is_empty(),
            "case Filter.JobStatus={status:?}: every job is {TERMINAL_STATUS}, so this \
             must select none, got {output}"
        );
    }

    // Submit-time bounds: every job was submitted before now and after the epoch.
    let far_future = 4_102_444_800_f64; // 2100-01-01T00:00:00Z
    for (case, filter, expect_all) in [
        ("SubmitTimeBefore in the far future", json!({ "SubmitTimeBefore": far_future }), true),
        ("SubmitTimeAfter in the far future", json!({ "SubmitTimeAfter": far_future }), false),
        ("SubmitTimeAfter at the epoch", json!({ "SubmitTimeAfter": 0.0 }), true),
        ("SubmitTimeBefore at the epoch", json!({ "SubmitTimeBefore": 0.0 }), false),
    ] {
        let output = comprehend
            .ok(
                "ListSentimentDetectionJobs",
                &json!({ "Filter": filter }),
            )
            .await;
        let page = output["SentimentDetectionJobPropertiesList"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let expected = if expect_all { ids.len() } else { 0 };
        assert_eq!(
            page.len(),
            expected,
            "case {case:?}: expected {expected} jobs, got {output}"
        );
    }

    // "You can only set one filter at a time."
    let (status, body) = comprehend
        .call(
            "ListSentimentDetectionJobs",
            &json!({ "Filter": { "JobName": "filter-job-0", "JobStatus": "SUBMITTED" } }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"two filter members\": expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidFilterException",
        "case \"two filter members\": expected InvalidFilterException, got {body}"
    );
}

#[tokio::test]
async fn list_refuses_a_token_it_did_not_issue() {
    let comprehend = Comprehend::new();
    start(
        &comprehend,
        "seed",
        &submission("only the required members"),
    )
    .await;

    for token in [
        "not-a-token",
        "00",
        "deadbeef",
        "0011223344556677889900",
        // Well-formed, but names no page this service ever issued: a cursor is a
        // submission sequence number, which starts at 1 and never runs ahead of
        // the newest submission.
        "0000000000000000",
        "00000000000003e8",
    ] {
        let (status, body) = comprehend
            .call(
                "ListSentimentDetectionJobs",
                &json!({ "NextToken": token }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case NextToken={token:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case NextToken={token:?}: expected InvalidRequestException, got {body}"
        );
    }
}

/// Cancelling an analysis job is two-phase: a job that has not reached a status
/// it can never leave is MARKED for termination, so the stop answers
/// `STOP_REQUESTED` — never a terminal status — and the clock then owes the job
/// `STOPPED`. Asserted over every submission, so it holds whatever the job was
/// started with.
#[tokio::test]
async fn a_stop_on_a_non_terminal_job_answers_stop_requested_then_settles_to_stopped() {
    let comprehend = Comprehend::new();
    let mut stopped_rows = 0usize;

    for case in SUBMISSION_CASES {
        let (job_id, _) = start(&comprehend, case, &submission(case)).await;

        let (http_status, stopped) = comprehend
            .call("StopSentimentDetectionJob", &json!({ "JobId": job_id }))
            .await;
        if http_status == 400 {
            // The clock owed this job its terminal status before the stop reached
            // it, which is a refusal. That is the other suite's case, so this row
            // has nothing left to assert.
            let properties = describe(&comprehend, &job_id).await;
            assert_eq!(
                properties["JobStatus"].as_str(),
                Some(TERMINAL_STATUS),
                "case {case:?}: a stop was refused for a job that is not terminal: \
                 {stopped} with {properties}"
            );
            continue;
        }
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected 200, got {http_status}: {stopped}"
        );
        assert_eq!(
            stopped["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: Stop reported a different JobId: {stopped}"
        );
        assert_eq!(
            stopped["JobStatus"].as_str(),
            Some("STOP_REQUESTED"),
            "case {case:?}: a stop on a non-terminal job must answer STOP_REQUESTED, \
             got {stopped}"
        );
        stopped_rows += 1;

        // The marked state is the STORED one, so Describe observes it, and the
        // stop explains itself in the modelled Message. The stopping span is
        // non-zero but not infinite, so a read taken straight after the call sees
        // either the marked state or the `STOPPED` it settles into.
        let properties = describe(&comprehend, &job_id).await;
        let observed = properties["JobStatus"].as_str().unwrap_or_default();
        assert!(
            observed == "STOP_REQUESTED" || observed == "STOPPED",
            "case {case:?}: the stop was not persisted, Describe says {properties}"
        );
        assert!(
            properties["Message"]
                .as_str()
                .is_some_and(|message| message.contains("request of the caller")),
            "case {case:?}: expected the stop to name its reason in Message, got \
             {properties}"
        );

        // And the clock settles it into STOPPED, which is where it stays: a
        // cancelled job never re-enters the lifecycle, and reading it again
        // changes nothing.
        let settled = wait_for_status(&comprehend, &job_id, "STOPPED", case).await;
        assert!(
            settled["EndTime"].as_f64().is_some_and(|end| end > 0.0),
            "case {case:?}: a stopped job must carry an EndTime, got {settled}"
        );
        let again = describe(&comprehend, &job_id).await;
        assert_eq!(
            settled, again,
            "case {case:?}: reading a stopped job changed it: {settled} then {again}"
        );
    }

    // The 400 branch above is a legitimate outcome for a row, but if EVERY row took
    // it the suite would assert nothing about a stop that lands.
    assert!(
        stopped_rows > 0,
        "case \"stop a running job\": no row reached the stop path; every submission \
         had already finished before its stop arrived"
    );
}

/// A stop on a job that already holds a status it can never leave has nothing to
/// mark for termination: it is refused with 400 `InvalidRequestException` and the
/// stored record is left exactly as it was.
///
/// The rows are the terminal statuses a sentiment-detection job can be driven
/// into: `FAILED`, which the clock owes a started job, and `STOPPED`, which the
/// clock owes a cancelled one. `COMPLETED` is in the modelled vocabulary but no
/// job reaches it here (nothing performs the batch run), so it cannot be driven.
#[tokio::test]
async fn a_stop_on_a_terminal_job_is_refused_and_changes_nothing() {
    let comprehend = Comprehend::new();

    for row in TERMINAL_JOB_STATUSES {
        if *row == "COMPLETED" {
            continue;
        }
        let (job_id, _) = start(
            &comprehend,
            row,
            &submission("only the required members"),
        )
        .await;

        let mut expected = TERMINAL_STATUS;
        if *row == "STOPPED" {
            let (http_status, stopped) = comprehend
                .call("StopSentimentDetectionJob", &json!({ "JobId": job_id }))
                .await;
            // A 200 means the stop landed, so this job's terminal status will be
            // STOPPED; a 400 means the clock had already finished it, which is the
            // FAILED row over again and is asserted identically below.
            if http_status == 200 {
                expected = "STOPPED";
                assert_eq!(
                    stopped["JobStatus"].as_str(),
                    Some("STOP_REQUESTED"),
                    "case {row}: the accepted stop must answer STOP_REQUESTED, got \
                     {stopped}"
                );
            }
        }

        let before = wait_for_terminal(&comprehend, &job_id, row).await;
        assert_eq!(
            before["JobStatus"].as_str(),
            Some(expected),
            "case {row}: expected the job to settle into {expected}, got {before}"
        );

        // Twice, so the refusal is not a one-off and neither call moves the job.
        for attempt in 1..=2 {
            let (status, body) = comprehend
                .call("StopSentimentDetectionJob", &json!({ "JobId": job_id }))
                .await;
            assert_eq!(
                status, 400,
                "case {row} attempt {attempt}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {row} attempt {attempt}: expected InvalidRequestException, got \
                 {body}"
            );
            assert_eq!(
                error_message(&body),
                JOB_ALREADY_TERMINAL_MESSAGE,
                "case {row} attempt {attempt}: unexpected refusal message in {body}"
            );
            let after = describe(&comprehend, &job_id).await;
            assert_eq!(
                after, before,
                "case {row} attempt {attempt}: a refused stop must leave the job \
                 unchanged: {before} became {after}"
            );
        }
    }
}
