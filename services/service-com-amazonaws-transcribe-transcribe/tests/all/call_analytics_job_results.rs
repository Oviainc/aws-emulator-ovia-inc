// SPDX-License-Identifier: CC0-1.0

//! What a FINISHED Call Analytics job reports about the call, end to end over the
//! wire: `StartCallAnalyticsJob` -> `GetCallAnalyticsJob` / `ListCallAnalyticsJobs`.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), and the job helpers are the Call Analytics
//! lifecycle suite's (`crate::call_analytics_jobs`), reused rather than stood up a
//! second time.
//!
//! THE RULE UNDER TEST: `StartCallAnalyticsJob` carries no language, media-format or
//! sample-rate member, so each of those is the SERVICE's own determination, and a job
//! that has finished reports every one of them — the language identification was
//! narrowed to (the codes `Settings.LanguageOptions` allowed), with its confidence
//! score, the format of the audio it was given, and that audio's sample rate — plus
//! the location of each transcript its redaction settings asked for
//! (`Transcript.RedactedTranscriptFileUri` for a job that asked for redaction, beside
//! or instead of the unredacted `TranscriptFileUri`).
//!
//! Each test asserts that over a table spanning the axis the rule generalises over —
//! every modelled media format and the URI forms that name none, every count of
//! language options, every redaction setting against every documented output-location
//! form — not over the one request the rule was noticed on.

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::state::{
    CALL_AUDIO_SAMPLE_RATE_HERTZ, DEFAULT_IDENTIFIED_LANGUAGE, IDENTIFIED_LANGUAGE_SCORE,
};

use crate::call_analytics_jobs::{get, job_name, list, names_of, PAST_THE_TRANSITION};
use crate::transcription_jobs::Transcribe;

/// Every value the model's `MediaFormat` enum carries, read from the generated
/// constant rather than copied, so a change to the model reaches this table
/// (`smithy_query(query="shape", shape="MediaFormat")`).
const MEDIA_FORMATS: &[&str] =
    service_com_amazonaws_transcribe_transcribe::ops::types::MEDIA_FORMAT_VALUES;

/// Start every row of `requests`, wait out the one completion span they share, and
/// answer each row's finished job beside its case name.
async fn finished_jobs(requests: Vec<(String, Value)>) -> Vec<(String, Value)> {
    let transcribe = Transcribe::new();
    for (case, request) in &requests {
        transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await
            .ok(case);
    }
    // One wait for every row, not one per row.
    std::thread::sleep(PAST_THE_TRANSITION);

    let mut jobs = Vec::new();
    for (case, request) in &requests {
        let fetched = get(&transcribe, &job_name(request)).await;
        let job = fetched.ok(case)["CallAnalyticsJob"].clone();
        assert_eq!(
            job["CallAnalyticsJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: the processing span has elapsed, so the job must be \
             COMPLETED, got {:?}",
            job["CallAnalyticsJobStatus"]
        );
        jobs.push((case.clone(), job));
    }
    jobs
}

#[tokio::test]
async fn a_finished_job_reports_the_media_format_and_sample_rate_of_the_audio_it_was_given() {
    // One row per modelled format, under the file name the media URI carries it in —
    // plus the forms that carry no format: an extension that is not one, no extension
    // at all, a dot in a folder name only, and an extension behind the query string an
    // http URI may end with. The last row gives its input audio as
    // `RedactedMediaFileUri`, the member `StartCallAnalyticsJob` accepts instead of
    // `MediaFileUri`.
    let mut rows: Vec<(String, Value, Value)> = Vec::new();
    for (index, format) in MEDIA_FORMATS.iter().enumerate() {
        rows.push((
            format!("media file named .{format}"),
            json!({
                "CallAnalyticsJobName": format!("format-{index}"),
                "Media": {"MediaFileUri": format!("s3://example-media/call.{format}")}
            }),
            json!(format),
        ));
    }
    let extra: Vec<(&str, Value, Value)> = vec![
        (
            "an upper-case extension names the same format",
            json!({
                "CallAnalyticsJobName": "format-upper",
                "Media": {"MediaFileUri": "s3://example-media/CALL.WAV"}
            }),
            json!("wav"),
        ),
        (
            "an extension behind an http query string",
            json!({
                "CallAnalyticsJobName": "format-query",
                "Media": {"MediaFileUri": "https://example.invalid/media/call.ogg?versionId=7"}
            }),
            json!("ogg"),
        ),
        (
            "an extension that is not a media format",
            json!({
                "CallAnalyticsJobName": "format-other",
                "Media": {"MediaFileUri": "s3://example-media/call.txt"}
            }),
            Value::Null,
        ),
        (
            "a media URI with no extension at all",
            json!({
                "CallAnalyticsJobName": "format-none",
                "Media": {"MediaFileUri": "s3://example-media/call"}
            }),
            Value::Null,
        ),
        (
            "a dot in a folder name, not in the file name",
            json!({
                "CallAnalyticsJobName": "format-folder-dot",
                "Media": {"MediaFileUri": "s3://example-media/calls.2026/recording"}
            }),
            Value::Null,
        ),
        (
            "redacted input audio names the format instead",
            json!({
                "CallAnalyticsJobName": "format-redacted-media",
                "Media": {"RedactedMediaFileUri": "s3://example-media/call.amr"}
            }),
            json!("amr"),
        ),
    ];
    rows.extend(
        extra
            .into_iter()
            .map(|(case, request, expected)| (case.to_string(), request, expected)),
    );

    let requests: Vec<(String, Value)> = rows
        .iter()
        .map(|(case, request, _)| (case.clone(), request.clone()))
        .collect();
    let jobs = finished_jobs(requests).await;

    for ((case, _, expected_format), (_, job)) in rows.iter().zip(&jobs) {
        assert_eq!(
            &job["MediaFormat"], expected_format,
            "case {case:?}: expected MediaFormat {expected_format:?}, got {:?}",
            job["MediaFormat"]
        );
        // The sample rate is reported for every finished job, whatever the format:
        // nothing in the request states it, so the service's own figure answers.
        assert_eq!(
            job["MediaSampleRateHertz"],
            json!(CALL_AUDIO_SAMPLE_RATE_HERTZ),
            "case {case:?}: expected MediaSampleRateHertz {CALL_AUDIO_SAMPLE_RATE_HERTZ}, \
             got {:?}",
            job["MediaSampleRateHertz"]
        );
        // Whatever it is, it is a rate the member itself models
        // (`@range(min: 8000, max: 48000)`).
        let hertz = job["MediaSampleRateHertz"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: MediaSampleRateHertz must be a number"));
        assert!(
            (8_000..=48_000).contains(&hertz),
            "case {case:?}: MediaSampleRateHertz must be within the modelled range, \
             got {hertz}"
        );
    }
}

#[tokio::test]
async fn a_finished_job_reports_the_language_identification_was_narrowed_to() {
    // One row per count of `LanguageOptions` — none, one, two, and the five the member
    // documents as its recommended ceiling — each listing codes the modelled
    // `LanguageCode` enum carries. Identification may only answer with a code the
    // request allowed, so the first one listed is what each row expects; a row that
    // allows nothing gets the service's default.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "no settings at all",
            json!({
                "CallAnalyticsJobName": "lang-none",
                "Media": {"MediaFileUri": "s3://example-media/call.wav"}
            }),
            json!(DEFAULT_IDENTIFIED_LANGUAGE),
        ),
        (
            "settings that narrow nothing",
            json!({
                "CallAnalyticsJobName": "lang-empty-settings",
                "Media": {"MediaFileUri": "s3://example-media/call.wav"},
                "Settings": {"VocabularyFilterName": "blocked-terms",
                             "VocabularyFilterMethod": "tag"}
            }),
            json!(DEFAULT_IDENTIFIED_LANGUAGE),
        ),
        (
            "one language option, which identification has no choice but to answer",
            json!({
                "CallAnalyticsJobName": "lang-one",
                "Media": {"MediaFileUri": "s3://example-media/call.wav"},
                "Settings": {"LanguageOptions": ["ja-JP"]}
            }),
            json!("ja-JP"),
        ),
        (
            "two language options",
            json!({
                "CallAnalyticsJobName": "lang-two",
                "Media": {"MediaFileUri": "s3://example-media/call.wav"},
                "Settings": {"LanguageOptions": ["pt-BR", "es-US"]}
            }),
            json!("pt-BR"),
        ),
        (
            "five language options, the count the member recommends as a ceiling",
            json!({
                "CallAnalyticsJobName": "lang-five",
                "Media": {"MediaFileUri": "s3://example-media/call.wav"},
                "Settings": {
                    "LanguageOptions": ["de-DE", "en-GB", "fr-FR", "it-IT", "nl-NL"]
                }
            }),
            json!("de-DE"),
        ),
    ];

    let transcribe = Transcribe::new();
    transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "blocked-terms",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await
        .ok("CreateVocabularyFilter blocked-terms");
    for (case, request, _) in &rows {
        transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await
            .ok(case);
    }
    std::thread::sleep(PAST_THE_TRANSITION);

    let listed = list(&transcribe, json!({"MaxResults": 100})).await;
    let summaries = listed.ok("every finished job")["CallAnalyticsJobSummaries"].clone();
    assert_eq!(
        names_of(&summaries).len(),
        rows.len(),
        "every started job must be listed, got {summaries:?}"
    );

    for (case, request, expected_language) in &rows {
        let name = job_name(request);
        let fetched = get(&transcribe, &name).await;
        let job = fetched.ok(case)["CallAnalyticsJob"].clone();
        assert_eq!(
            &job["LanguageCode"], expected_language,
            "case {case:?}: expected LanguageCode {expected_language:?}, got {:?}",
            job["LanguageCode"]
        );
        // The score the identification is reported with, within the documented 0..1.
        assert_eq!(
            job["IdentifiedLanguageScore"],
            json!(IDENTIFIED_LANGUAGE_SCORE),
            "case {case:?}: expected IdentifiedLanguageScore {IDENTIFIED_LANGUAGE_SCORE}, \
             got {:?}",
            job["IdentifiedLanguageScore"]
        );
        let score = job["IdentifiedLanguageScore"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: IdentifiedLanguageScore must be a number"));
        assert!(
            (0.0..=1.0).contains(&score),
            "case {case:?}: a confidence score is between 0 and 1, got {score}"
        );

        // The listing reports the same determination, not a different one.
        let summary = summaries
            .as_array()
            .expect("CallAnalyticsJobSummaries must be a list")
            .iter()
            .find(|summary| summary["CallAnalyticsJobName"] == json!(name.clone()))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the job must appear in the listing"));
        assert_eq!(
            &summary["LanguageCode"], expected_language,
            "case {case:?}: the summary must report the same LanguageCode as the job, \
             got {:?}",
            summary["LanguageCode"]
        );

        // A read does not change what a later read sees.
        let again = get(&transcribe, &name).await;
        assert_eq!(
            again.ok(case)["CallAnalyticsJob"], job,
            "case {case:?}: a second read must report the same job"
        );
    }
}

#[tokio::test]
async fn a_finished_job_reports_the_transcripts_its_redaction_settings_asked_for() {
    // One row per way a request asks for redaction — not at all, `redacted`,
    // `redacted_and_unredacted`, and the redacted input audio that "produces a
    // redacted audio file in addition to a redacted transcript" — crossed with the
    // output-location forms `StartCallAnalyticsJobRequest$OutputLocation` documents,
    // so the redacted location is checked in the caller's bucket, in a folder of it,
    // at a file name the request chose, and in the service-managed location.
    let redactions: Vec<(&str, Value, bool, bool)> = vec![
        ("no redaction asked for", Value::Null, true, false),
        (
            "RedactionOutput redacted: only a redacted transcript",
            json!({"RedactionType": "PII", "RedactionOutput": "redacted"}),
            false,
            true,
        ),
        (
            "RedactionOutput redacted_and_unredacted: both transcripts",
            json!({
                "RedactionType": "PII",
                "RedactionOutput": "redacted_and_unredacted",
                "PiiEntityTypes": ["NAME", "PHONE"]
            }),
            true,
            true,
        ),
    ];
    let locations: Vec<(&str, Option<&str>)> = vec![
        ("no output location", None),
        ("a bucket alone", Some("s3://example-output")),
        ("a folder", Some("s3://example-output/analytics/")),
        (
            "a location naming the output file",
            Some("s3://example-output/analytics/chosen-name.json"),
        ),
    ];

    let mut rows: Vec<(String, Value, bool, bool)> = Vec::new();
    for (index, (location_case, location)) in locations.iter().enumerate() {
        for (redaction_index, (redaction_case, redaction, unredacted, redacted)) in
            redactions.iter().enumerate()
        {
            let mut request = json!({
                "CallAnalyticsJobName": format!("redaction-{index}-{redaction_index}"),
                "Media": {"MediaFileUri": "s3://example-media/call.flac"}
            });
            if let Some(location) = location {
                request["OutputLocation"] = json!(location);
            }
            if !redaction.is_null() {
                request["Settings"] = json!({"ContentRedaction": redaction.clone()});
            }
            rows.push((
                format!("{location_case}, {redaction_case}"),
                request,
                *unredacted,
                *redacted,
            ));
        }
        // Redacted input audio, which asks for a redacted transcript with no
        // `ContentRedaction` of its own.
        let mut request = json!({
            "CallAnalyticsJobName": format!("redaction-{index}-media"),
            "Media": {"RedactedMediaFileUri": "s3://example-media/call.flac"}
        });
        if let Some(location) = location {
            request["OutputLocation"] = json!(location);
        }
        rows.push((
            format!("{location_case}, redacted input audio"),
            request,
            true,
            true,
        ));
    }

    let requests: Vec<(String, Value)> = rows
        .iter()
        .map(|(case, request, _, _)| (case.clone(), request.clone()))
        .collect();
    let jobs = finished_jobs(requests).await;

    for ((case, _, unredacted, redacted), (_, job)) in rows.iter().zip(&jobs) {
        let transcript = job["Transcript"].clone();
        assert_eq!(
            transcript["TranscriptFileUri"].is_string(),
            *unredacted,
            "case {case:?}: an unredacted transcript location is reported exactly where \
             an unredacted transcript was asked for, got {transcript:?}"
        );
        assert_eq!(
            transcript["RedactedTranscriptFileUri"].is_string(),
            *redacted,
            "case {case:?}: a redacted transcript location is reported exactly where \
             redaction was asked for, got {transcript:?}"
        );
        if !*redacted {
            continue;
        }
        let redacted_uri = transcript["RedactedTranscriptFileUri"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a redacted location, got {transcript:?}"))
            .to_string();
        // Two separate files: the redacted one is a location of its own, in the same
        // folder the unredacted output goes to, under the job's own output name.
        assert!(
            redacted_uri.ends_with(".json"),
            "case {case:?}: the redacted transcript is a transcript file, got \
             {redacted_uri:?}"
        );
        if let Some(unredacted_uri) = transcript["TranscriptFileUri"].as_str() {
            assert_ne!(
                redacted_uri, unredacted_uri,
                "case {case:?}: the two transcripts are separate files, got \
                 {redacted_uri:?} for both"
            );
            let (redacted_folder, _) = redacted_uri
                .rsplit_once('/')
                .unwrap_or_else(|| panic!("case {case:?}: an S3 URI has a folder"));
            let (unredacted_folder, _) = unredacted_uri
                .rsplit_once('/')
                .unwrap_or_else(|| panic!("case {case:?}: an S3 URI has a folder"));
            assert_eq!(
                redacted_folder, unredacted_folder,
                "case {case:?}: both transcripts go where the request directed its \
                 output, got {redacted_uri:?} beside {unredacted_uri:?}"
            );
        }
    }

    // The spelling, pinned on the one row whose output location names its file: the
    // redacted transcript sits beside the unredacted one under its own name.
    let (case, _, _, _) = rows
        .iter()
        .find(|(case, _, _, _)| {
            case.contains("naming the output file") && case.contains("redacted_and_unredacted")
        })
        .unwrap_or_else(|| panic!("the row table must carry the file-named output row"));
    let (_, job) = jobs
        .iter()
        .find(|(job_case, _)| job_case == case)
        .unwrap_or_else(|| panic!("case {case:?}: the row must have a finished job"));
    assert_eq!(
        job["Transcript"],
        json!({
            "TranscriptFileUri": "s3://example-output/analytics/chosen-name.json",
            "RedactedTranscriptFileUri":
                "s3://example-output/analytics/redacted-chosen-name.json"
        }),
        "case {case:?}: unexpected transcript locations, got {:?}",
        job["Transcript"]
    );
}
