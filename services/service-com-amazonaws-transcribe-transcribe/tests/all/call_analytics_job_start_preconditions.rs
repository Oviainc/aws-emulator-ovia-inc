// SPDX-License-Identifier: CC0-1.0

//! What has to be TRUE ALREADY before a `StartCallAnalyticsJob` request becomes a job
//! record — asserted end to end, with the emulator's own Amazon S3 mounted beside
//! transcribe on one engine, the way the assembled emulator runs them.
//!
//! THE RULE UNDER TEST: a Call Analytics start names things that must exist when it is
//! made — its input audio as an object in Amazon S3 ("To make a StartCallAnalyticsJob
//! request, you must first upload your media file into an Amazon S3 bucket; you can then
//! specify the Amazon S3 location of the file using the Media parameter") and, in its
//! settings, a custom vocabulary, custom vocabulary filter or custom language model this
//! account holds (`smithy_query(query="shape",
//! shape="CallAnalyticsJobSettings,LanguageIdSettings")`) — so a start naming any of
//! them that is not there is refused with the operation's modelled client error
//! (`BadRequestException`, HTTP 400) and NOTHING is persisted: no read and no listing
//! reports a job, the clock finishes none, and the name the refused start asked for is
//! still free.
//!
//! The two halves are asserted over ONE table whose rows differ along which precondition
//! is unmet (the media, each settings member that names a custom resource, and both at
//! once), where in the request it sits (`Media.MediaFileUri`,
//! `Media.RedactedMediaFileUri`, top-level `Settings`, a per-language
//! `Settings.LanguageIdSettings` entry), which part of the media address does not resolve
//! (the bucket, the key) and which family the named resource would have to be held in.
//! The first test drives the refusal, the second drives the SAME rows once the media is
//! staged and the resources are held, which is what shows the refusals stored nothing and
//! reserved nothing.
//!
//! The media rule across all four start operations of the service is
//! `crate::media_object_resolution`'s subject and the settings rule per member is
//! `crate::call_analytics_job_start_limits`'s; this suite is the Call Analytics start's
//! own composition of them — both preconditions, one emulator, through to the listing and
//! the clock.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::call_analytics_jobs::{create_named_resources, get, list, names_of, PAST_THE_TRANSITION};
use crate::media_object_resolution::{emulator, given_staged_media, ABSENT_BUCKET, BUCKET, STAGED_KEYS};
use crate::transcription_job_absent_media_state::MEDIA_NAMES_NO_OBJECT;
use crate::transcription_job_request_rules::not_found_message;
use crate::transcription_jobs::Transcribe;

/// The custom resources [`create_named_resources`] records, named by the rows below: in
/// the first test nothing creates them, in the second that helper does.
const VOCABULARY: &str = "call-terms";
const VOCABULARY_FILTER: &str = "blocked-terms";
const LANGUAGE_MODEL: &str = "support-model";

/// Transcribe and Amazon S3 on one engine, with the media objects the rows below name
/// when they mean to resolve already stored. The harness is the transcription-job
/// suite's, wrapped around the two-service router rather than its single-service one, so
/// this suite gets that harness's call, answer assertions and vocabulary-build wait
/// without a second one being written.
async fn emulator_with_staged_media() -> Transcribe {
    let router: Arc<smithy_server_runtime::MultiServiceRouter> = emulator();
    given_staged_media(&router).await;
    Transcribe { router }
}

/// Which precondition a row's request does not meet, which is what decides the refusal's
/// wording.
enum Unmet {
    /// The media the start names is an address the object store reports it does not
    /// hold.
    Media,
    /// A settings member names a custom resource the account does not hold; the word is
    /// the family that member reads, which a refusal names.
    NotHeld(&'static str),
    /// Both at once. Either precondition is enough to refuse the start, and the media is
    /// the one the service takes on FIRST — it is resolved before the account's own
    /// records are consulted at all — so this is answered about the media, and no record
    /// exists under the name either way.
    MediaAndNotHeld,
}

impl Unmet {
    /// The refusal wording that satisfies this row.
    fn expected_message(&self) -> String {
        match self {
            Self::Media | Self::MediaAndNotHeld => MEDIA_NAMES_NO_OBJECT.to_string(),
            Self::NotHeld(kind) => not_found_message(kind),
        }
    }
}

/// One row: a start request AWS refuses, and the same request with its precondition met.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// A job name of the modelled pattern (`^[0-9a-zA-Z._-]+$`), one per row.
    name: &'static str,
    /// The `Media` of the refused request.
    media: fn() -> Value,
    /// The `Media` of the same request once the media is there — the member the row
    /// names its input audio in, pointed at a staged object.
    served_media: fn() -> Value,
    /// The `Settings` both halves carry, or `Value::Null` for a start carrying none.
    settings: fn() -> Value,
    unmet: Unmet,
}

/// `Media.MediaFileUri` naming the first staged object.
fn staged_media() -> Value {
    json!({"MediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[0])})
}

/// `Media.RedactedMediaFileUri` naming the first staged object — the member this
/// operation accepts INSTEAD of `MediaFileUri` for its input audio.
fn staged_redacted_media() -> Value {
    json!({"RedactedMediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[0])})
}

/// The table: one row per precondition a Call Analytics start can fail to meet.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "media in a bucket the object store does not hold",
            name: "absent-bucket",
            media: || json!({"MediaFileUri": format!("s3://{ABSENT_BUCKET}/call.wav")}),
            served_media: staged_media,
            settings: || Value::Null,
            unmet: Unmet::Media,
        },
        Row {
            case: "media at a key nothing was ever written to",
            name: "absent-key",
            media: || json!({"MediaFileUri": format!("s3://{BUCKET}/never-uploaded.wav")}),
            served_media: staged_media,
            settings: || Value::Null,
            unmet: Unmet::Media,
        },
        Row {
            case: "input audio named in RedactedMediaFileUri at an absent key",
            name: "absent-redacted-key",
            media: || {
                json!({"RedactedMediaFileUri": format!("s3://{BUCKET}/never-uploaded-redacted.wav")})
            },
            served_media: staged_redacted_media,
            settings: || Value::Null,
            unmet: Unmet::Media,
        },
        Row {
            case: "Settings.VocabularyName naming a custom vocabulary nothing created",
            name: "unheld-vocabulary",
            media: staged_media,
            served_media: staged_media,
            settings: || json!({"VocabularyName": VOCABULARY}),
            unmet: Unmet::NotHeld("vocabulary"),
        },
        Row {
            // `VocabularyFilterMethod` is co-required with the filter name, so the row
            // carries it: what is unmet here is the filter's existence, not that rule.
            case: "Settings.VocabularyFilterName naming a filter nothing created",
            name: "unheld-filter",
            media: staged_media,
            served_media: staged_media,
            settings: || {
                json!({
                    "VocabularyFilterName": VOCABULARY_FILTER,
                    "VocabularyFilterMethod": "mask"
                })
            },
            unmet: Unmet::NotHeld("vocabulary filter"),
        },
        Row {
            case: "Settings.LanguageModelName naming a model nothing created",
            name: "unheld-language-model",
            media: staged_media,
            served_media: staged_media,
            settings: || json!({"LanguageModelName": LANGUAGE_MODEL}),
            unmet: Unmet::NotHeld("language model"),
        },
        Row {
            case: "a per-language Settings.LanguageIdSettings entry naming a custom \
                   vocabulary nothing created",
            name: "unheld-per-language-vocabulary",
            media: staged_media,
            served_media: staged_media,
            settings: || {
                json!({
                    "LanguageOptions": ["en-US", "es-US"],
                    "LanguageIdSettings": {"en-US": {"VocabularyName": VOCABULARY}}
                })
            },
            unmet: Unmet::NotHeld("vocabulary"),
        },
        Row {
            case: "media at an absent key beside a custom vocabulary nothing created",
            name: "absent-media-and-vocabulary",
            media: || json!({"MediaFileUri": format!("s3://{BUCKET}/no-such-call.wav")}),
            served_media: staged_media,
            settings: || json!({"VocabularyName": VOCABULARY}),
            unmet: Unmet::MediaAndNotHeld,
        },
    ]
}

/// A `StartCallAnalyticsJob` request for `name` over `media`, carrying `settings` when
/// the row supplies any.
fn start_request(name: &str, media: Value, settings: Value) -> Value {
    let mut request = json!({"CallAnalyticsJobName": name, "Media": media});
    if !settings.is_null() {
        request["Settings"] = settings;
    }
    request
}

/// The job names `ListCallAnalyticsJobs` reported, for a page that may carry no
/// summaries member at all when the account holds no job.
fn listed_names(summaries: &Value) -> Vec<String> {
    if summaries.is_null() {
        return Vec::new();
    }
    names_of(summaries)
}

#[tokio::test]
async fn a_start_whose_preconditions_are_unmet_is_refused_and_stores_nothing() {
    let transcribe = emulator_with_staged_media().await;

    for row in rows() {
        let case = row.case;
        let request = start_request(row.name, (row.media)(), (row.settings)());
        let refused = transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        let expected = row.unmet.expected_message();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
        // Refused means NOTHING WAS STORED, not merely that this answer carried no job:
        // the read of that name reports no such job.
        get(&transcribe, row.name)
            .await
            .refused(case, 400, "BadRequestException");
    }

    // And no refused start reached the family's listing.
    let after_the_refusals = "ListCallAnalyticsJobs after the refusals";
    let listed = list(&transcribe, json!({"MaxResults": 100})).await;
    let names = listed_names(&listed.ok(after_the_refusals)["CallAnalyticsJobSummaries"]);
    assert!(
        names.is_empty(),
        "{after_the_refusals}: a refused start must reach no listing, got {names:?}"
    );

    // Nor is there a record for the clock to finish: past the transition the listing is
    // still empty and every name still reads as no such job.
    std::thread::sleep(PAST_THE_TRANSITION);
    let past_the_transition = "ListCallAnalyticsJobs past the transition";
    let settled = list(&transcribe, json!({"MaxResults": 100})).await;
    let names = listed_names(&settled.ok(past_the_transition)["CallAnalyticsJobSummaries"]);
    assert!(
        names.is_empty(),
        "{past_the_transition}: no refused start may be completed by the clock, got \
         {names:?}"
    );
    for row in rows() {
        get(&transcribe, row.name)
            .await
            .refused(row.case, 400, "BadRequestException");
    }
}

#[tokio::test]
async fn the_same_start_is_served_once_its_media_and_custom_resources_are_there() {
    let transcribe = emulator_with_staged_media().await;
    // The very resources the rows name, now held by this account.
    create_named_resources(&transcribe).await;

    for row in rows() {
        let case = row.case;
        let media = (row.served_media)();
        let settings = (row.settings)();
        let request = start_request(row.name, media.clone(), settings.clone());
        let started = transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await;
        // The name a refused start asked for was never reserved: this start is accepted
        // under it rather than colliding with a phantom job.
        let job = started.ok(case)["CallAnalyticsJob"].clone();
        assert_eq!(
            job["CallAnalyticsJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: an accepted start reports the job as in progress, got {job:?}"
        );

        // Round trip: the record reads back by name, with the media and the settings the
        // start named.
        let read = get(&transcribe, row.name).await;
        let stored = read.ok(case)["CallAnalyticsJob"].clone();
        assert_eq!(
            stored["Media"], media,
            "case {case:?}: the stored job must report the media it was started with, \
             got {stored:?}"
        );
        if settings.is_null() {
            assert!(
                stored["Settings"].is_null(),
                "case {case:?}: a start carrying no settings stores none, got {stored:?}"
            );
        } else {
            assert_eq!(
                stored["Settings"], settings,
                "case {case:?}: the stored job must report the settings it was started \
                 with, got {stored:?}"
            );
        }
    }

    // Every accepted row is one job of the family, and the clock finishes each of them.
    std::thread::sleep(PAST_THE_TRANSITION);
    let after_the_starts = "ListCallAnalyticsJobs after the accepted starts";
    let listed = list(&transcribe, json!({"MaxResults": 100})).await;
    let mut names = listed_names(&listed.ok(after_the_starts)["CallAnalyticsJobSummaries"]);
    names.sort();
    let mut expected: Vec<String> = rows().iter().map(|row| row.name.to_string()).collect();
    expected.sort();
    assert_eq!(
        names, expected,
        "{after_the_starts}: every accepted start must reach the listing, got {names:?}"
    );
    for row in rows() {
        let read = get(&transcribe, row.name).await;
        let job = read.ok(row.case)["CallAnalyticsJob"].clone();
        assert_eq!(
            job["CallAnalyticsJobStatus"],
            json!("COMPLETED"),
            "case {:?}: a job whose preconditions were met must complete, got {job:?}",
            row.case
        );
    }
}
