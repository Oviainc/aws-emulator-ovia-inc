// SPDX-License-Identifier: CC0-1.0

//! The `MediaFormat` a FINISHED job reports for the media it processed, end to end over
//! the wire: `StartTranscriptionJob` / `StartMedicalTranscriptionJob` /
//! `StartCallAnalyticsJob` -> the reader of each family.
//!
//! The emulator, the calls and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), and the per-family start requests are the
//! media-resolution suite's (`crate::media_object_resolution::Family`), reused rather
//! than stood up a second time.
//!
//! THE RULE UNDER TEST: `MediaFormat` is "The format of the input media file" on every
//! job shape that reports one, so it is a fact about the FILE rather than an echo of the
//! request: a job that has finished reports the format the service determined for its
//! input audio — read off the name that audio is stored under — wherever the request
//! declared none, and a format the request DID declare stands, which a finished job's
//! media cannot contradict (a job whose media contradicts it ends `FAILED` instead, which
//! `crate::media_object_resolution` pins). Media whose name evidences no modelled format
//! leaves the member absent rather than guessed. Determining it is part of PROCESSING the
//! media, so a job that is still in progress reports only what the request declared.
//!
//! Both halves are asserted over a TABLE whose rows differ along what the rule
//! generalises over: every value the model's `MediaFormat` enum carries, the URI forms
//! that evidence one (case, a query string behind it) and the forms that evidence none
//! (an extension that is no format, no extension at all, a dot in a folder name), and
//! whether the request declared a format of its own — each row driven through BOTH
//! families whose start request carries the member. The last test drives one medium
//! through every family that reports the member at all, which is the whole point of the
//! rule: one determination, not one per family.

use serde_json::{json, Value};

use crate::media_object_resolution::{Family, PAST_THE_TRANSITION};
use crate::transcription_jobs::{Reply, Transcribe};

/// Every value the model's `MediaFormat` enum carries, read from the generated constant
/// rather than copied, so a change to the model reaches this table
/// (`smithy_query(query="shape", shape="MediaFormat")`).
const MEDIA_FORMATS: &[&str] =
    service_com_amazonaws_transcribe_transcribe::ops::types::MEDIA_FORMAT_VALUES;

/// The families whose start request carries a `MediaFormat` member of its own
/// (`smithy_query(query="operation", operation="StartTranscriptionJob,StartMedicalTranscriptionJob")`),
/// so every row of the table below is driven through each of them.
const DECLARING_FAMILIES: &[Family] = &[Family::Transcription, Family::Medical];

/// One row: the media a start is pointed at, what the request declares the format to be,
/// and what the finished job must report it as.
struct FormatCase {
    /// What this row varies, named in every assertion.
    case: String,
    /// The `MediaFileUri` the start names.
    uri: String,
    /// The `MediaFormat` the request declares, or `None` for a request declaring none.
    declared: Option<&'static str>,
    /// The `MediaFormat` the finished job must report, or `None` for media that
    /// evidences none.
    expected: Option<&'static str>,
}

/// The table: one row per modelled format under the file name that carries it, the two
/// URI forms that still evidence one, the three that evidence none, and the rows whose
/// request declares a format of its own.
fn format_cases() -> Vec<FormatCase> {
    let mut rows: Vec<FormatCase> = MEDIA_FORMATS
        .iter()
        .map(|format| FormatCase {
            case: format!("media stored under a name ending .{format}, nothing declared"),
            uri: format!("s3://example-media/dictation.{format}"),
            declared: None,
            expected: Some(format),
        })
        .collect();
    let extra: Vec<(&'static str, &'static str, Option<&'static str>, Option<&'static str>)> = vec![
        (
            "an upper-case extension names the same format",
            "s3://example-media/DICTATION.WAV",
            None,
            Some("wav"),
        ),
        (
            "an extension behind an http query string",
            "https://example.invalid/media/dictation.ogg?versionId=7",
            None,
            Some("ogg"),
        ),
        (
            "an extension that is not a modelled media format",
            "s3://example-media/dictation.txt",
            None,
            None,
        ),
        (
            "a media URI with no extension at all",
            "s3://example-media/dictation",
            None,
            None,
        ),
        (
            "a dot in a folder name, not in the file name",
            "s3://example-media/clinic.2026/dictation",
            None,
            None,
        ),
        (
            "a declared format the stored media's own name agrees with",
            "s3://example-media/dictation.flac",
            Some("flac"),
            Some("flac"),
        ),
        (
            "a declared format over media whose name evidences none",
            "s3://example-media/dictation",
            Some("mp4"),
            Some("mp4"),
        ),
    ];
    rows.extend(
        extra
            .into_iter()
            .map(|(case, uri, declared, expected)| FormatCase {
                case: case.to_string(),
                uri: uri.to_string(),
                declared,
                expected,
            }),
    );
    rows
}

/// The job name a row is started under: unique per family and row, and within the
/// `^[0-9a-zA-Z._-]+$` pattern every job name of this service is modelled with.
fn job_name(family_index: usize, row_index: usize) -> String {
    format!("media-format-{family_index}-{row_index}")
}

/// The status member of whichever family answered — the three readers name their own.
fn status_of(job: &Value) -> &str {
    job["TranscriptionJobStatus"]
        .as_str()
        .or_else(|| job["CallAnalyticsJobStatus"].as_str())
        .unwrap_or_default()
}

/// Read one job of `family` back by name.
async fn read_job(transcribe: &Transcribe, family: Family, name: &str) -> Reply {
    transcribe
        .call(family.get(), json!({family.name_member(): name}))
        .await
}

#[tokio::test]
async fn a_finished_job_reports_the_format_its_input_media_evidences() {
    let transcribe = Transcribe::new();
    let rows = format_cases();

    for (family_index, family) in DECLARING_FAMILIES.iter().enumerate() {
        for (row_index, row) in rows.iter().enumerate() {
            let name = job_name(family_index, row_index);
            let mut request = family.start_request(&name, json!({"MediaFileUri": row.uri}));
            if let Some(declared) = row.declared {
                request["MediaFormat"] = json!(declared);
            }
            let answered = transcribe.call(family.start(), request).await;
            let started = &answered.ok(&row.case)[family.job_member()];
            // Determining the format is part of PROCESSING the media, so a job that has
            // only just been started reports what the request declared and nothing more.
            assert_eq!(
                started["MediaFormat"],
                row.declared.map_or(Value::Null, |declared| json!(declared)),
                "case {:?} ({}): a start reports only the declared format, got {started:?}",
                row.case,
                family.start()
            );
        }
    }

    // One wait for every row of every family, not one per row.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (family_index, family) in DECLARING_FAMILIES.iter().enumerate() {
        for (row_index, row) in rows.iter().enumerate() {
            let name = job_name(family_index, row_index);
            let read = read_job(&transcribe, *family, &name).await;
            let job = &read.ok(&row.case)[family.job_member()];
            let status = status_of(job);
            assert_eq!(
                status, "COMPLETED",
                "case {:?} ({}): the processing span has elapsed, so the job must be \
                 COMPLETED, got {job:?}",
                row.case,
                family.get()
            );
            let expected = row.expected.map_or(Value::Null, |format| json!(format));
            assert_eq!(
                job["MediaFormat"], expected,
                "case {:?} ({}): expected MediaFormat {expected:?}, got {:?}",
                row.case,
                family.get(),
                job["MediaFormat"]
            );
            // Whatever it reports is a value of the modelled enum, never an invented
            // label read off a file name.
            if let Some(reported) = job["MediaFormat"].as_str() {
                assert!(
                    MEDIA_FORMATS.contains(&reported),
                    "case {:?} ({}): a reported MediaFormat must be a value the model \
                     carries, got {reported:?}",
                    row.case,
                    family.get()
                );
            }

            // Reading a finished job again reports the very same determination: it is a
            // function of the stored record, so no read changes what the next one sees.
            let again = read_job(&transcribe, *family, &name).await;
            assert_eq!(
                again.ok(&row.case)[family.job_member()]["MediaFormat"], expected,
                "case {:?} ({}): a second read must report the same MediaFormat, got {:?}",
                row.case,
                family.get(),
                again.body
            );
        }
    }
}

#[tokio::test]
async fn one_medium_reports_one_format_in_every_family_that_reports_the_member() {
    // The three job families whose output shape carries `MediaFormat`
    // (`TranscriptionJob`, `MedicalTranscriptionJob` and `CallAnalyticsJob` each model
    // it as "The format of the input media file"); `MedicalScribeJob` models no such
    // member, so it is not among them.
    const FAMILIES: &[Family] = &[Family::Transcription, Family::Medical, Family::CallAnalytics];
    // Two media: one whose stored name evidences a modelled format and one whose name
    // evidences none. Neither start declares a format — `StartCallAnalyticsJob` carries
    // no member to declare one with, which is what makes the two media the common
    // ground between the families.
    let media: &[(&str, &str, Option<&str>)] = &[
        (
            "media stored under a name ending .wav",
            "s3://example-media/dictation.wav",
            Some("wav"),
        ),
        (
            "media stored under a name ending in no modelled format",
            "s3://example-media/dictation.bin",
            None,
        ),
    ];

    let transcribe = Transcribe::new();
    for (family_index, family) in FAMILIES.iter().enumerate() {
        for (row_index, (case, uri, _)) in media.iter().enumerate() {
            let name = job_name(family_index, row_index);
            let request = family.start_request(&name, json!({"MediaFileUri": uri}));
            transcribe.call(family.start(), request).await.ok(case);
        }
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    for (family_index, family) in FAMILIES.iter().enumerate() {
        for (row_index, (case, _, expected_format)) in media.iter().enumerate() {
            let name = job_name(family_index, row_index);
            let read = read_job(&transcribe, *family, &name).await;
            let job = &read.ok(case)[family.job_member()];
            assert_eq!(
                status_of(job),
                "COMPLETED",
                "case {case:?} ({}): the job must be COMPLETED before it reports a \
                 determination, got {job:?}",
                family.get()
            );
            let expected = expected_format.map_or(Value::Null, |format| json!(format));
            assert_eq!(
                job["MediaFormat"], expected,
                "case {case:?} ({}): every family reporting the member must report the \
                 same determination {expected:?} for the same medium, got {:?}",
                family.get(),
                job["MediaFormat"]
            );
        }
    }
}
