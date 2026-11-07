// SPDX-License-Identifier: CC0-1.0

//! Comprehend's asynchronous dominant-language detection job:
//! `StartDominantLanguageDetectionJob`, `DescribeDominantLanguageDetectionJob` and
//! `StopDominantLanguageDetectionJob`.
//!
//! The job is not a record-keeping exercise: starting it READS the input documents
//! from the Amazon S3 location `InputDataConfig` names, identifies each document's
//! dominant language with the same in-process engine `DetectDominantLanguage` uses,
//! and writes the `output.tar.gz` archive `OutputDataConfig$S3Uri` documents back to
//! S3. Each suite below asserts that over a SET of submissions rather than one
//! recorded request: the rows differ along the input format, the number of input
//! files, the languages in them, the prefix forms, and the optional members the
//! submission carries.
//!
//! The emulator mounted here therefore holds the S3 service beside Comprehend on one
//! engine (`Comprehend::with_object_storage`), exactly as the assembled emulator
//! does. The one suite that mounts Comprehend ALONE asserts the other half of the
//! rule: a run this emulator cannot carry out ends in the modelled `FAILED` state
//! with a `Message` naming what stopped it, never in a `COMPLETED` that nothing
//! performed.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background.
use std::io::Read;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::JOB_STATUS_VALUES;

use service_com_amazonaws_comprehend_comprehend_20171127::state::{
    JOB_ALREADY_TERMINAL_MESSAGE, JOB_NOT_FOUND_MESSAGE, TERMINAL_JOB_STATUSES,
};

use crate::comprehend_client::{
    create_bucket_payload, error_code, error_message, Comprehend, ACCOUNT_ID, REGION,
};
use crate::sync_inference::DOCUMENTS;

/// The bucket every suite writes its input documents to.
const BUCKET: &str = "comprehend-batch";

/// The role ARN a submission names; the modelled `IamRoleArn` form.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/ComprehendRole";

/// Longest a suite waits for the clock to owe a job its terminal status. A job walks
/// two phase spans (`crate::state::LIFECYCLE_PHASE_SPAN` each) to a terminal status;
/// this leaves ample head-room on a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

/// The analysis-type token AWS names an analysis job's output directory with, for
/// this family (`<account-id>-LANGUAGE-<JobId>`).
const OUTPUT_ANALYSIS_TYPE: &str = "LANGUAGE";

/// The sample document written in `code`, from the catalog the synchronous
/// identification suite drives `DetectDominantLanguage` with — the SAME documents, so
/// the batch run and the synchronous call are asserted against one language set
/// (`ops::types::LANGUAGE_CODE_VALUES`, the model's own).
fn document(code: &str) -> &'static str {
    DOCUMENTS
        .iter()
        .find(|(_, candidate, _)| *candidate == code)
        .map(|(_, _, text)| *text)
        .unwrap_or_else(|| panic!("the document catalog has no sample for {code:?}"))
}

/// One end-to-end run: where its documents live, how they are split into documents,
/// and the language each document is written in.
struct RunCase {
    case: &'static str,
    /// `InputDataConfig$InputFormat`, or `None` to leave it to the service.
    input_format: Option<&'static str>,
    /// The key prefix the input documents are written under, with and without a
    /// trailing delimiter across the rows.
    input_prefix: &'static str,
    /// The key prefix the output archive is written under; empty for a bucket-only
    /// output URI.
    output_prefix: &'static str,
    /// `(key under the input prefix, the `LanguageCode` each of its lines is written
    /// in)`.
    files: &'static [(&'static str, &'static [&'static str])],
}

/// The runs the suites below drive. The rows differ along what the rule generalises
/// over: the input format (absent, each value the model's `InputFormat` enum names),
/// the number of input files, the number of documents in a file, the languages, and
/// the prefix forms.
const RUN_CASES: &[RunCase] = &[
    RunCase {
        case: "one file, format left to the service",
        input_format: None,
        input_prefix: "in/",
        output_prefix: "out/",
        files: &[("english.txt", &["en"])],
    },
    RunCase {
        case: "three files, one document each, ONE_DOC_PER_FILE",
        input_format: Some("ONE_DOC_PER_FILE"),
        input_prefix: "corpus/2026/",
        output_prefix: "results",
        files: &[
            ("french.txt", &["fr"]),
            ("japanese.txt", &["ja"]),
            ("nested/german.txt", &["de"]),
        ],
    },
    RunCase {
        case: "one file of four documents, ONE_DOC_PER_LINE",
        input_format: Some("ONE_DOC_PER_LINE"),
        input_prefix: "lines",
        output_prefix: "",
        files: &[("mixed.txt", &["en", "fr", "ja", "it"])],
    },
    RunCase {
        case: "two files of two documents each, ONE_DOC_PER_LINE, output at the bucket root",
        input_format: Some("ONE_DOC_PER_LINE"),
        input_prefix: "pairs/",
        output_prefix: "/",
        files: &[("one.txt", &["es", "pt"]), ("two.txt", &["ko", "ar"])],
    },
];

impl RunCase {
    /// The object key the file `name` under this run's input prefix is written to.
    fn input_key(&self, name: &str) -> String {
        match self.input_prefix.strip_suffix('/') {
            Some(prefix) => format!("{prefix}/{name}"),
            None => format!("{}/{name}", self.input_prefix),
        }
    }

    /// The `StartDominantLanguageDetectionJob` body for this run.
    fn submission(&self) -> Value {
        let mut input_data_config = json!({ "S3Uri": format!("s3://{BUCKET}/{}", self.input_prefix) });
        if let Some(input_format) = self.input_format {
            input_data_config["InputFormat"] = json!(input_format);
        }
        json!({
            "JobName": format!("run-{}", self.case.len()),
            "InputDataConfig": input_data_config,
            "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/{}", self.output_prefix) },
            "DataAccessRoleArn": ROLE_ARN,
        })
    }

    /// The output archive's key for the job `job_id`, as `OutputDataConfig$S3Uri`
    /// documents the layout: the submitted URI is the prefix, the service creates a
    /// directory specific to the job, and the file in it is called `output.tar.gz`.
    fn output_key(&self, job_id: &str) -> String {
        let directory = format!("{ACCOUNT_ID}-{OUTPUT_ANALYSIS_TYPE}-{job_id}/output/output.tar.gz");
        match self.output_prefix.trim_end_matches('/') {
            "" => directory,
            prefix => format!("{prefix}/{directory}"),
        }
    }

    /// The output entries this run owes, in the order the archive holds them:
    /// `(File, Line, LanguageCode)`.
    fn expected_entries(&self) -> Vec<(String, Option<usize>, &'static str)> {
        let per_line = self.input_format == Some("ONE_DOC_PER_LINE");
        let mut expected = Vec::new();
        for (name, codes) in self.files {
            if per_line {
                for (line, code) in codes.iter().enumerate() {
                    expected.push(((*name).to_string(), Some(line), *code));
                }
            } else {
                // Every line of a per-file row is written in one language, so the
                // whole file is a document in that language.
                expected.push(((*name).to_string(), None, codes[0]));
            }
        }
        expected
    }

    /// The bytes of the file `name`: its lines, each a sample document.
    fn file_body(&self, codes: &[&str]) -> String {
        codes
            .iter()
            .map(|code| document(code))
            .collect::<Vec<&str>>()
            .join("\n")
    }
}

/// Create `BUCKET`, so the job's reads and writes have somewhere to go.
async fn create_bucket(comprehend: &Comprehend) {
    let (status, body) = comprehend
        .s3("PUT", &format!("/{BUCKET}"), create_bucket_payload().as_bytes())
        .await;
    assert_eq!(
        status,
        200,
        "CreateBucket {BUCKET}: expected 200, got {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// Write one object.
async fn put_object(comprehend: &Comprehend, key: &str, body: &[u8]) {
    let (status, response) = comprehend
        .s3("PUT", &format!("/{BUCKET}/{key}"), body)
        .await;
    assert_eq!(
        status,
        200,
        "PutObject {key}: expected 200, got {status}: {:?}",
        String::from_utf8_lossy(&response)
    );
}

/// Read one object back.
async fn get_object(comprehend: &Comprehend, key: &str) -> (u16, Vec<u8>) {
    comprehend.s3("GET", &format!("/{BUCKET}/{key}"), b"").await
}

/// Start a job and return `(JobId, JobArn)`, asserting the submission was accepted.
async fn start(comprehend: &Comprehend, case: &str, body: &Value) -> (String, String) {
    let output = comprehend
        .ok("StartDominantLanguageDetectionJob", body)
        .await;
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
    assert_eq!(
        job_arn,
        format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:dominant-language-detection-job/{job_id}"),
        "case {case:?}: the JobArn must carry the request's region and the job's id, got {output}"
    );
    (job_id, job_arn)
}

/// The properties `DescribeDominantLanguageDetectionJob` reports for `job_id`.
async fn describe(comprehend: &Comprehend, job_id: &str) -> Value {
    let output = comprehend
        .ok(
            "DescribeDominantLanguageDetectionJob",
            &json!({ "JobId": job_id }),
        )
        .await;
    output["DominantLanguageDetectionJobProperties"].clone()
}

/// Poll `job_id` until the clock owes it a terminal status, and return its
/// properties then.
async fn settled(comprehend: &Comprehend, case: &str, job_id: &str) -> Value {
    let started = Instant::now();
    loop {
        let properties = describe(comprehend, job_id).await;
        let status = properties["JobStatus"].as_str().unwrap_or_default();
        assert!(
            JOB_STATUS_VALUES.contains(&status),
            "case {case:?}: {status:?} is not a modelled JobStatus, got {properties}"
        );
        if status == "COMPLETED" || status == "FAILED" {
            return properties;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: the job never reached a terminal status, last {properties}"
        );
        sleep(Duration::from_millis(40));
    }
}

/// The JSON documents the `output.tar.gz` at `archive` holds: it is gunzipped, the
/// single `output` member is read, and each of its lines is parsed.
fn archive_entries(case: &str, archive: &[u8]) -> Vec<Value> {
    let decoder = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);
    let mut body = String::new();
    let mut names: Vec<String> = Vec::new();
    for entry in tar
        .entries()
        .unwrap_or_else(|error| panic!("case {case:?}: the output archive is not a tar: {error}"))
    {
        let mut entry = entry
            .unwrap_or_else(|error| panic!("case {case:?}: unreadable archive entry: {error}"));
        let name = entry
            .path()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_default();
        names.push(name);
        entry
            .read_to_string(&mut body)
            .unwrap_or_else(|error| panic!("case {case:?}: unreadable archive member: {error}"));
    }
    assert_eq!(
        names,
        vec!["output".to_string()],
        "case {case:?}: expected the archive to hold one `output` member, got {names:?}"
    );
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|error| {
                panic!("case {case:?}: output line {line:?} is not JSON: {error}")
            })
        })
        .collect()
}

#[tokio::test]
async fn a_job_analyses_its_input_documents_and_writes_the_output_archive() {
    for run in RUN_CASES {
        let case = run.case;
        let comprehend = Comprehend::with_object_storage();
        create_bucket(&comprehend).await;
        for (name, codes) in run.files {
            put_object(
                &comprehend,
                &run.input_key(name),
                run.file_body(codes).as_bytes(),
            )
            .await;
        }

        let submission = run.submission();
        let (job_id, _) = start(&comprehend, case, &submission).await;

        // Before the clock owes the terminal status, the job reports the output
        // configuration it was SUBMITTED with: the archive's own location is
        // revealed together with the status that matches it, never before.
        let pending = describe(&comprehend, &job_id).await;
        if pending["JobStatus"].as_str() != Some("COMPLETED") {
            assert_eq!(
                pending["OutputDataConfig"], submission["OutputDataConfig"],
                "case {case:?}: a job that is still {:?} must report the submitted output \
                 configuration, got {pending}",
                pending["JobStatus"]
            );
        }

        let properties = settled(&comprehend, case, &job_id).await;

        assert_eq!(
            properties["JobStatus"].as_str(),
            Some("COMPLETED"),
            "case {case:?}: the run read every document, so the job completed, got {properties}"
        );
        assert!(
            properties["Message"].is_null(),
            "case {case:?}: a completed run has nothing to report in Message, got {properties}"
        );
        assert!(
            properties["EndTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: a terminal job carries an EndTime, got {properties}"
        );
        // What was submitted is what is reported back — except the output URI, which
        // `OutputDataConfig$S3Uri` documents a finished job reports as "the location
        // of the output file, called output.tar.gz".
        assert_eq!(
            properties["InputDataConfig"], submission["InputDataConfig"],
            "case {case:?}: Describe must report the submitted InputDataConfig, got {properties}"
        );
        assert_eq!(
            properties["JobName"], submission["JobName"],
            "case {case:?}: Describe must report the submitted JobName, got {properties}"
        );
        assert_eq!(
            properties["DataAccessRoleArn"].as_str(),
            Some(ROLE_ARN),
            "case {case:?}: Describe must report the submitted role, got {properties}"
        );
        let expected_uri = format!("s3://{BUCKET}/{}", run.output_key(&job_id));
        assert_eq!(
            properties["OutputDataConfig"]["S3Uri"].as_str(),
            Some(expected_uri.as_str()),
            "case {case:?}: expected the output archive's own location, got {properties}"
        );

        // The archive is really there, and it holds one entry per analysed document,
        // naming the file it came from and the language the engine identified.
        let (status, archive) = get_object(&comprehend, &run.output_key(&job_id)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reported output archive must be readable, got {status}"
        );
        let entries = archive_entries(case, &archive);
        let expected = run.expected_entries();
        assert_eq!(
            entries.len(),
            expected.len(),
            "case {case:?}: expected {} output entries, got {entries:?}",
            expected.len()
        );
        for (entry, (file, line, code)) in entries.iter().zip(&expected) {
            assert_eq!(
                entry["File"].as_str(),
                Some(file.as_str()),
                "case {case:?}: expected the entry to name {file:?}, got {entry}"
            );
            assert_eq!(
                entry["Line"].as_u64().map(|line| line as usize),
                *line,
                "case {case:?}: wrong Line for {file:?}, got {entry}"
            );
            let languages = entry["Languages"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected Languages, got {entry}"))
                .clone();
            assert_eq!(
                languages.first().and_then(|top| top["LanguageCode"].as_str()),
                Some(*code),
                "case {case:?}: expected {code:?} as the dominant language, got {entry}"
            );
            let score = languages
                .first()
                .and_then(|top| top["Score"].as_f64())
                .unwrap_or_default();
            assert!(
                score > 0.0 && score <= 1.0,
                "case {case:?}: expected a confidence in (0, 1], got {entry}"
            );
        }
    }
}

/// An input URI may name ONE file instead of a prefix: "if the prefix is a single
/// file, Amazon Comprehend uses that file as input. If more than one file begins with
/// the prefix, Amazon Comprehend uses all of them as input"
/// (`InputDataConfig$S3Uri`). The rows are the two forms of that one URI member.
#[tokio::test]
async fn an_input_uri_naming_one_file_analyses_only_that_file() {
    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    put_object(&comprehend, "in/english.txt", document("en").as_bytes()).await;
    put_object(&comprehend, "in/french.txt", document("fr").as_bytes()).await;

    for (case, s3_uri, expected) in [
        (
            "the prefix is one file",
            format!("s3://{BUCKET}/in/english.txt"),
            vec![("english.txt", "en")],
        ),
        (
            "the prefix is a collection",
            format!("s3://{BUCKET}/in/"),
            vec![("english.txt", "en"), ("french.txt", "fr")],
        ),
    ] {
        let submission = json!({
            "InputDataConfig": { "S3Uri": s3_uri },
            "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
            "DataAccessRoleArn": ROLE_ARN,
        });
        let (job_id, _) = start(&comprehend, case, &submission).await;
        let properties = settled(&comprehend, case, &job_id).await;
        assert_eq!(
            properties["JobStatus"].as_str(),
            Some("COMPLETED"),
            "case {case:?}: expected the run to complete, got {properties}"
        );
        let key = properties["OutputDataConfig"]["S3Uri"]
            .as_str()
            .unwrap_or_default()
            .trim_start_matches(&format!("s3://{BUCKET}/"))
            .to_string();
        let (status, archive) = get_object(&comprehend, &key).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reported output archive must be readable, got {status}"
        );
        let entries = archive_entries(case, &archive);
        let files: Vec<(String, String)> = entries
            .iter()
            .map(|entry| {
                (
                    entry["File"].as_str().unwrap_or_default().to_string(),
                    entry["Languages"][0]["LanguageCode"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(file, code)| ((*file).to_string(), (*code).to_string()))
            .collect();
        assert_eq!(
            files, expected,
            "case {case:?}: expected exactly the documents the URI names"
        );
    }
}

#[tokio::test]
async fn reading_a_job_does_not_change_what_a_later_read_sees() {
    let run = &RUN_CASES[0];
    let case = run.case;
    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    for (name, codes) in run.files {
        put_object(
            &comprehend,
            &run.input_key(name),
            run.file_body(codes).as_bytes(),
        )
        .await;
    }
    let (job_id, _) = start(&comprehend, case, &run.submission()).await;

    let once = settled(&comprehend, case, &job_id).await;
    let twice = describe(&comprehend, &job_id).await;
    assert_eq!(
        once, twice,
        "case \"terminal reads\": reading a finished job changed it: {once} then {twice}"
    );

    // And the archive it reported is still exactly one object: a read wrote nothing.
    let (status, first) = get_object(&comprehend, &run.output_key(&job_id)).await;
    assert_eq!(status, 200, "the output archive must still be readable");
    let (_, second) = get_object(&comprehend, &run.output_key(&job_id)).await;
    assert_eq!(
        first, second,
        "reading the output archive twice returned different bytes"
    );
}

#[tokio::test]
async fn a_run_this_emulator_cannot_carry_out_fails_with_the_reason() {
    // Each row is a submission whose run cannot be performed, and the phrase its
    // failure must name. The first mounts Comprehend ALONE, so nothing answers the
    // job's object-storage calls at all.
    let rows: &[(&str, bool, Value, &str)] = &[
        (
            "no object storage answering",
            false,
            json!({
                "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/in/") },
                "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
                "DataAccessRoleArn": ROLE_ARN,
            }),
            "Could not list the input documents",
        ),
        (
            "an input prefix holding no documents",
            true,
            json!({
                "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/empty/") },
                "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
                "DataAccessRoleArn": ROLE_ARN,
            }),
            "No input documents were found",
        ),
        (
            "a bucket that does not exist",
            true,
            json!({
                "InputDataConfig": { "S3Uri": "s3://no-such-bucket/in/" },
                "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
                "DataAccessRoleArn": ROLE_ARN,
            }),
            "Could not list the input documents",
        ),
        (
            "text extraction this emulator does not perform",
            true,
            json!({
                "InputDataConfig": {
                    "S3Uri": format!("s3://{BUCKET}/in/"),
                    "DocumentReaderConfig": { "DocumentReadAction": "TEXTRACT_DETECT_DOCUMENT_TEXT" },
                },
                "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
                "DataAccessRoleArn": ROLE_ARN,
            }),
            "Amazon Textract",
        ),
        (
            "a document that is not UTF-8 text",
            true,
            json!({
                "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/binary/") },
                "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
                "DataAccessRoleArn": ROLE_ARN,
            }),
            "not UTF-8 text",
        ),
        (
            "a document past the bytes this emulator reads from one document",
            true,
            json!({
                "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/oversized/") },
                "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
                "DataAccessRoleArn": ROLE_ARN,
            }),
            "bytes this emulator reads from one document",
        ),
    ];

    for (case, with_storage, submission, expected) in rows {
        let comprehend = if *with_storage {
            let comprehend = Comprehend::with_object_storage();
            create_bucket(&comprehend).await;
            put_object(&comprehend, "in/english.txt", document("en").as_bytes()).await;
            put_object(&comprehend, "binary/blob.bin", &[0xff, 0xfe, 0x00, 0x01]).await;
            // One document past the per-document byte bound, so the run refuses it
            // from the listing rather than buffering it.
            put_object(&comprehend, "oversized/huge.txt", &vec![b'a'; 1024 * 1024 + 1]).await;
            comprehend
        } else {
            Comprehend::new()
        };

        let (job_id, _) = start(&comprehend, case, submission).await;
        let properties = settled(&comprehend, case, &job_id).await;
        assert_eq!(
            properties["JobStatus"].as_str(),
            Some("FAILED"),
            "case {case:?}: a run that was not carried out must end FAILED, got {properties}"
        );
        let message = properties["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(expected),
            "case {case:?}: expected the failure to name {expected:?}, got {message:?}"
        );
        assert!(
            properties["EndTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: a terminal job carries an EndTime, got {properties}"
        );
        // Nothing was written, so the output configuration is still the submitted
        // one: no location is reported for an archive that does not exist.
        assert_eq!(
            properties["OutputDataConfig"], submission["OutputDataConfig"],
            "case {case:?}: a failed run must not report an output location, got {properties}"
        );
    }
}

#[tokio::test]
async fn a_client_request_token_identifies_one_submission() {
    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    put_object(&comprehend, "in/english.txt", document("en").as_bytes()).await;

    let base = json!({
        "JobName": "token-job",
        "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/in/") },
        "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
        "DataAccessRoleArn": ROLE_ARN,
        "VolumeKmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
        "Tags": [{ "Key": "team", "Value": "research" }],
        "ClientRequestToken": "token-abc-123",
    });

    let (first_id, first_arn) = start(&comprehend, "first submission", &base).await;
    // The same token with the same request is the SAME submission, however often it
    // is re-sent, and it creates no second job.
    for attempt in 0..3 {
        let replay = comprehend
            .ok("StartDominantLanguageDetectionJob", &base)
            .await;
        assert_eq!(
            replay["JobId"].as_str(),
            Some(first_id.as_str()),
            "case \"replay {attempt}\": a replayed token must return the first JobId, got {replay}"
        );
        assert_eq!(
            replay["JobArn"].as_str(),
            Some(first_arn.as_str()),
            "case \"replay {attempt}\": a replayed token must return the first JobArn, got {replay}"
        );
    }

    // The token and the request TOGETHER identify a submission: the same token
    // carrying a different request is a different submission, which mints its own
    // job rather than being refused — the model declares no error for it.
    let mut rows: Vec<(&str, Value)> = Vec::new();
    for (difference, member, value) in [
        ("a different JobName", "JobName", json!("other-job")),
        (
            "a different input location",
            "InputDataConfig",
            json!({ "S3Uri": format!("s3://{BUCKET}/in/english.txt") }),
        ),
        (
            "a different output location",
            "OutputDataConfig",
            json!({ "S3Uri": format!("s3://{BUCKET}/elsewhere/") }),
        ),
        (
            "a different role",
            "DataAccessRoleArn",
            json!("arn:aws:iam::210987654321:role/OtherRole"),
        ),
        (
            "a different volume key",
            "VolumeKmsKeyId",
            json!("abcd1234-12ab-34cd-56ef-1234567890ab"),
        ),
        (
            "one more tag",
            "Tags",
            json!([{ "Key": "team", "Value": "research" }, { "Key": "cost-centre" }]),
        ),
    ] {
        let mut body = base.clone();
        body[member] = value;
        rows.push((difference, body));
    }

    let mut minted: Vec<String> = vec![first_id.clone()];
    for (difference, body) in &rows {
        let output = comprehend
            .ok("StartDominantLanguageDetectionJob", body)
            .await;
        let job_id = output["JobId"].as_str().unwrap_or_default().to_string();
        assert!(
            !minted.contains(&job_id),
            "case {difference:?}: the same token on a different request must mint its own \
             job, got {output}"
        );
        // And it is a replay of ITSELF from then on.
        let replay = comprehend
            .ok("StartDominantLanguageDetectionJob", body)
            .await;
        assert_eq!(
            replay["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {difference:?}: re-sending that request must replay its own job, got {replay}"
        );
        minted.push(job_id);
    }

    // A submission carrying no token is never a replay.
    let mut untokened = base.clone();
    untokened.as_object_mut().expect("an object").remove("ClientRequestToken");
    let (first, _) = start(&comprehend, "no token, first", &untokened).await;
    let (second, _) = start(&comprehend, "no token, second", &untokened).await;
    assert_ne!(
        first, second,
        "case \"no ClientRequestToken\": each submission must mint its own JobId"
    );
}

#[tokio::test]
async fn describe_refuses_a_job_id_it_never_minted() {
    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    put_object(&comprehend, "in/english.txt", document("en").as_bytes()).await;
    // One real job, so each refusal is about the id and not about an empty store.
    let (job_id, _) = start(
        &comprehend,
        "seed",
        &json!({
            "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/in/") },
            "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
            "DataAccessRoleArn": ROLE_ARN,
        }),
    )
    .await;

    for case in [
        // Well formed, within the modelled 1..32 length, and minted by nobody.
        "0".repeat(32),
        "deadbeef".to_string(),
        "a".to_string(),
        // The real job's id with one character changed.
        format!("{}0", &job_id[..job_id.len() - 1]),
    ] {
        let (status, body) = comprehend
            .call(
                "DescribeDominantLanguageDetectionJob",
                &json!({ "JobId": case }),
            )
            .await;
        if case == job_id {
            continue;
        }
        assert_eq!(
            status, 400,
            "case JobId={case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "JobNotFoundException",
            "case JobId={case:?}: expected JobNotFoundException, got {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some("Could not find specified job."),
            "case JobId={case:?}: expected AWS's fixed sentence, got {body}"
        );
    }

    // Each family has its own id space: a dominant-language job's id names no
    // sentiment-detection job.
    let (status, body) = comprehend
        .call("DescribeSentimentDetectionJob", &json!({ "JobId": job_id }))
        .await;
    assert_eq!(
        status, 400,
        "case \"another family's id\": expected 400, got {status}: {body}"
    );
}

/// Poll `job_id` until `DescribeDominantLanguageDetectionJob` reports one of
/// `targets`, and return its properties then.
///
/// `settled` waits for the status the clock owes a RUN; this waits for whatever
/// status a row is about, which for a cancelled job is `STOPPED`.
async fn wait_for(comprehend: &Comprehend, case: &str, job_id: &str, targets: &[&str]) -> Value {
    let started = Instant::now();
    loop {
        let properties = describe(comprehend, job_id).await;
        let status = properties["JobStatus"].as_str().unwrap_or_default();
        assert!(
            JOB_STATUS_VALUES.contains(&status),
            "case {case:?}: {status:?} is not a modelled JobStatus, got {properties}"
        );
        if targets.contains(&status) {
            return properties;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: the job never reached any of {targets:?}, last {properties}"
        );
        sleep(Duration::from_millis(40));
    }
}

/// One job a stop is driven against: the submission it was started with and whether
/// the emulator it runs on answers the job's object-storage calls at all (so the run
/// it is racing against would end `COMPLETED` or `FAILED`).
struct StopCase {
    case: &'static str,
    with_object_storage: bool,
    submission: fn() -> Value,
}

/// A job whose run reads one document and completes.
fn completing_submission() -> Value {
    json!({
        "JobName": "stop-completing",
        "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/in/") },
        "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
        "DataAccessRoleArn": ROLE_ARN,
    })
}

/// A job carrying every optional member the submission models, whose run reads
/// several documents of several languages.
fn decorated_submission() -> Value {
    json!({
        "JobName": "stop.decorated/2026",
        "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/in/"), "InputFormat": "ONE_DOC_PER_LINE" },
        "OutputDataConfig": {
            "S3Uri": format!("s3://{BUCKET}/out/"),
            "KmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/abcd-1234",
        },
        "DataAccessRoleArn": ROLE_ARN,
        "VolumeKmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
        "VpcConfig": {
            "SecurityGroupIds": ["sg-0123456789abcdef0"],
            "Subnets": ["subnet-0123456789abcdef0"],
        },
        "Tags": [{ "Key": "team", "Value": "research" }],
        "ClientRequestToken": "stop-token-1",
    })
}

/// A job whose run cannot be carried out, because nothing answers its
/// object-storage calls: it is racing a `FAILED` rather than a `COMPLETED`.
fn failing_submission() -> Value {
    json!({
        "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/in/") },
        "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
        "DataAccessRoleArn": ROLE_ARN,
    })
}

/// The jobs the stop suites below are driven over. The rows differ along what the
/// rule generalises over: the optional members the submission carries, the number
/// and form of the input documents, and the outcome the run the stop races would
/// otherwise have reported.
const STOP_CASES: &[StopCase] = &[
    StopCase {
        case: "a run that would complete",
        with_object_storage: true,
        submission: completing_submission,
    },
    StopCase {
        case: "every optional member set, several documents per file",
        with_object_storage: true,
        submission: decorated_submission,
    },
    StopCase {
        case: "a run nothing can carry out",
        with_object_storage: false,
        submission: failing_submission,
    },
];

/// An emulator for `row`, with the input documents in place when it holds object
/// storage.
async fn emulator_for(row: &StopCase) -> Comprehend {
    if !row.with_object_storage {
        return Comprehend::new();
    }
    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    put_object(&comprehend, "in/english.txt", document("en").as_bytes()).await;
    put_object(
        &comprehend,
        "in/mixed.txt",
        format!("{}\n{}", document("fr"), document("ja")).as_bytes(),
    )
    .await;
    comprehend
}

/// Cancelling an analysis job is two-phase: a job that has not reached a status it
/// can never leave is MARKED for termination, so the stop answers `STOP_REQUESTED`
/// — never a terminal status — and the clock then owes the job `STOPPED`. Asserted
/// over every row, so it holds whatever the job was submitted with and whatever its
/// run would otherwise have reported.
#[tokio::test]
async fn a_stop_on_a_non_terminal_job_answers_stop_requested_then_settles_to_stopped() {
    let mut stopped_rows = 0usize;

    for row in STOP_CASES {
        let case = row.case;
        let comprehend = emulator_for(row).await;
        let submission = (row.submission)();
        let (job_id, _) = start(&comprehend, case, &submission).await;

        let (http_status, stopped) = comprehend
            .call(
                "StopDominantLanguageDetectionJob",
                &json!({ "JobId": job_id }),
            )
            .await;
        if http_status == 400 {
            // The clock owed this job its terminal status before the stop reached
            // it, which is a refusal — the other suite's case, so this row has
            // nothing left to assert.
            let properties = describe(&comprehend, &job_id).await;
            let status = properties["JobStatus"].as_str().unwrap_or_default();
            assert!(
                TERMINAL_JOB_STATUSES.contains(&status),
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
            "case {case:?}: a stop on a non-terminal job must answer STOP_REQUESTED, got {stopped}"
        );

        // The mark is PERSISTED: the very next read of the record reports it, and a
        // repeated stop while the job is being stopped reports the same mark again
        // rather than minting a second one.
        let marked = describe(&comprehend, &job_id).await;
        let marked_status = marked["JobStatus"].as_str().unwrap_or_default();
        assert!(
            marked_status == "STOP_REQUESTED" || marked_status == "STOPPED",
            "case {case:?}: a stopped job must read back as STOP_REQUESTED or STOPPED, got {marked}"
        );
        if marked_status == "STOP_REQUESTED" {
            let repeated = comprehend
                .ok(
                    "StopDominantLanguageDetectionJob",
                    &json!({ "JobId": job_id }),
                )
                .await;
            assert_eq!(
                repeated["JobStatus"].as_str(),
                Some("STOP_REQUESTED"),
                "case {case:?}: a repeated stop must report the mark already made, got {repeated}"
            );
        }

        // And the clock owes the marked job `STOPPED` — not the status its run
        // would otherwise have reported.
        let settled = wait_for(&comprehend, case, &job_id, &["STOPPED"]).await;
        assert_eq!(
            settled["JobStatus"].as_str(),
            Some("STOPPED"),
            "case {case:?}: a marked job must settle into STOPPED, got {settled}"
        );
        assert!(
            settled["EndTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: a terminal job carries an EndTime, got {settled}"
        );
        assert!(
            settled["Message"]
                .as_str()
                .is_some_and(|message| !message.trim().is_empty()),
            "case {case:?}: a stopped job reports why it stopped, got {settled}"
        );
        // Reading a terminal job does not move it on.
        let again = describe(&comprehend, &job_id).await;
        assert_eq!(
            settled, again,
            "case {case:?}: reading a stopped job changed it: {settled} then {again}"
        );
        stopped_rows += 1;
    }

    assert!(
        stopped_rows > 0,
        "every row's job had already finished before its stop arrived, so the \
         two-phase stop was never exercised"
    );
}

/// A stop on a job that already holds a status it can never leave has nothing to
/// mark for termination: it is refused with 400 `InvalidRequestException` and the
/// stored record is left exactly as it was.
///
/// The rows are the terminal statuses a dominant-language job can be driven into:
/// `COMPLETED`, which the clock owes a run that read every document, `FAILED`,
/// which it owes a run nothing could carry out, and `STOPPED`, which it owes a
/// cancelled job.
#[tokio::test]
async fn a_stop_on_a_terminal_job_is_refused_and_changes_nothing() {
    for terminal in TERMINAL_JOB_STATUSES {
        let row = match *terminal {
            "COMPLETED" => &STOP_CASES[0],
            "FAILED" => &STOP_CASES[2],
            _ => &STOP_CASES[1],
        };
        let comprehend = emulator_for(row).await;
        let (job_id, _) = start(&comprehend, terminal, &(row.submission)()).await;

        let mut expected = *terminal;
        if *terminal == "STOPPED" {
            let (http_status, stopped) = comprehend
                .call(
                    "StopDominantLanguageDetectionJob",
                    &json!({ "JobId": job_id }),
                )
                .await;
            // A 200 means the mark landed, so this job's terminal status will be
            // STOPPED; a 400 means the clock had already finished the run, which is
            // the COMPLETED row over again and is asserted identically below.
            if http_status == 200 {
                assert_eq!(
                    stopped["JobStatus"].as_str(),
                    Some("STOP_REQUESTED"),
                    "case {terminal}: the accepted stop must answer STOP_REQUESTED, got {stopped}"
                );
            } else {
                expected = "COMPLETED";
            }
        }

        let before = wait_for(&comprehend, terminal, &job_id, TERMINAL_JOB_STATUSES).await;
        assert_eq!(
            before["JobStatus"].as_str(),
            Some(expected),
            "case {terminal}: expected the job to settle into {expected}, got {before}"
        );

        // Twice, so the refusal is not a one-off and neither call moves the job.
        for attempt in 1..=2 {
            let (status, body) = comprehend
                .call(
                    "StopDominantLanguageDetectionJob",
                    &json!({ "JobId": job_id }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {terminal} attempt {attempt}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {terminal} attempt {attempt}: expected InvalidRequestException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                JOB_ALREADY_TERMINAL_MESSAGE,
                "case {terminal} attempt {attempt}: unexpected refusal message in {body}"
            );
            let after = describe(&comprehend, &job_id).await;
            assert_eq!(
                after, before,
                "case {terminal} attempt {attempt}: a refused stop must leave the job \
                 unchanged: {before} became {after}"
            );
        }
    }
}

/// A stop names a job that must exist: an id this family never minted is refused
/// with `JobNotFoundException`, whatever form the id takes, and the refusal
/// echoes none of it.
#[tokio::test]
async fn stop_refuses_a_job_id_this_family_never_minted() {
    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    put_object(&comprehend, "in/english.txt", document("en").as_bytes()).await;
    // One real job of this family, so each refusal is about the id and not about an
    // empty store.
    let (job_id, _) = start(&comprehend, "seed", &completing_submission()).await;

    // Every row satisfies the modelled `JobId` constraints (length 1..32, pattern
    // ^([\p{L}\p{Z}\p{N}_.:/=+\-%@]*)$), so each reaches the handler.
    let mut rows: Vec<String> = vec![
        "a".to_string(),
        "0".repeat(32),
        "doesnotexist".to_string(),
        "Job.Name:with/odd=chars".to_string(),
        // The real job's id with its last character changed.
        format!("{}0", &job_id[..job_id.len() - 1]),
    ];
    // Each family has its own id space: a sentiment-detection job's id names no
    // dominant-language job, so stopping it here is a not-found too.
    let other_family = comprehend
        .ok(
            "StartSentimentDetectionJob",
            &json!({
                "LanguageCode": "en",
                "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/in/") },
                "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/out/") },
                "DataAccessRoleArn": ROLE_ARN,
            }),
        )
        .await;
    rows.push(
        other_family["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("expected a sentiment JobId, got {other_family}"))
            .to_string(),
    );

    for case in rows {
        if case == job_id {
            continue;
        }
        let (status, body) = comprehend
            .call(
                "StopDominantLanguageDetectionJob",
                &json!({ "JobId": case }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case JobId={case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "JobNotFoundException",
            "case JobId={case:?}: expected JobNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            JOB_NOT_FOUND_MESSAGE,
            "case JobId={case:?}: expected the fixed not-found sentence, got {body}"
        );
        assert!(
            !error_message(&body).contains(case.as_str()),
            "case JobId={case:?}: the refusal must echo no caller-supplied id, got {body}"
        );
    }

    // The refused stops wrote nothing: the one real job is untouched and still the
    // only job this family holds.
    let properties = describe(&comprehend, &job_id).await;
    assert!(
        properties["JobId"].as_str() == Some(job_id.as_str()),
        "the seeded job must still be readable, got {properties}"
    );
}
