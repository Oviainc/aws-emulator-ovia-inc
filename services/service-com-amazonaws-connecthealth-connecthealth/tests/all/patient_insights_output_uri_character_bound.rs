// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the UNIT the modelled `S3Uri` bound is measured in, where
//! a completed patient insights job's output location is DERIVED from the configured
//! output path.
//!
//! The sibling suite (`patient_insights_job_terminal_output`) pins the shape of that
//! location over the forms of path the modelled pattern allows, every one of them ASCII.
//! This one pins the rule along the dimension that table does not span — how wide the
//! characters of the configured path are:
//!
//! 1. A `@length` bound on a string counts CHARACTERS, so whether a derived location
//!    fits inside `S3Uri`'s bound depends on the characters of the configured path and
//!    never on how many bytes they encode to: a path of multi-byte characters whose
//!    derived location is inside the bound reports its `/<jobId>/` prefix exactly like an
//!    ASCII path of the same character count.
//! 2. The bound is still enforced: a configured path with no character room left for a
//!    job prefix is reported unchanged, whatever its characters encode to, and the
//!    reported `uri` is never longer than the bound its own member declares.
//! 3. The job prefix is what keeps two jobs sharing one configured path apart, so the
//!    pair of jobs of every row that has room reports two different locations.
//!
//! The harness (`Emulator`, `error_code`, `string_at`), the route, the reader, the
//! request body, the `S3Uri` check, the wait and the room rule are the sibling suites':
//! one way of driving this service, not two.
//!
//! The constraints asserted here are the model's own
//! (`smithy_query(query="shape",shape="S3Uri,InsightsOutput,OutputDataConfig",model_path="services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json")`):
//! `S3Uri` is `@length max 1024` with pattern
//! `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`, whose key segment `(/.*)?` admits
//! any character after the bucket field.

use std::collections::BTreeSet;

use serde_json::Value;

use service_com_amazonaws_connecthealth_connecthealth::state::MAX_S3_URI_LEN;

use crate::domain_subscription_lifecycle::{error_code, string_at, Emulator};
use crate::patient_insights_job_lifecycle::{get_job, start_path};
use crate::patient_insights_job_terminal_output::{
    body_with_output, configured_has_room, domain_of, is_s3_uri, past_both_spans,
};

/// The character widths the key segment of an `S3Uri` is driven with: one UTF-8 encoded
/// width each, so a row's character count and its byte count disagree by a known factor.
/// None of these characters is cited by this service's issues or docs.
const WIDTHS: [(&str, char, usize); 4] = [
    ("one-byte", 'a', 1),
    ("two-byte", '\u{e9}', 2),
    ("three-byte", '\u{65e5}', 3),
    ("four-byte", '\u{1d11e}', 4),
];

/// One row of the table: a configured output path, and whether a job prefix fits under it
/// inside the modelled bound.
#[derive(Debug)]
struct Row {
    case: String,
    path: String,
    /// Expected from the row's own CHARACTER count and the length of a minted job id —
    /// the rule — rather than read back off the service.
    prefix_expected: bool,
}

/// A valid `S3Uri` of exactly `len` characters whose key segment is `filler` repeated.
fn path_of(filler: char, len: usize) -> String {
    let head = "s3://clinic-output/";
    let head_chars = head.chars().count();
    assert!(
        len >= head_chars,
        "a path of {len} characters must hold the bucket field"
    );
    let key = filler.to_string().repeat(len - head_chars);
    format!("{head}{key}")
}

/// The table: every character width at every length class around the room a job prefix
/// needs, where `overhead` is the characters `/<jobId>/` adds to the configured path.
fn rows(overhead: usize) -> Vec<Row> {
    let lengths = [
        ("well inside the bound", 600, true),
        (
            "at the longest configured path a job prefix still fits under",
            MAX_S3_URI_LEN - overhead,
            true,
        ),
        (
            "one character past the room for a job prefix",
            MAX_S3_URI_LEN - overhead + 1,
            false,
        ),
        ("at the modelled bound", MAX_S3_URI_LEN, false),
    ];

    let mut rows = Vec::new();
    for (width, filler, encoded) in WIDTHS {
        for (length_case, len, prefix_expected) in lengths {
            let path = path_of(filler, len);
            rows.push(Row {
                case: format!(
                    "a {width} path {length_case}: {len} characters, {} bytes \
                     ({encoded} per filler character)",
                    path.len()
                ),
                path,
                prefix_expected,
            });
        }
    }
    rows
}

#[tokio::test]
async fn a_completed_jobs_output_location_is_bounded_by_characters_not_bytes() {
    let emulator = Emulator::new("us-west-2");
    let domain_id = domain_of(&emulator, "clinic-output-uri-characters").await;

    // What a derived location adds to the configured path is `/<jobId>/`: the id this
    // service mints plus two separators. Its length is read off a job rather than
    // restated here, so the boundary rows sit exactly where the rule puts them.
    let probe = start_job(&emulator, "a probe job", &domain_id, "s3://clinic-output/").await;
    let overhead = probe.chars().count() + 2;

    let rows = rows(overhead);
    assert!(
        rows.iter()
            .any(|row| row.prefix_expected && row.path.len() > MAX_S3_URI_LEN),
        "the table must span a configured path inside the bound in characters and past it \
         in bytes, else the unit under test is unpinned"
    );

    // Two jobs per row, all started before anything is read: the clock is what moves
    // them, so one wait covers the whole table, and the pair is what shows the location
    // is per JOB rather than per configured path.
    let mut started: Vec<(&Row, Vec<String>)> = Vec::new();
    for row in &rows {
        let mut ids = Vec::new();
        for attempt in ["first", "second"] {
            let case = format!("{} ({attempt} job)", row.case);
            ids.push(start_job(&emulator, &case, &domain_id, &row.path).await);
        }
        started.push((row, ids));
    }

    std::thread::sleep(past_both_spans());

    let mut reported: BTreeSet<String> = BTreeSet::new();
    for (row, ids) in &started {
        let case = &row.case;
        let configured = row.path.trim_end_matches('/');
        let mut uris: Vec<String> = Vec::new();
        for job_id in ids {
            let fetched = get_job(&emulator, case, &domain_id, job_id).await;
            assert_eq!(
                string_at(&fetched, "jobStatus"),
                "SUCCEEDED",
                "case {case:?}: a job the service accepted must reach the modelled terminal \
                 success status, got {fetched}"
            );
            let uri = string_at(
                fetched.get("insightsOutput").unwrap_or_else(|| {
                    panic!("case {case:?}: a succeeded job must report an insightsOutput, \
                         got {fetched}")
                }),
                "uri",
            );

            // The reported location fills an `S3Uri`, so it satisfies that shape — its
            // bound counted in characters, the unit `@length` on a string is defined in.
            assert!(
                is_s3_uri(&uri),
                "case {case:?}: the reported uri must satisfy the modelled S3Uri shape, \
                 got {uri:?} ({} characters, {} bytes)",
                uri.chars().count(),
                uri.len()
            );

            // The room a job prefix needs is a character count, so the service agrees
            // with the rule on this row whatever its characters encode to.
            assert_eq!(
                configured_has_room(&row.path, job_id),
                row.prefix_expected,
                "case {case:?}: whether a job prefix fits is a character count, so the \
                 room rule must agree with the row ({} characters configured, job id {} \
                 characters)",
                configured.chars().count(),
                job_id.chars().count()
            );

            if row.prefix_expected {
                assert_eq!(
                    uri,
                    format!("{configured}/{job_id}/"),
                    "case {case:?}: the reported uri must name the job under the configured \
                     path, got {uri:?} ({} characters)",
                    uri.chars().count()
                );
                assert!(
                    reported.insert(uri.clone()),
                    "case {case:?}: two jobs must not report the same output location, \
                     got {uri:?} twice"
                );
            } else {
                assert_eq!(
                    &uri, &row.path,
                    "case {case:?}: a configured path with no character room for a job \
                     prefix must be reported unchanged, got {uri:?} ({} characters)",
                    uri.chars().count()
                );
            }
            uris.push(uri);
        }
        if row.prefix_expected {
            assert_ne!(
                uris[0], uris[1],
                "case {case:?}: two jobs sharing one configured path must report different \
                 output locations, got {uris:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_output_path_past_the_modelled_bound_is_refused_at_every_character_width() {
    let emulator = Emulator::new("us-east-1");
    let domain_id = domain_of(&emulator, "clinic-output-uri-refusal").await;

    // One character past `S3Uri`'s `@length max 1024`, at each width: the bound the
    // boundary enforces is the modelled one, counted in the same characters, so no job is
    // ever stored with an output path the shape does not allow.
    for (width, filler, _) in WIDTHS {
        let path = path_of(filler, MAX_S3_URI_LEN + 1);
        let case = format!(
            "a {width} path one character past the bound: {} characters, {} bytes",
            path.chars().count(),
            path.len()
        );
        let (status, refused) = emulator
            .send("POST", &start_path(&domain_id), Some(&body_with_output(&path)))
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
}

/// `StartPatientInsightsJob` with `path` configured as the output path, asserting the
/// modelled success and returning the `jobId` it minted.
async fn start_job(emulator: &Emulator, case: &str, domain_id: &str, path: &str) -> String {
    let (status, started): (u16, Value) = emulator
        .send("POST", &start_path(domain_id), Some(&body_with_output(path)))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartPatientInsightsJob must answer 200, got {status} with {started}"
    );
    string_at(&started, "jobId")
}
