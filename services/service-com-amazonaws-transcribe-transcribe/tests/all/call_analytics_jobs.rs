// SPDX-License-Identifier: CC0-1.0

//! The Call Analytics job family, end to end over the wire:
//! `StartCallAnalyticsJob` -> `GetCallAnalyticsJob` -> `ListCallAnalyticsJobs`.
//!
//! The family's delete is the sibling suite `crate::call_analytics_job_delete`, which
//! reuses this one's request table and helpers rather than standing up a second set;
//! that is why they are `pub(crate)`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time.
//!
//! THE RULES UNDER TEST:
//!
//! * a Call Analytics job is a stored record, readable by name, whose status the clock
//!   moves from `IN_PROGRESS` to `COMPLETED`, at which point it reports the location
//!   its transcript would be written to — derived from the `OutputLocation` the
//!   request named, in each of the three forms that member documents;
//! * `ListCallAnalyticsJobs` reports those same records as summaries, newest first,
//!   narrowed by `Status` and by a case-insensitive `JobNameContains`, in pages of
//!   `MaxResults` (default 5) walked by a `NextToken` this service handed out.
//!
//! What a FINISHED job reports about the call that the request could not state — the
//! identified language, the media format, the sample rate and the location of each
//! transcript its redaction settings asked for — is the sibling suite
//! `crate::call_analytics_job_results`, which reuses this one's harness and helpers.
//!
//! Each test asserts that over a table spanning the request class (media member and
//! URI scheme, every documented output-location form, language options, vocabulary and
//! filter settings, content redaction, summarization, channel definitions, tag count),
//! not over one row.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for the `IN_PROGRESS -> COMPLETED` transition of a job started during
/// the test to have fallen due: one span of `crate::state::JOB_TRANSITION_DELAY`
/// (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so `tokio::time::sleep` is not
/// available here.
pub(crate) const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="StartCallAnalyticsJob")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/CallAnalyticsAccess";

/// Where the custom language model [`create_named_resources`] creates is trained from:
/// the bucket, the prefix the create names, and one file key under it.
///
/// `pub(crate)` because a suite that mounts an object store beside Transcribe has to
/// STAGE that data before calling the helper: a model's training settles on what the
/// store answers about this location, and only a model that settled `COMPLETED` is one a
/// job may apply.
pub(crate) const MODEL_TRAINING_BUCKET: &str = "doc-example-bucket";
/// See [`MODEL_TRAINING_BUCKET`].
pub(crate) const MODEL_TRAINING_PREFIX: &str = "training/";
/// See [`MODEL_TRAINING_BUCKET`].
pub(crate) const MODEL_TRAINING_KEY: &str = "training/corpus.txt";

/// The members `StartCallAnalyticsJob` accepts that the `CallAnalyticsJob` shape also
/// carries, so a stored job must read them back unchanged
/// (`smithy_query(query="shape", shape="CallAnalyticsJob")`).
const ECHOED_MEMBERS: &[&str] = &[
    "CallAnalyticsJobName",
    "Media",
    "Settings",
    "DataAccessRoleArn",
    "ChannelDefinitions",
    "Tags",
];

/// The class of job a `StartCallAnalyticsJob` request can describe, one row per axis
/// the service's rules generalise over: which `Media` member names the input audio and
/// under which URI scheme, whether an output location is named, how many languages the
/// request leaves identification to choose between, which custom vocabulary / filter /
/// language model is applied, whether content is redacted or summarized, whether the
/// speakers are pinned to channels, and how many tags are attached.
pub(crate) fn start_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "minimal: s3 media, no output location, no settings",
            json!({
                "CallAnalyticsJobName": "minimal-call",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"}
            }),
        ),
        (
            "redacted input audio, https scheme, output location, one language option, \
             vocabulary filter, two tags",
            json!({
                "CallAnalyticsJobName": "redacted.call_1",
                "Media": {"RedactedMediaFileUri": "https://example.invalid/media/call.mp4"},
                "OutputLocation": "s3://example-output/analytics/",
                "DataAccessRoleArn": ROLE_ARN,
                "OutputEncryptionKMSKeyId":
                    "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "Settings": {
                    "VocabularyName": "call-terms",
                    "VocabularyFilterName": "blocked-terms",
                    "VocabularyFilterMethod": "mask",
                    "LanguageOptions": ["en-US"]
                },
                "Tags": [{"Key": "team", "Value": "support"}, {"Key": "cost", "Value": "7"}]
            }),
        ),
        (
            "both media members, several language options, content redaction, \
             summarization, custom language model",
            json!({
                "CallAnalyticsJobName": "redaction_call-2",
                "Media": {
                    "MediaFileUri": "s3://example-media/call.wav",
                    "RedactedMediaFileUri": "s3://example-media/call-redacted.wav"
                },
                "OutputLocation": "s3://example-output/analytics/call-2.json",
                "DataAccessRoleArn": "arn:aws-us-gov:iam::111122223333:role/support/Analytics",
                "Settings": {
                    "LanguageModelName": "support-model",
                    "LanguageOptions": ["en-US", "es-US"],
                    // `RedactionType` and `RedactionOutput` are both required members
                    // of `ContentRedaction`
                    // (`smithy_query(query="shape", shape="ContentRedaction")`).
                    "ContentRedaction": {
                        "RedactionType": "PII",
                        "RedactionOutput": "redacted_and_unredacted",
                        "PiiEntityTypes": ["BANK_ACCOUNT_NUMBER", "NAME"]
                    },
                    "Summarization": {"GenerateAbstractiveSummary": true}
                },
                "Tags": [{"Key": "team", "Value": "support"}]
            }),
        ),
        (
            "channel definitions pinning each participant to a channel",
            json!({
                "CallAnalyticsJobName": "channels-3",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.ogg"},
                "OutputLocation": "s3://example-output",
                "DataAccessRoleArn": ROLE_ARN,
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "AGENT"},
                    {"ChannelId": 1, "ParticipantRole": "CUSTOMER"}
                ]
            }),
        ),
    ]
}

/// The job name of a start request.
pub(crate) fn job_name(request: &Value) -> String {
    request["CallAnalyticsJobName"]
        .as_str()
        .expect("every row names its job")
        .to_string()
}

/// The language a FINISHED job must report: the first code
/// `Settings.LanguageOptions` lists, since identification may only answer with one of
/// the codes the request allowed, and the service's default for a request that
/// narrowed the choice to nothing.
///
/// The sibling suite `crate::call_analytics_job_results` asserts that rule over a
/// table of its own; here it is what the lifecycle and listing assertions expect.
pub(crate) fn expected_language(request: &Value) -> Value {
    match request["Settings"]["LanguageOptions"].as_array() {
        Some(options) if !options.is_empty() => options[0].clone(),
        _ => json!(service_com_amazonaws_transcribe_transcribe::state::DEFAULT_IDENTIFIED_LANGUAGE),
    }
}

/// Whether a request asked for a REDACTED transcript: through `ContentRedaction`, or
/// through the `Media.RedactedMediaFileUri` that "produces a redacted audio file in
/// addition to a redacted transcript".
pub(crate) fn expects_redacted_transcript(request: &Value) -> bool {
    request["Settings"]["ContentRedaction"].is_object()
        || request["Media"]["RedactedMediaFileUri"].is_string()
}

/// Whether it asked for the UNREDACTED transcript too: everything except
/// `RedactionOutput: redacted`, for which "Amazon Transcribe creates only a redacted
/// transcript".
pub(crate) fn expects_unredacted_transcript(request: &Value) -> bool {
    request["Settings"]["ContentRedaction"]["RedactionOutput"] != json!("redacted")
}

/// `GetCallAnalyticsJob` for `name`.
pub(crate) async fn get(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call("GetCallAnalyticsJob", json!({"CallAnalyticsJobName": name}))
        .await
}

/// Create the custom resources the rows of [`start_requests`] NAME, so each row
/// applies a custom vocabulary, vocabulary filter and language model this account
/// holds.
///
/// A start may only name custom resources that exist — "The name of the custom
/// vocabulary you want to include in your Call Analytics transcription request", and
/// the same sentence for the filter and the model
/// (`smithy_query(query="shape", shape="CallAnalyticsJobSettings")`) — so a suite that
/// starts a job with settings creates what those settings name first.
pub(crate) async fn create_named_resources(transcribe: &Transcribe) {
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "call-terms",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
        )
        .await
        .ok("CreateVocabulary call-terms");
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
    transcribe
        .call(
            "CreateLanguageModel",
            json!({
                "ModelName": "support-model",
                "LanguageCode": "en-US",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": format!("s3://{MODEL_TRAINING_BUCKET}/{MODEL_TRAINING_PREFIX}"),
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        )
        .await
        .ok("CreateLanguageModel support-model");
    // A start may only apply a custom vocabulary once its build has finished: it is
    // READY that makes one usable, and a create leaves it PENDING. A custom language
    // model is the same rule on its own member: COMPLETED is what makes one usable, and
    // a create leaves it IN_PROGRESS.
    transcribe.vocabulary_ready("call-terms", None).await;
    transcribe
        .language_model_completed("support-model", None)
        .await;
}

/// `ListCallAnalyticsJobs` with `request`.
pub(crate) async fn list(transcribe: &Transcribe, request: Value) -> Reply {
    transcribe.call("ListCallAnalyticsJobs", request).await
}

/// The job names of a listing's summaries, in the order the page reported them.
pub(crate) fn names_of(summaries: &Value) -> Vec<String> {
    summaries
        .as_array()
        .expect("CallAnalyticsJobSummaries must be a list")
        .iter()
        .map(|summary| {
            summary["CallAnalyticsJobName"]
                .as_str()
                .expect("every summary names its job")
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_started_call_analytics_job_reads_back_exactly_as_it_was_started() {
    let transcribe = Transcribe::new();
    create_named_resources(&transcribe).await;

    for (case, request) in start_requests() {
        let started = transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await;
        let job = started.ok(case)["CallAnalyticsJob"].clone();

        assert_eq!(
            job["CallAnalyticsJobStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a job just started must be IN_PROGRESS, got {:?}",
            job["CallAnalyticsJobStatus"]
        );
        assert!(
            job["CreationTime"].is_number() && job["StartTime"].is_number(),
            "case {case:?}: CreationTime and StartTime must be set, got {:?} and {:?}",
            job["CreationTime"],
            job["StartTime"]
        );
        assert!(
            job["CompletionTime"].is_null() && job["Transcript"].is_null(),
            "case {case:?}: a job still processing has no completion time and no \
             transcript, got {:?} and {:?}",
            job["CompletionTime"],
            job["Transcript"]
        );
        assert!(
            job["FailureReason"].is_null(),
            "case {case:?}: an accepted job has no FailureReason, got {:?}",
            job["FailureReason"]
        );
        // The language, the media format and the sample rate are not members of the
        // start request: the service determines each of them while it processes the
        // call, so a job that has not finished processing reports none of them (what
        // a finished one reports is the sibling suite
        // `crate::call_analytics_job_results`).
        for member in [
            "LanguageCode",
            "IdentifiedLanguageScore",
            "MediaFormat",
            "MediaSampleRateHertz",
        ] {
            assert!(
                job[member].is_null(),
                "case {case:?}: a job still processing has determined no {member}, \
                 got {:?}",
                job[member]
            );
        }

        let name = job_name(&request);
        let fetched = get(&transcribe, &name).await;
        let fetched = fetched.ok(case)["CallAnalyticsJob"].clone();

        for member in ECHOED_MEMBERS {
            if let Some(sent) = request.get(*member) {
                assert_eq!(
                    fetched.get(*member),
                    Some(sent),
                    "case {case:?}: GetCallAnalyticsJob must return the stored {member}, \
                     got {:?}",
                    fetched.get(*member)
                );
            }
        }
        // A job carrying no tags reports no `Tags` at all: `TagList` is modelled
        // `length {min 1, max 200}`, so an empty list is not a value of that shape.
        if request.get("Tags").is_none() {
            assert!(
                fetched["Tags"].is_null(),
                "case {case:?}: a job started with no tags must report no Tags, got {:?}",
                fetched["Tags"]
            );
        }
        assert_eq!(
            fetched["CreationTime"], job["CreationTime"],
            "case {case:?}: CreationTime must be stable across reads, got {:?} then {:?}",
            job["CreationTime"], fetched["CreationTime"]
        );
    }
}

#[tokio::test]
async fn a_call_analytics_job_completes_once_its_processing_span_has_elapsed() {
    let transcribe = Transcribe::new();
    create_named_resources(&transcribe).await;

    for (case, request) in start_requests() {
        transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await
            .ok(case);
    }

    // One wait for every row, not one per row.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in start_requests() {
        let name = job_name(&request);
        let fetched = get(&transcribe, &name).await;
        let job = fetched.ok(case)["CallAnalyticsJob"].clone();

        assert_eq!(
            job["CallAnalyticsJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: the processing span has elapsed, so the job must be \
             COMPLETED, got {:?}",
            job["CallAnalyticsJobStatus"]
        );
        assert!(
            job["CompletionTime"].is_number(),
            "case {case:?}: a completed job reports a CompletionTime, got {:?}",
            job["CompletionTime"]
        );
        assert_eq!(
            job["Transcript"]["TranscriptFileUri"].is_string(),
            expects_unredacted_transcript(&request),
            "case {case:?}: an unredacted transcript location is reported exactly where \
             the request asked for an unredacted transcript, got {:?}",
            job["Transcript"]
        );
        assert_eq!(
            job["Transcript"]["RedactedTranscriptFileUri"].is_string(),
            expects_redacted_transcript(&request),
            "case {case:?}: a redacted transcript location is reported exactly where the \
             request asked for redaction, got {:?}",
            job["Transcript"]
        );

        // A read does not change what a later read sees.
        let again = get(&transcribe, &name).await;
        let again = again.ok(case)["CallAnalyticsJob"].clone();
        assert_eq!(
            again, job,
            "case {case:?}: a second read must report the same job, got {again:?}"
        );

        let listed = list(&transcribe, json!({"MaxResults": 100})).await;
        let summaries = listed.ok(case)["CallAnalyticsJobSummaries"].clone();
        let summary = summaries
            .as_array()
            .expect("CallAnalyticsJobSummaries must be a list")
            .iter()
            .find(|summary| summary["CallAnalyticsJobName"] == json!(name.clone()))
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the job must appear in the listing"));
        assert_eq!(
            summary,
            json!({
                "CallAnalyticsJobName": name,
                "CallAnalyticsJobStatus": "COMPLETED",
                "CreationTime": job["CreationTime"].clone(),
                "StartTime": job["StartTime"].clone(),
                "CompletionTime": job["CompletionTime"].clone(),
                // The summary's LanguageCode follows the same rule as the job's.
                "LanguageCode": expected_language(&request)
            })
            .as_object()
            .map(|members| {
                // A member the job does not report is absent from the summary too,
                // rather than present and null.
                let mut members = members.clone();
                members.retain(|_, value| !value.is_null());
                Value::Object(members)
            })
            .expect("an object"),
            "case {case:?}: unexpected summary, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn the_transcript_location_follows_the_documented_output_location_rule() {
    let transcribe = Transcribe::new();
    let account = engine::emulator_account_id();

    // The three forms `StartCallAnalyticsJobRequest$OutputLocation` documents
    // (`s3://BUCKET`, `s3://BUCKET/my-output-folder/`,
    // `s3://BUCKET/my-output-folder/my-call-analytics-job.json`), a folder spelled
    // without its trailing slash, and the service-managed location a request that
    // names none gets.
    let rows: Vec<(&str, Option<&str>, String)> = vec![
        (
            "no output location: the service-managed location",
            None,
            format!(
                "https://s3.us-east-1.amazonaws.com/aws-transcribe-us-east-1-prod/\
                 {account}/output-none.json"
            ),
        ),
        (
            "a bucket alone",
            Some("s3://example-output"),
            "s3://example-output/output-bucket.json".to_string(),
        ),
        (
            "a folder with its trailing slash",
            Some("s3://example-output/analytics/"),
            "s3://example-output/analytics/output-folder.json".to_string(),
        ),
        (
            "a folder without a trailing slash",
            Some("s3://example-output/analytics"),
            "s3://example-output/analytics/output-bare-folder.json".to_string(),
        ),
        (
            "a location naming the output file itself",
            Some("s3://example-output/analytics/chosen-name.json"),
            "s3://example-output/analytics/chosen-name.json".to_string(),
        ),
    ];
    let names = [
        "output-none",
        "output-bucket",
        "output-folder",
        "output-bare-folder",
        "output-file",
    ];

    for ((case, location, _), name) in rows.iter().zip(names) {
        let mut request = json!({
            "CallAnalyticsJobName": name,
            "Media": {"MediaFileUri": "s3://example-media/call.flac"}
        });
        if let Some(location) = location {
            request["OutputLocation"] = json!(location);
        }
        transcribe
            .call("StartCallAnalyticsJob", request)
            .await
            .ok(case);
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    for ((case, _, expected), name) in rows.iter().zip(names) {
        let fetched = get(&transcribe, name).await;
        let job = fetched.ok(case)["CallAnalyticsJob"].clone();
        assert_eq!(
            job["Transcript"]["TranscriptFileUri"],
            json!(expected),
            "case {case:?}: expected the transcript at {expected}, got {:?}",
            job["Transcript"]["TranscriptFileUri"]
        );
    }
}

#[tokio::test]
async fn the_listing_is_every_stored_job_newest_first_paged_and_filtered() {
    let transcribe = Transcribe::new();

    // Eleven jobs, more than the documented default page of 5, in two name families
    // of different case so `JobNameContains` has something to narrow and something to
    // match case-insensitively.
    let mut created: Vec<String> = Vec::new();
    for index in 0..11 {
        let name = if index % 2 == 0 {
            format!("alpha-{index}")
        } else {
            format!("BETA-{index}")
        };
        transcribe
            .call(
                "StartCallAnalyticsJob",
                json!({
                    "CallAnalyticsJobName": name.clone(),
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"}
                }),
            )
            .await
            .ok(&name);
        created.push(name);
    }

    // Newest first: the reverse of the creation order.
    let newest_first: Vec<String> = created.iter().rev().cloned().collect();
    let all = list(&transcribe, json!({"MaxResults": 100})).await;
    assert_eq!(
        names_of(&all.ok("unpaged listing")["CallAnalyticsJobSummaries"]),
        newest_first,
        "the listing must be newest first, got {:?}",
        all.body["CallAnalyticsJobSummaries"]
    );

    // Every page size pages through exactly the same sequence, with no job repeated
    // and none missed. 1 and 100 are the published bounds of `MaxResults`
    // (`@range(min: 1, max: 100)`).
    for page_size in [1_u64, 2, 3, 7, 100] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"MaxResults": page_size});
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = list(&transcribe, request).await;
            let body = page.ok(&case);
            let page_names = names_of(&body["CallAnalyticsJobSummaries"]);
            assert!(
                page_names.len() as u64 <= page_size,
                "case {case:?}: a page must hold at most {page_size} jobs, got {page_names:?}"
            );
            seen.extend(page_names);
            token = body["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                seen.len() <= newest_first.len(),
                "case {case:?}: paging must terminate, got {seen:?}"
            );
        }
        assert_eq!(
            seen, newest_first,
            "case {case:?}: paging must walk every job once, newest first, got {seen:?}"
        );
    }

    // An unfiltered listing with no MaxResults returns the documented default page of
    // 5, with a cursor to the rest.
    let defaulted = list(&transcribe, json!({})).await;
    let body = defaulted.ok("default page size");
    assert_eq!(
        names_of(&body["CallAnalyticsJobSummaries"]).len(),
        5,
        "the documented default page size is 5, got {:?}",
        body["CallAnalyticsJobSummaries"]
    );
    assert!(
        body["NextToken"].is_string(),
        "a truncated page must carry a NextToken, got {:?}",
        body["NextToken"]
    );

    // `JobNameContains` is a case-insensitive substring search.
    let beta: Vec<String> = created
        .iter()
        .rev()
        .filter(|name| name.starts_with("BETA"))
        .cloned()
        .collect();
    let contains_cases: &[(&str, Vec<String>)] = &[
        ("beta", beta.clone()),
        ("BETA", beta.clone()),
        ("Beta", beta),
        ("alpha-4", vec!["alpha-4".to_string()]),
        ("zeta", Vec::new()),
    ];
    for (needle, expected) in contains_cases {
        let filtered = list(
            &transcribe,
            json!({"JobNameContains": needle, "MaxResults": 100}),
        )
        .await;
        assert_eq!(
            &names_of(&filtered.ok(needle)["CallAnalyticsJobSummaries"]),
            expected,
            "case {needle:?}: unexpected JobNameContains result, got {:?}",
            filtered.body["CallAnalyticsJobSummaries"]
        );
    }

    // The `Status` filter selects only jobs in that status, is echoed back, and the
    // four modelled statuses together still cover every job. Waiting past the
    // transition first keeps the partition from being read mid-transition, and the
    // assertion is on the UNION of the names rather than on a sum, so it cannot depend
    // on where the clock is between two of the calls.
    std::thread::sleep(PAST_THE_TRANSITION);
    let mut covered: Vec<String> = Vec::new();
    // Every value of `CallAnalyticsJobStatus`
    // (`smithy_query(query="shape", shape="CallAnalyticsJobStatus")`).
    for status in ["QUEUED", "IN_PROGRESS", "COMPLETED", "FAILED"] {
        let filtered = list(&transcribe, json!({"Status": status, "MaxResults": 100})).await;
        let body = filtered.ok(status);
        assert_eq!(
            body["Status"],
            json!(status),
            "case {status:?}: the Status filter must be echoed back, got {:?}",
            body["Status"]
        );
        for summary in body["CallAnalyticsJobSummaries"]
            .as_array()
            .expect("CallAnalyticsJobSummaries must be a list")
        {
            assert_eq!(
                summary["CallAnalyticsJobStatus"],
                json!(status),
                "case {status:?}: the filter must return only jobs in that status, \
                 got {summary:?}"
            );
        }
        covered.extend(names_of(&body["CallAnalyticsJobSummaries"]));
    }
    covered.sort();
    covered.dedup();
    let mut expected_names = newest_first.clone();
    expected_names.sort();
    assert_eq!(
        covered, expected_names,
        "the four statuses together must cover every job, got {covered:?}"
    );

    // The two filters compose, and so do a filter and a page.
    let composed = list(
        &transcribe,
        json!({"JobNameContains": "ALPHA", "Status": "COMPLETED", "MaxResults": 2}),
    )
    .await;
    let body = composed.ok("JobNameContains and Status together, paged");
    let page = names_of(&body["CallAnalyticsJobSummaries"]);
    assert!(
        page.len() == 2 && page.iter().all(|name| name.starts_with("alpha")),
        "a composed filtered page must hold two alpha jobs, got {page:?}"
    );
    assert!(
        body["NextToken"].is_string(),
        "a truncated filtered page must carry a NextToken, got {:?}",
        body["NextToken"]
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_hand_out_is_refused() {
    let transcribe = Transcribe::new();

    // Neither an unparseable cursor nor a well-formed one naming a sequence that
    // cannot exist (creation sequences start at 1) is accepted: silently listing from
    // the top would hide a caller's paging bug.
    for token in ["not-a-cursor", "0|demo-call", "|", "-1|demo-call"] {
        let refused = list(&transcribe, json!({"NextToken": token})).await;
        refused.refused(token, 400, "BadRequestException");
    }

    // A token this service DID hand out is accepted, even for a listing that has
    // since been emptied of nothing: the cursor is the stored creation sequence.
    for index in 0..6 {
        let name = format!("token-{index}");
        transcribe
            .call(
                "StartCallAnalyticsJob",
                json!({
                    "CallAnalyticsJobName": name.clone(),
                    "Media": {"MediaFileUri": "s3://example-media/call.flac"}
                }),
            )
            .await
            .ok(&name);
    }
    let first = list(&transcribe, json!({})).await;
    let token = first
        .ok("first page")["NextToken"]
        .as_str()
        .expect("a truncated page carries a NextToken")
        .to_string();
    let second = list(&transcribe, json!({"NextToken": token})).await;
    assert_eq!(
        names_of(&second.ok("second page")["CallAnalyticsJobSummaries"]),
        vec!["token-0".to_string()],
        "the second page must hold the one job the first page left, got {:?}",
        second.body["CallAnalyticsJobSummaries"]
    );
}

#[tokio::test]
async fn a_call_analytics_job_this_account_never_started_is_refused() {
    let transcribe = Transcribe::new();

    // A job of ANOTHER family under the same name is not a Call Analytics job: each
    // family is keyed on its own store.
    transcribe
        .call(
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "other-family",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"}
            }),
        )
        .await
        .ok("a transcription job of the same name");

    // The name forms `CallAnalyticsJobName`'s pattern allows (`^[0-9a-zA-Z._-]+$`),
    // including both published length bounds, none of them ever started here.
    for name in [
        "never-started",
        "a",
        "never.started_2",
        "9",
        "other-family",
        "never-started",
        "a",
        "never.started_2",
        "9",
        &"n".repeat(200),
    ] {
        let reply = get(&transcribe, name).await;
        reply.refused(name, 400, "BadRequestException");
        // The service's own not-found text names the KIND of resource and never
        // echoes the name the caller asked about.
        let message = reply.error_message(name);
        assert_eq!(
            message,
            "The requested job couldn't be found. Check the job name and try your \
             request again.",
            "case {name:?}: unexpected not-found message, got {message:?}"
        );
    }

    // One past the published name length is refused by the boundary validator, before
    // the handler runs: `CallAnalyticsJobName` is `length {min 1, max 200}`.
    get(&transcribe, &"n".repeat(201))
        .await
        .refused("a 201-character job name", 400, "ValidationException");

    // An empty store lists an empty page and no cursor, rather than refusing.
    let listed = list(&transcribe, json!({"MaxResults": 100})).await;
    let body = listed.ok("empty listing");
    assert_eq!(
        body["CallAnalyticsJobSummaries"],
        json!([]),
        "an account that started no job lists nothing, got {:?}",
        body["CallAnalyticsJobSummaries"]
    );
    assert!(
        body["NextToken"].is_null(),
        "a listing that is not truncated carries no NextToken, got {:?}",
        body["NextToken"]
    );
}

#[tokio::test]
async fn a_start_whose_members_cannot_hold_together_is_refused() {
    let transcribe = Transcribe::new();

    // One row per combination the operation documents but the model's constraint
    // traits cannot express.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a Media that names neither MediaFileUri nor RedactedMediaFileUri",
            json!({
                "CallAnalyticsJobName": "refused-1",
                "Media": {}
            }),
        ),
        (
            "a vocabulary filter without its filter method",
            json!({
                "CallAnalyticsJobName": "refused-2",
                "Media": {"MediaFileUri": "s3://example-media/call.flac"},
                "Settings": {"VocabularyFilterName": "blocked-terms"}
            }),
        ),
        (
            "two channel definitions naming the same channel",
            json!({
                "CallAnalyticsJobName": "refused-3",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "AGENT"},
                    {"ChannelId": 0, "ParticipantRole": "CUSTOMER"}
                ]
            }),
        ),
        (
            "two channel definitions naming the same participant role",
            json!({
                "CallAnalyticsJobName": "refused-4",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "AGENT"},
                    {"ChannelId": 1, "ParticipantRole": "AGENT"}
                ]
            }),
        ),
    ];

    for (case, request) in &rows {
        let refused = transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await;
        refused.refused(case, 400, "BadRequestException");

        // A refused start stores nothing, so the name it asked for is unknown.
        get(&transcribe, &job_name(request))
            .await
            .refused(case, 400, "BadRequestException");
    }

    // A member outside a constraint the model DOES express is refused by the boundary
    // validator, before the handler runs: `ChannelDefinitions` is
    // `@length(min: 2, max: 2)` and `ChannelId` is `@range(min: 0, max: 1)`.
    let validated: Vec<(&str, Value)> = vec![
        (
            "one channel definition where the model requires two",
            json!({
                "CallAnalyticsJobName": "refused-length",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "ChannelDefinitions": [{"ChannelId": 0, "ParticipantRole": "AGENT"}]
            }),
        ),
        (
            "a channel id past the published range",
            json!({
                "CallAnalyticsJobName": "refused-range",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "AGENT"},
                    {"ChannelId": 2, "ParticipantRole": "CUSTOMER"}
                ]
            }),
        ),
    ];
    for (case, request) in &validated {
        transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await
            .refused(case, 400, "ValidationException");
    }

    // Nothing any refused start asked for reached the store.
    let listed = list(&transcribe, json!({"MaxResults": 100})).await;
    assert_eq!(
        listed.ok("list after refusals")["CallAnalyticsJobSummaries"],
        json!([]),
        "a refused StartCallAnalyticsJob must store nothing, got {:?}",
        listed.body
    );

    // `ChannelDefinition$ParticipantRole` is OPTIONAL, so two definitions that name
    // no role at all have not named one twice: the distinctness rule applies to the
    // roles a request actually names, and a start that names none is accepted.
    let case = "two channel definitions that name no participant role";
    let accepted = transcribe
        .call(
            "StartCallAnalyticsJob",
            json!({
                "CallAnalyticsJobName": "roles-omitted",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "ChannelDefinitions": [{"ChannelId": 0}, {"ChannelId": 1}]
            }),
        )
        .await;
    assert_eq!(
        accepted.ok(case)["CallAnalyticsJob"]["CallAnalyticsJobStatus"],
        json!("IN_PROGRESS"),
        "case {case:?}: the start must be accepted, got {:?}",
        accepted.body
    );
}

#[tokio::test]
async fn a_second_start_under_a_live_job_name_is_a_conflict() {
    let transcribe = Transcribe::new();
    create_named_resources(&transcribe).await;

    for (case, request) in start_requests() {
        transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await
            .ok(case);

        // "must be unique within an Amazon Web Services account. If you try to create
        // a new job with the same name as an existing job, you get a
        // ConflictException error."
        let conflict = transcribe
            .call("StartCallAnalyticsJob", request.clone())
            .await;
        conflict.refused(case, 400, "ConflictException");

        // The first job is untouched by the refused second start, and the listing
        // still holds exactly one record for the name.
        let fetched = get(&transcribe, &job_name(&request)).await;
        assert_eq!(
            fetched.ok(case)["CallAnalyticsJob"]["Media"],
            request["Media"],
            "case {case:?}: a refused start must not rewrite the stored job, got {:?}",
            fetched.body
        );
        let listed = list(
            &transcribe,
            json!({"JobNameContains": job_name(&request), "MaxResults": 100}),
        )
        .await;
        assert_eq!(
            names_of(&listed.ok(case)["CallAnalyticsJobSummaries"]),
            vec![job_name(&request)],
            "case {case:?}: the name must hold exactly one record, got {:?}",
            listed.body["CallAnalyticsJobSummaries"]
        );
    }
}
