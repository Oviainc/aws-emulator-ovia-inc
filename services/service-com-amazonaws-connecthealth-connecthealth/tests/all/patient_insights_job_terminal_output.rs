// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of where a COMPLETED patient insights job reports its
//! output.
//!
//! The sibling suite (`patient_insights_job_lifecycle`) pins the round trip of a start
//! and the statuses the clock walks a job through. This one pins the terminal end of
//! that walk, over a table rather than one request:
//!
//! 1. A job the service accepted reaches the modelled terminal SUCCESS status, and a
//!    successful job reports an `insightsOutput` whose `uri` is UNDER the
//!    `outputDataConfig.s3OutputPath` the start configured and identifies the job that
//!    wrote it, whatever shape that configured path has.
//! 2. The reported `uri` satisfies the modelled `S3Uri` shape it fills — both its
//!    `@length` bound and its pattern — including for a configured path at the published
//!    bound, where no job prefix fits inside it.
//! 3. The location is a STORED property of the job: two reads report the same `uri`, and
//!    two jobs sharing one configured output path report different ones.
//!
//! The harness (`Emulator`, `error_code`, `string_at`), the two routes, the reader and
//! the minimal request body are the sibling suites': one way of driving this service,
//! not two.
//!
//! The constraints asserted here are the model's own
//! (`smithy_query(query="shape",shape="S3Uri,InsightsOutput,OutputDataConfig",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`),
//! quoted beside each constant.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use service_com_amazonaws_connecthealth_connecthealth::state::{
    MAX_S3_URI_LEN, PATIENT_INSIGHTS_JOB_STEP_SPAN,
};

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::patient_insights_job_lifecycle::{get_job, minimal_body, start_path};

/// `S3Uri`'s pattern: `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$` — so a bucket
/// field of 3..63 characters, of lowercase letters and digits, which dots and hyphens
/// may appear inside but not at either end.
const BUCKET_LEN: (usize, usize) = (3, 63);

/// The two spans a job takes to reach a terminal status (`SUBMITTED` -> `IN_PROGRESS` ->
/// terminal), plus a quarter span of slack, so both transitions the clock owes have
/// fallen due by the time a row is read.
fn past_both_spans() -> std::time::Duration {
    PATIENT_INSIGHTS_JOB_STEP_SPAN * 2 + PATIENT_INSIGHTS_JOB_STEP_SPAN / 4
}

/// Whether `uri` satisfies the modelled `S3Uri` shape — matched by hand, field by field,
/// rather than with a regex engine (the sibling suites in this workspace do the same).
fn is_s3_uri(uri: &str) -> bool {
    if uri.is_empty() || uri.len() > MAX_S3_URI_LEN {
        return false;
    }
    let Some(rest) = uri.strip_prefix("s3://") else {
        return false;
    };
    let bucket = match rest.find('/') {
        Some(at) => &rest[..at],
        None => rest,
    };
    (BUCKET_LEN.0..=BUCKET_LEN.1).contains(&bucket.len())
        && bucket.bytes().enumerate().all(|(index, byte)| {
            let inside = index > 0 && index + 1 < bucket.len();
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (inside && (byte == b'.' || byte == b'-'))
        })
}

/// A `StartPatientInsightsJob` body whose output path is `s3_output_path`.
fn body_with_output(s3_output_path: &str) -> Value {
    let mut body = minimal_body();
    body["outputDataConfig"] = json!({ "s3OutputPath": s3_output_path });
    body
}

/// A domain to start jobs in.
async fn domain_of(emulator: &Emulator, name: &str) -> String {
    string_at(&emulator.create_domain(&json!({ "name": name })).await, "domainId")
}

/// The configured output paths a job is driven with: one row per shape of `S3Uri` the
/// modelled pattern allows, so the rule is asserted over the CLASS of output paths rather
/// than one recorded body. The rows differ in whether a key follows the bucket, how deep
/// it goes, whether it ends in a separator (and how many), whether it names an object,
/// which characters the bucket field carries, and how long the whole path is.
fn output_path_rows() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = vec![
        ("a bucket root with no key".to_string(), "s3://clinic-output".to_string()),
        (
            "a bucket root with a trailing separator".to_string(),
            "s3://clinic-output/".to_string(),
        ),
        ("a one-segment prefix".to_string(), "s3://clinic-output/insights".to_string()),
        (
            "a one-segment prefix with a trailing separator".to_string(),
            "s3://clinic-output/insights/".to_string(),
        ),
        (
            "a deep prefix".to_string(),
            "s3://clinic-output/2026/01/24/insights/".to_string(),
        ),
        (
            "a path naming an object".to_string(),
            "s3://clinic-output/insights/run.json".to_string(),
        ),
        (
            "repeated trailing separators".to_string(),
            "s3://clinic-output/insights///".to_string(),
        ),
        // The bucket field at both published ends of its own 3..63 characters, with the
        // dots and hyphens the pattern allows inside it — a shape no example in this
        // service's issues or docs cites.
        ("the shortest bucket field".to_string(), "s3://abc/insights/".to_string()),
        (
            "the longest bucket field, with dots and hyphens".to_string(),
            format!("s3://{}/insights/", longest_bucket()),
        ),
    ];

    // `S3Uri` is `@length max 1024`: a path well inside the bound, and the bound itself,
    // where nothing can be appended without leaving the shape the reported `uri` fills.
    for len in [MAX_S3_URI_LEN - 100, MAX_S3_URI_LEN] {
        rows.push((format!("a configured path of {len} characters"), long_path(len)));
    }

    rows
}

/// A bucket field of exactly [`BUCKET_LEN`]`.1` characters, carrying the dots and hyphens
/// the modelled pattern allows inside one.
fn longest_bucket() -> String {
    let edge = "c";
    let inside = "linic-output.eu".repeat(BUCKET_LEN.1);
    format!("{edge}{}{edge}", &inside[..BUCKET_LEN.1 - 2])
}

/// A valid `S3Uri` of exactly `len` characters: a bucket and a key padded out to length.
fn long_path(len: usize) -> String {
    let head = "s3://clinic-output/";
    assert!(len > head.len(), "a path of {len} characters must hold the bucket");
    format!("{head}{}", "a".repeat(len - head.len()))
}

#[tokio::test]
async fn a_completed_job_reports_its_output_under_the_configured_path() {
    let emulator = Emulator::new("us-west-2");
    let domain_id = domain_of(&emulator, "clinic-terminal-output").await;

    // Two jobs per row, started before anything is read: the clock is what moves them,
    // so one wait covers the whole table, and the pair is what shows the location is per
    // JOB rather than per configured path.
    let rows = output_path_rows();
    let mut started: Vec<(String, String, Vec<String>)> = Vec::new();
    for (case, path) in &rows {
        let body = body_with_output(path);
        let mut ids = Vec::new();
        for attempt in ["first", "second"] {
            let (status, response) = emulator
                .send("POST", &start_path(&domain_id), Some(&body))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?} ({attempt} job): StartPatientInsightsJob must answer 200, \
                 got {status} with {response}"
            );
            ids.push(string_at(&response, "jobId"));
        }
        started.push((case.clone(), path.clone(), ids));
    }

    std::thread::sleep(past_both_spans());

    let mut reported: BTreeSet<String> = BTreeSet::new();
    for (case, path, ids) in &started {
        let mut uris: Vec<String> = Vec::new();
        for job_id in ids {
            let fetched = get_job(&emulator, case, &domain_id, job_id).await;
            assert_eq!(
                string_at(&fetched, "jobStatus"),
                "SUCCEEDED",
                "case {case:?}: a job the service accepted must reach the modelled terminal \
                 success status, got {fetched}"
            );
            let output = fetched
                .get("insightsOutput")
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: a succeeded job must report an \
                     insightsOutput, got {fetched}"));
            let uri = string_at(&output, "uri");
            assert!(
                is_s3_uri(&uri),
                "case {case:?}: the reported uri must satisfy the modelled S3Uri shape, \
                 got {uri:?} ({} characters)",
                uri.len()
            );

            // Under the configured path: the location starts at the path the start was
            // given, with any trailing separator of that path collapsed into the one
            // that joins it to what follows.
            let configured = path.trim_end_matches('/');
            assert!(
                uri.starts_with(configured),
                "case {case:?}: the reported uri must sit under the configured path \
                 {configured:?}, got {uri:?}"
            );

            // And it identifies the job that wrote it — except where the configured path
            // is so long that a job prefix would not fit inside the modelled `S3Uri`
            // bound, which is the one case the location stays the configured path.
            let room = configured.len() + job_id.len() + 2 <= MAX_S3_URI_LEN;
            if room {
                assert_eq!(
                    uri,
                    format!("{configured}/{job_id}/"),
                    "case {case:?}: the reported uri must name the job under the configured \
                     path, got {uri:?}"
                );
            } else {
                assert_eq!(
                    &uri, path,
                    "case {case:?}: a configured path with no room for a job prefix must be \
                     reported unchanged, got {uri:?}"
                );
            }

            // Reading it again reports the same location: it is stored, not recomputed
            // per read.
            let again = get_job(&emulator, case, &domain_id, job_id).await;
            assert_eq!(
                again, fetched,
                "case {case:?}: a terminal job must read the same every time, got {again}"
            );

            // Nothing is said about the status of a job that did what it was asked.
            assert!(
                fetched.get("statusDetails").is_none(),
                "case {case:?}: a job that ran to completion has no status detail, \
                 got {fetched}"
            );
            if room {
                assert!(
                    reported.insert(uri.clone()),
                    "case {case:?}: two jobs must not report the same output location, \
                     got {uri:?} twice"
                );
            }
            uris.push(uri);
        }
        if configured_has_room(path, &ids[0]) {
            assert_ne!(
                uris[0], uris[1],
                "case {case:?}: two jobs sharing the configured path {path:?} must report \
                 different output locations, got {uris:?}"
            );
        }
    }
}

/// Whether a job prefix fits under `path` inside the modelled `S3Uri` bound.
fn configured_has_room(path: &str, job_id: &str) -> bool {
    path.trim_end_matches('/').len() + job_id.len() + 2 <= MAX_S3_URI_LEN
}

#[tokio::test]
async fn an_output_path_past_the_modelled_bound_is_refused() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = domain_of(&emulator, "clinic-output-bound").await;

    // One character past `S3Uri`'s `@length max 1024`: the modelled refusal, so no job
    // is ever stored with an output path the shape does not allow.
    let case = "a configured path one character past the bound";
    let (status, refused) = emulator
        .send(
            "POST",
            &start_path(&domain_id),
            Some(&body_with_output(&long_path(MAX_S3_URI_LEN + 1))),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the modelled 400, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "case {case:?}: got {refused}"
    );
}
