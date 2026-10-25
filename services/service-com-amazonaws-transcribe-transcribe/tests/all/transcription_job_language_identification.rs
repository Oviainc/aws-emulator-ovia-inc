// SPDX-License-Identifier: CC0-1.0

//! What a COMPLETED transcription job reports about the language the service
//! IDENTIFIED for it, end to end over the wire: `StartTranscriptionJob` ->
//! `GetTranscriptionJob` / `ListTranscriptionJobs`.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: a job whose language the service identifies — one started with
//! automatic language identification rather than with a language of its own — reports
//! the identification RESULT once it has completed, restricted to the `LanguageOptions`
//! the request allowed: the single identified `LanguageCode` with its
//! `IdentifiedLanguageScore` for single-language identification, or one `LanguageCodes`
//! entry per identified language for multi-language identification. A job that named
//! its own language had nothing identified for it and reports neither.
//!
//! Each test asserts that over a table spanning the axis the rule generalises over: how
//! many language codes the request allowed identification to choose between (none, one,
//! two, five) and which, including codes taken from the model's own `LanguageCode` enum,
//! crossed with whether execution was deferred so the job reaches the same end through
//! an extra status transition.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_transcribe_transcribe::ops::types::LANGUAGE_CODE_VALUES;
use service_com_amazonaws_transcribe_transcribe::state::{
    DEFAULT_IDENTIFIED_LANGUAGE, IDENTIFIED_LANGUAGE_SCORE,
};

use crate::transcription_jobs::Transcribe;

/// Long enough for every clock-owed transition of a job started during the test to
/// have fallen due: a deferred job's `QUEUED -> IN_PROGRESS -> COMPLETED` is two spans
/// of `crate::state::JOB_TRANSITION_DELAY` (500 ms each).
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_EVERY_TRANSITION: Duration = Duration::from_millis(1_300);

/// The role a deferred job is queued with, so the row that defers execution satisfies
/// the co-requirements `JobExecutionSettings` documents on itself.
const DEFERRED_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/transcribe-access";

/// What a request allowed identification to choose between: one row per count of
/// `LanguageOptions`, since that list is what "restricts [identification] to only the
/// language options that you specify".
///
/// The last row's codes are read off the model's own `LanguageCode` enum
/// (`LANGUAGE_CODE_VALUES`, from `smithy_query(query="shape", shape="LanguageCode")`)
/// rather than hand-picked, so the rule is exercised on codes this suite never names.
fn option_sets() -> Vec<(&'static str, Option<Vec<String>>)> {
    let first = LANGUAGE_CODE_VALUES
        .first()
        .expect("the model's LanguageCode enum carries values")
        .to_string();
    let last = LANGUAGE_CODE_VALUES
        .last()
        .expect("the model's LanguageCode enum carries values")
        .to_string();
    vec![
        ("no language options, so identification is narrowed to nothing", None),
        (
            "one option, which identification has no choice but to answer",
            Some(vec!["ja-JP".to_string()]),
        ),
        (
            "two options",
            Some(vec!["pt-BR".to_string(), "es-US".to_string()]),
        ),
        (
            "five options",
            Some(vec![
                "de-DE".to_string(),
                "en-GB".to_string(),
                "fr-FR".to_string(),
                "it-IT".to_string(),
                "nl-NL".to_string(),
            ]),
        ),
        (
            "the first and the last code the model's LanguageCode enum carries",
            Some(vec![first, last]),
        ),
    ]
}

/// The codes identification may answer with for `options`: the ones the request
/// allowed, or the service's default where it allowed nothing.
fn allowed_codes(options: &Option<Vec<String>>) -> Vec<String> {
    options
        .clone()
        .filter(|options| !options.is_empty())
        .unwrap_or_else(|| vec![DEFAULT_IDENTIFIED_LANGUAGE.to_string()])
}

/// A start request asking for the identification `member` names, over `options`, with
/// execution deferred when `deferred`.
fn start_request(
    name: &str,
    member: &str,
    options: &Option<Vec<String>>,
    deferred: bool,
) -> Value {
    let mut request = json!({
        "TranscriptionJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
        member: true
    });
    if let Some(options) = options {
        request["LanguageOptions"] = json!(options);
    }
    if deferred {
        request["JobExecutionSettings"] = json!({
            "AllowDeferredExecution": true,
            "DataAccessRoleArn": DEFERRED_ROLE_ARN
        });
    }
    request
}

/// Every row of the table, crossed with whether execution is deferred: the case name,
/// the job name, the options the request allows and the deferral.
fn rows(prefix: &str) -> Vec<(String, String, Option<Vec<String>>, bool)> {
    let mut rows = Vec::new();
    for (index, (case, options)) in option_sets().into_iter().enumerate() {
        for deferred in [false, true] {
            let suffix = if deferred { "deferred" } else { "immediate" };
            rows.push((
                format!("{case} ({suffix})"),
                format!("{prefix}-{index}-{suffix}"),
                options.clone(),
                deferred,
            ));
        }
    }
    rows
}

/// One job out of `ListTranscriptionJobs`' answer.
async fn summary_of(transcribe: &Transcribe, case: &str, name: &str) -> Value {
    let listed = transcribe
        .call("ListTranscriptionJobs", json!({"MaxResults": 100}))
        .await;
    listed.ok(case)["TranscriptionJobSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: TranscriptionJobSummaries must be a list"))
        .iter()
        .find(|summary| summary["TranscriptionJobName"] == json!(name))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the job must appear in the listing"))
}

#[tokio::test]
async fn single_language_identification_reports_the_identified_code_and_its_score() {
    let transcribe = Transcribe::new();
    let rows = rows("idlang");

    for (case, name, options, deferred) in &rows {
        let request = start_request(name, "IdentifyLanguage", options, *deferred);
        let started = transcribe.call("StartTranscriptionJob", request).await;
        let job = started.ok(case)["TranscriptionJob"].clone();
        // Identifying the language is part of processing the media, so a job that has
        // not finished reports no result yet.
        for member in ["LanguageCode", "IdentifiedLanguageScore", "LanguageCodes"] {
            assert!(
                job[member].is_null(),
                "case {case:?}: a job that has not completed reports no {member}, got {:?}",
                job[member]
            );
        }
        assert_eq!(
            job["IdentifyLanguage"],
            json!(true),
            "case {case:?}: the job must report that identification was enabled, got {:?}",
            job["IdentifyLanguage"]
        );
    }

    std::thread::sleep(PAST_EVERY_TRANSITION);

    for (case, name, options, _) in &rows {
        let expected = allowed_codes(options)
            .first()
            .cloned()
            .expect("at least one code is allowed");
        let fetched = transcribe
            .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
            .await;
        let job = fetched.ok(case)["TranscriptionJob"].clone();
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: the job must have completed, got {:?}",
            job["TranscriptionJobStatus"]
        );
        assert_eq!(
            job["LanguageCode"],
            json!(expected),
            "case {case:?}: expected the identified LanguageCode {expected:?}, got {:?}",
            job["LanguageCode"]
        );
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
        // Single-language identification reports the SINGULAR member; the plural one
        // belongs to multi-language identification.
        assert!(
            job["LanguageCodes"].is_null(),
            "case {case:?}: single-language identification reports no LanguageCodes, got {:?}",
            job["LanguageCodes"]
        );

        // The listing reports the same determination, not a different one.
        let summary = summary_of(&transcribe, case, name).await;
        assert_eq!(
            summary["LanguageCode"],
            json!(expected),
            "case {case:?}: the summary must report the same LanguageCode, got {:?}",
            summary["LanguageCode"]
        );
        assert_eq!(
            summary["IdentifiedLanguageScore"],
            json!(IDENTIFIED_LANGUAGE_SCORE),
            "case {case:?}: the summary must report the same score, got {:?}",
            summary["IdentifiedLanguageScore"]
        );

        // A read does not change what a later read sees.
        let again = transcribe
            .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
            .await;
        assert_eq!(
            again.ok(case)["TranscriptionJob"], job,
            "case {case:?}: a second read must report the same job"
        );
    }
}

#[tokio::test]
async fn multi_language_identification_reports_one_entry_per_identified_language() {
    let transcribe = Transcribe::new();
    let rows = rows("idmulti");

    for (case, name, options, deferred) in &rows {
        let request = start_request(name, "IdentifyMultipleLanguages", options, *deferred);
        let started = transcribe.call("StartTranscriptionJob", request).await;
        let job = started.ok(case)["TranscriptionJob"].clone();
        assert!(
            job["LanguageCodes"].is_null(),
            "case {case:?}: a job that has not completed reports no LanguageCodes, got {:?}",
            job["LanguageCodes"]
        );
    }

    std::thread::sleep(PAST_EVERY_TRANSITION);

    for (case, name, options, _) in &rows {
        let expected = allowed_codes(options);
        let fetched = transcribe
            .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
            .await;
        let job = fetched.ok(case)["TranscriptionJob"].clone();
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: the job must have completed, got {:?}",
            job["TranscriptionJobStatus"]
        );
        let identified: Vec<Value> = job["LanguageCodes"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a completed multi-language job reports LanguageCodes, got {:?}",
                    job["LanguageCodes"]
                )
            })
            .clone();
        let codes: Vec<Value> = identified
            .iter()
            .map(|item| item["LanguageCode"].clone())
            .collect();
        assert_eq!(
            codes,
            expected.iter().map(|code| json!(code)).collect::<Vec<_>>(),
            "case {case:?}: expected one entry per allowed language {expected:?}, got {:?}",
            job["LanguageCodes"]
        );
        // Multi-language identification reports the PLURAL member; the singular one
        // belongs to single-language identification, and the score that accompanies it.
        for member in ["LanguageCode", "IdentifiedLanguageScore"] {
            assert!(
                job[member].is_null(),
                "case {case:?}: multi-language identification reports no {member}, got {:?}",
                job[member]
            );
        }

        // The listing reports the same determination, not a different one.
        let summary = summary_of(&transcribe, case, name).await;
        assert_eq!(
            summary["LanguageCodes"], job["LanguageCodes"],
            "case {case:?}: the summary must report the same LanguageCodes, got {:?}",
            summary["LanguageCodes"]
        );

        // A read does not change what a later read sees.
        let again = transcribe
            .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
            .await;
        assert_eq!(
            again.ok(case)["TranscriptionJob"], job,
            "case {case:?}: a second read must report the same job"
        );
    }
}

#[tokio::test]
async fn a_job_that_names_its_own_language_has_nothing_identified() {
    let transcribe = Transcribe::new();

    // One row per language the request names itself, read off the model's own
    // `LanguageCode` enum (`LANGUAGE_CODE_VALUES`): the first, the last and one in the
    // middle, so the rule is checked on codes this suite does not hand-pick.
    let middle = LANGUAGE_CODE_VALUES.len() / 2;
    let named: Vec<&str> = vec![
        LANGUAGE_CODE_VALUES[0],
        LANGUAGE_CODE_VALUES[middle],
        LANGUAGE_CODE_VALUES[LANGUAGE_CODE_VALUES.len() - 1],
    ];

    for (index, code) in named.iter().enumerate() {
        let request = json!({
            "TranscriptionJobName": format!("named-{index}"),
            "Media": {"MediaFileUri": "s3://example-media/audio.flac"},
            "LanguageCode": code
        });
        transcribe
            .call("StartTranscriptionJob", request)
            .await
            .ok(code);
    }

    std::thread::sleep(PAST_EVERY_TRANSITION);

    for (index, code) in named.iter().enumerate() {
        let name = format!("named-{index}");
        let fetched = transcribe
            .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
            .await;
        let job = fetched.ok(code)["TranscriptionJob"].clone();
        assert_eq!(
            job["LanguageCode"],
            json!(code),
            "case {code:?}: a job keeps the language it was started with, got {:?}",
            job["LanguageCode"]
        );
        for member in ["IdentifiedLanguageScore", "LanguageCodes"] {
            assert!(
                job[member].is_null(),
                "case {code:?}: nothing was identified for this job, so it reports no \
                 {member}, got {:?}",
                job[member]
            );
        }
    }
}
