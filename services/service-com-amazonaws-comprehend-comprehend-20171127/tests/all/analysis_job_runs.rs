// SPDX-License-Identifier: CC0-1.0

//! The runs Comprehend's asynchronous analysis jobs perform, over object storage
//! mounted beside Comprehend on the same engine.
//!
//! RULE: a `Start*DetectionJob` of a family whose per-document analysis this emulator
//! performs READS every input document from the Amazon S3 location `InputDataConfig`
//! names, analyses it in process and writes the `output.tar.gz` back under the prefix
//! `OutputDataConfig` names, walking the documented `JobStatus` state machine
//! `SUBMITTED` -> `IN_PROGRESS` -> `COMPLETED` with every state OBSERVABLE to a
//! caller polling the family's Describe.
//!
//! The first suite is a TABLE over the input layouts a submission can point at (a
//! prefix of one file, a prefix of several, a URI naming one file, a document holding
//! no content word) and asserts the archive the key-phrases run wrote. The second is
//! a TABLE over every family whose run this emulator performs and asserts the state
//! machine, which is what the issue reported skipping `IN_PROGRESS`.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background.
use std::io::Read;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::comprehend_client::Comprehend;

/// The bucket every row's input and output live in.
const BUCKET: &str = "analysis-job-bucket";

/// Longest a suite waits for the clock to owe a job its next status.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

/// A document naming terms a key-phrase extractor can find, with stop words and
/// punctuation between them.
const LAPTOP_DOCUMENT: &str =
    "The new laptop screen is broken. Send a replacement screen for the laptop, quickly.";

/// A second document, so a prefix can hold more than one.
const INVOICE_DOCUMENT: &str = "Our final invoice covers the annual support contract.";

/// A document holding nothing but stop words: an extractor that invents nothing finds
/// no key phrase in it.
const STOP_WORDS_ONLY: &str = "the and of to a an is are it";

/// One row of the key-phrases run table.
struct Run {
    case: &'static str,
    /// The objects written under `BUCKET`, as `(key, body)`.
    objects: &'static [(&'static str, &'static str)],
    /// The `InputDataConfig$S3Uri` the submission names.
    input_uri: &'static str,
    /// The `InputDataConfig$InputFormat` the submission names, if any.
    input_format: Option<&'static str>,
    /// The `OutputDataConfig$S3Uri` prefix the submission names.
    output_prefix: &'static str,
    /// The `(File, Line)` of every output entry the run owes, in archive order.
    entries: &'static [(&'static str, Option<usize>)],
}

/// The key-phrases runs the first suite performs.
const RUNS: &[Run] = &[
    Run {
        case: "a prefix holding one file, one document per file",
        objects: &[("in/doc.txt", LAPTOP_DOCUMENT)],
        input_uri: "in/",
        // The model declares no default, so the submission names none and the run
        // treats each file as one document.
        input_format: None,
        output_prefix: "out/",
        entries: &[("doc.txt", None)],
    },
    Run {
        case: "a prefix holding two files, one document per line",
        objects: &[
            (
                "corpus/first.txt",
                "The new laptop screen is broken.\nSend a replacement screen for the laptop.",
            ),
            ("corpus/second.txt", INVOICE_DOCUMENT),
        ],
        input_uri: "corpus/",
        input_format: Some("ONE_DOC_PER_LINE"),
        output_prefix: "corpus-out/",
        entries: &[
            ("first.txt", Some(0)),
            ("first.txt", Some(1)),
            ("second.txt", Some(0)),
        ],
    },
    Run {
        // "If the prefix is a single file, Amazon Comprehend uses that file as
        // input" — the entry then names that object.
        case: "an input URI naming one file",
        objects: &[("solo.txt", LAPTOP_DOCUMENT), ("other.txt", INVOICE_DOCUMENT)],
        input_uri: "solo.txt",
        input_format: Some("ONE_DOC_PER_FILE"),
        output_prefix: "solo-out/deep/prefix/",
        entries: &[("solo.txt", None)],
    },
    Run {
        case: "a document holding no content word",
        objects: &[("empty/doc.txt", STOP_WORDS_ONLY)],
        input_uri: "empty/",
        input_format: None,
        output_prefix: "empty-out/",
        entries: &[("doc.txt", None)],
    },
];

/// Create `BUCKET`, so the runs' reads and writes have somewhere to go.
async fn create_bucket(comprehend: &Comprehend) {
    let (status, body) = comprehend.s3("PUT", &format!("/{BUCKET}"), b"").await;
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
        names.push(
            entry
                .path()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_default(),
        );
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
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("case {case:?}: output line {line:?} is not JSON: {error}"))
        })
        .collect()
}

/// The properties `operation` reports for `job_id`, under `member`.
async fn describe(comprehend: &Comprehend, operation: &str, member: &str, job_id: &str) -> Value {
    let output = comprehend
        .ok(operation, &json!({ "JobId": job_id }))
        .await;
    output[member].clone()
}

/// Poll `job_id` until the clock owes it a terminal status, returning every status
/// seen in order and the properties it ended with.
async fn settled(
    comprehend: &Comprehend,
    case: &str,
    operation: &str,
    member: &str,
    job_id: &str,
) -> (Vec<String>, Value) {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let properties = describe(comprehend, operation, member, job_id).await;
        let status = properties["JobStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if matches!(status.as_str(), "COMPLETED" | "FAILED" | "STOPPED") {
            return (seen, properties);
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: the job never reached a terminal status; statuses seen: {seen:?}"
        );
        sleep(Duration::from_millis(20));
    }
}

/// The characters of `text` from `begin` up to `end`, the half-open span of
/// zero-based character offsets a key phrase reports.
fn slice(text: &str, begin: usize, end: usize) -> String {
    text.chars().skip(begin).take(end - begin).collect()
}

/// The document one output entry was produced from, out of `run`'s objects.
fn document_of(run: &Run, file: &str, line: Option<usize>) -> String {
    let (_, body) = run
        .objects
        .iter()
        .find(|(key, _)| key.ends_with(file))
        .unwrap_or_else(|| panic!("case {:?}: no object matches the entry {file:?}", run.case));
    match line {
        None => (*body).to_string(),
        Some(line) => body
            .lines()
            .filter(|line| !line.trim().is_empty())
            .nth(line)
            .unwrap_or_else(|| panic!("case {:?}: {file:?} has no line {line}", run.case))
            .to_string(),
    }
}

#[tokio::test]
async fn a_key_phrases_job_analyses_its_documents_and_writes_the_output_archive() {
    for run in RUNS {
        let case = run.case;
        let comprehend = Comprehend::with_object_storage();
        create_bucket(&comprehend).await;
        for (key, body) in run.objects {
            put_object(&comprehend, key, body.as_bytes()).await;
        }

        let mut input = json!({ "S3Uri": format!("s3://{BUCKET}/{}", run.input_uri) });
        if let Some(format) = run.input_format {
            input["InputFormat"] = json!(format);
        }
        let submitted = comprehend
            .ok(
                "StartKeyPhrasesDetectionJob",
                &json!({
                    "JobName": "key-phrases-run",
                    "LanguageCode": "en",
                    "InputDataConfig": input,
                    "OutputDataConfig": {
                        "S3Uri": format!("s3://{BUCKET}/{}", run.output_prefix),
                    },
                    "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
                }),
            )
            .await;
        let job_id = submitted["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JobId, got {submitted}"))
            .to_string();

        let (seen, properties) = settled(
            &comprehend,
            case,
            "DescribeKeyPhrasesDetectionJob",
            "KeyPhrasesDetectionJobProperties",
            &job_id,
        )
        .await;
        assert_eq!(
            seen.last().map(String::as_str),
            Some("COMPLETED"),
            "case {case:?}: expected the run to COMPLETE, saw {seen:?}: {properties}"
        );
        assert!(
            properties["Message"].is_null(),
            "case {case:?}: a completed run has nothing to report, got {properties}"
        );

        // A finished job reports the archive's own location, under the prefix the
        // submission named and in the per-job directory the output documentation
        // describes.
        let reported = properties["OutputDataConfig"]["S3Uri"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected = format!(
            "s3://{BUCKET}/{}000000000000-KEY_PHRASES-{job_id}/output/output.tar.gz",
            run.output_prefix
        );
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the archive's own location, got {properties}"
        );

        let key = reported
            .strip_prefix(&format!("s3://{BUCKET}/"))
            .unwrap_or_default()
            .to_string();
        let (status, archive) = comprehend.s3("GET", &format!("/{BUCKET}/{key}"), b"").await;
        assert_eq!(
            status, 200,
            "case {case:?}: the reported output archive must be readable, got {status}"
        );
        let entries = archive_entries(case, &archive);
        assert_eq!(
            entries.len(),
            run.entries.len(),
            "case {case:?}: expected one entry per analysed document, got {entries:?}"
        );

        for (entry, (file, line)) in entries.iter().zip(run.entries) {
            assert_eq!(
                entry["File"].as_str(),
                Some(*file),
                "case {case:?}: an entry names the file it came from, got {entry}"
            );
            match line {
                None => assert!(
                    entry.get("Line").is_none(),
                    "case {case:?}: a one-document-per-file entry carries no Line, got {entry}"
                ),
                Some(line) => assert_eq!(
                    entry["Line"].as_u64(),
                    Some(*line as u64),
                    "case {case:?}: an entry names the line it came from, got {entry}"
                ),
            }

            let document = document_of(run, file, *line);
            let phrases = entry["KeyPhrases"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected KeyPhrases, got {entry}"));
            if document == STOP_WORDS_ONLY {
                assert!(
                    phrases.is_empty(),
                    "case {case:?}: a document holding no content word has no key phrase, got \
                     {entry}"
                );
                continue;
            }
            assert!(
                !phrases.is_empty(),
                "case {case:?}: expected the key phrases of {document:?}, got {entry}"
            );

            let mut previous_end = 0usize;
            let mut top_scores = 0usize;
            for phrase in phrases {
                let begin = phrase["BeginOffset"].as_u64().unwrap_or_default() as usize;
                let end = phrase["EndOffset"].as_u64().unwrap_or_default() as usize;
                let text = phrase["Text"].as_str().unwrap_or_default();
                // The phrase is a span of the document ITSELF: slicing the document
                // by the offsets reported returns the text reported.
                assert!(
                    begin < end,
                    "case {case:?}: expected a non-empty span, got {phrase}"
                );
                assert_eq!(
                    slice(&document, begin, end),
                    text,
                    "case {case:?}: the offsets must span the phrase's own text in \
                     {document:?}, got {phrase}"
                );
                // Phrases are reported in document order and never overlap.
                assert!(
                    begin >= previous_end,
                    "case {case:?}: phrases must be in document order and disjoint, got \
                     {phrase} after offset {previous_end}"
                );
                previous_end = end;
                let score = phrase["Score"].as_f64().unwrap_or_default();
                assert!(
                    score > 0.0 && score <= 1.0,
                    "case {case:?}: expected a Score in (0, 1], got {phrase}"
                );
                if (score - 1.0).abs() < 1e-6 {
                    top_scores += 1;
                }
            }
            assert!(
                top_scores >= 1,
                "case {case:?}: the document's highest-scoring phrase scores 1.0, got {phrases:?}"
            );
            // The phrases name terms the document names.
            if document.contains("laptop") {
                assert!(
                    phrases.iter().any(|phrase| phrase["Text"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("laptop")),
                    "case {case:?}: expected a phrase naming a term of {document:?}, got \
                     {phrases:?}"
                );
            }
        }
    }
}

/// One `Start*DetectionJob` family whose run this emulator performs.
struct Family {
    start: &'static str,
    describe: &'static str,
    properties: &'static str,
    /// Whether the family's submission models a `LanguageCode` (the
    /// dominant-language family detects it instead).
    language: bool,
}

/// Every family whose per-document analysis this emulator performs in process.
const FAMILIES: &[Family] = &[
    Family {
        start: "StartKeyPhrasesDetectionJob",
        describe: "DescribeKeyPhrasesDetectionJob",
        properties: "KeyPhrasesDetectionJobProperties",
        language: true,
    },
    Family {
        start: "StartSentimentDetectionJob",
        describe: "DescribeSentimentDetectionJob",
        properties: "SentimentDetectionJobProperties",
        language: true,
    },
    Family {
        start: "StartDominantLanguageDetectionJob",
        describe: "DescribeDominantLanguageDetectionJob",
        properties: "DominantLanguageDetectionJobProperties",
        language: false,
    },
];

#[tokio::test]
async fn a_job_walks_submitted_then_in_progress_then_completed() {
    // The documented JobStatus state machine, with every state observable: the
    // issue's report was a job that flipped straight from SUBMITTED to COMPLETED.
    const LIFECYCLE: &[&str] = &["SUBMITTED", "IN_PROGRESS", "COMPLETED"];

    for family in FAMILIES {
        let case = family.start;
        let comprehend = Comprehend::with_object_storage();
        create_bucket(&comprehend).await;
        put_object(&comprehend, "lifecycle/doc.txt", LAPTOP_DOCUMENT.as_bytes()).await;

        let mut body = json!({
            "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/lifecycle/") },
            "OutputDataConfig": { "S3Uri": format!("s3://{BUCKET}/lifecycle-out/") },
            "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
        });
        if family.language {
            body["LanguageCode"] = json!("en");
        }
        let submitted = comprehend.ok(family.start, &body).await;
        assert_eq!(
            submitted["JobStatus"].as_str(),
            Some("SUBMITTED"),
            "case {case:?}: a freshly started job must be SUBMITTED, got {submitted}"
        );
        let job_id = submitted["JobId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JobId, got {submitted}"))
            .to_string();

        // The statuses a poller sees are a prefix-stable walk ALONG the documented
        // order, never a jump over `IN_PROGRESS`: the run's report carries the
        // remaining spans forward, so the running state is there to be seen however
        // long the run took. The first poll may already be past `SUBMITTED` on a
        // loaded machine — the Start response above reported it — so what is asserted
        // is the ORDER and that `IN_PROGRESS` was observed.
        let (seen, properties) = settled(
            &comprehend,
            case,
            family.describe,
            family.properties,
            &job_id,
        )
        .await;
        let ranks: Vec<usize> = seen
            .iter()
            .map(|status| {
                LIFECYCLE
                    .iter()
                    .position(|known| known == status)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: unexpected status {status:?} in {seen:?}")
                    })
            })
            .collect();
        assert!(
            ranks.windows(2).all(|pair| pair[0] < pair[1]),
            "case {case:?}: the statuses must move forward along {LIFECYCLE:?}, saw {seen:?}"
        );
        assert!(
            seen.iter().any(|status| status == "IN_PROGRESS"),
            "case {case:?}: a running job must be observable as IN_PROGRESS, saw {seen:?}: \
             {properties}"
        );
        assert_eq!(
            seen.last().map(String::as_str),
            Some("COMPLETED"),
            "case {case:?}: expected the run to COMPLETE, saw {seen:?}: {properties}"
        );
        assert!(
            properties["EndTime"].as_f64().is_some_and(|at| at > 0.0),
            "case {case:?}: a terminal job carries an EndTime, got {properties}"
        );
        // Reading a settled job never moves it on.
        let once = describe(&comprehend, family.describe, family.properties, &job_id).await;
        let twice = describe(&comprehend, family.describe, family.properties, &job_id).await;
        assert_eq!(
            once, twice,
            "case {case:?}: reading a finished job changed it: {once} then {twice}"
        );
    }
}
