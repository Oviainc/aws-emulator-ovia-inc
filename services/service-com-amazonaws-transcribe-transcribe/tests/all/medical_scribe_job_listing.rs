// SPDX-License-Identifier: CC0-1.0

//! `ListMedicalScribeJobs`, end to end over the wire: `StartMedicalScribeJob` ->
//! `ListMedicalScribeJobs` -> `GetMedicalScribeJob` -> `DeleteMedicalScribeJob`.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.
//!
//! THE RULE UNDER TEST: `ListMedicalScribeJobs` reports the Medical Scribe (AWS
//! HealthScribe) jobs this account has stored — and only those — as summaries
//! projected from each stored record, newest first, narrowed by `Status` (echoed back)
//! and by a case-insensitive `JobNameContains` substring search, in pages of
//! `MaxResults` (default 5) walked by a `NextToken` this service handed out, and
//! refusing any other cursor. Each test asserts that over a table or a set of inputs
//! spanning what the rule generalises over (page size, job count, name case, status,
//! and the request members a summary carries), not over one row.

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
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/HealthScribeAccess";

/// The class of job a `StartMedicalScribeJob` request can describe, one row per axis
/// the summary projection generalises over: the media URI scheme, which of the two
/// speaker-partitioning modes is chosen, each published bound of `MaxSpeakerLabels`
/// (`@range(min: 2, max: 30)`), whether a custom vocabulary and filter are applied,
/// whether the output is KMS-encrypted, how many tags are attached, whether a patient
/// context is supplied, and the name forms `TranscriptionJobName`'s pattern allows
/// (`^[0-9a-zA-Z._-]+$`).
fn start_requests() -> Vec<(&'static str, Value)> {
    vec![
        (
            "speaker labels at the lowest published MaxSpeakerLabels, no tags, no context",
            json!({
                "MedicalScribeJobName": "listing-minimal",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
            }),
        ),
        (
            "speaker labels at the highest published MaxSpeakerLabels, vocabulary filter, \
             KMS encryption, two tags, patient context",
            json!({
                "MedicalScribeJobName": "listing.job_2",
                "Media": {"MediaFileUri": "https://example.invalid/media/visit.mp4"},
                "OutputBucketName": "example-output.bucket",
                "DataAccessRoleArn": "arn:aws-us-gov:iam::111122223333:role/clinic/Scribe",
                "OutputEncryptionKMSKeyId":
                    "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab",
                "KMSEncryptionContext": {"department": "cardiology"},
                "Settings": {
                    "ShowSpeakerLabels": true,
                    "MaxSpeakerLabels": 30,
                    "VocabularyName": "clinic-terms",
                    "VocabularyFilterName": "blocked-terms",
                    "VocabularyFilterMethod": "mask"
                },
                "Tags": [{"Key": "clinic", "Value": "cardiology"}, {"Key": "cost", "Value": "7"}],
                "MedicalScribeContext": {"PatientContext": {"Pronouns": "THEY_THEM"}}
            }),
        ),
        (
            "channel identification with the two channel definitions it requires",
            json!({
                "MedicalScribeJobName": "LISTING_channels-3",
                "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ChannelIdentification": true},
                "ChannelDefinitions": [
                    {"ChannelId": 0, "ParticipantRole": "CLINICIAN"},
                    {"ChannelId": 1, "ParticipantRole": "PATIENT"}
                ],
                "Tags": [{"Key": "clinic", "Value": "primary-care"}]
            }),
        ),
        (
            "a single-character job name, the lowest published length bound",
            json!({
                "MedicalScribeJobName": "L",
                "Media": {"MediaFileUri": "s3://example-media/notes.mp3"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 11}
            }),
        ),
    ]
}

/// The smallest start request that names `name`, for the rows that vary only the job
/// NAME.
fn start_request(name: &str) -> Value {
    json!({
        "MedicalScribeJobName": name,
        "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
        "OutputBucketName": "example-scribe-output",
        "DataAccessRoleArn": ROLE_ARN,
        "Settings": {"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}
    })
}

/// Create the custom resources the rows of [`start_requests`] NAME, so each row
/// applies a custom vocabulary and vocabulary filter this account holds.
///
/// A start may only name custom resources that exist (`smithy_query(query="shape",
/// shape="MedicalScribeSettings")`), so a suite that starts a job naming them creates
/// them first.
async fn create_named_resources(transcribe: &Transcribe) {
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "clinic-terms",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark", "Transcribe"]
            }),
        )
        .await
        .ok("CreateVocabulary clinic-terms");
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
}

/// The job name of a start request.
fn job_name(request: &Value) -> String {
    request["MedicalScribeJobName"]
        .as_str()
        .expect("every row names its job")
        .to_string()
}

/// `ListMedicalScribeJobs` with `request` as its input.
async fn list(transcribe: &Transcribe, request: Value) -> Reply {
    transcribe.call("ListMedicalScribeJobs", request).await
}

/// `GetMedicalScribeJob` for `name`.
async fn get(transcribe: &Transcribe, name: &str) -> Reply {
    transcribe
        .call("GetMedicalScribeJob", json!({"MedicalScribeJobName": name}))
        .await
}

/// The summaries of a listing answer.
fn summaries_of(body: &Value) -> &Vec<Value> {
    body["MedicalScribeJobSummaries"]
        .as_array()
        .expect("MedicalScribeJobSummaries must be a list")
}

/// The job names of a listing's summaries, in the order the page reported them.
fn names_of(body: &Value) -> Vec<String> {
    summaries_of(body)
        .iter()
        .map(|summary| {
            summary["MedicalScribeJobName"]
                .as_str()
                .expect("every summary names its job")
                .to_string()
        })
        .collect()
}

/// An object with its null members dropped, so an expectation can name a member that
/// is absent for this row without asserting it is present and null.
fn without_nulls(value: Value) -> Value {
    let mut members = value.as_object().expect("an object").clone();
    members.retain(|_, value| !value.is_null());
    Value::Object(members)
}

/// Start every row of [`start_requests`], in table order, and answer their names.
async fn start_the_table(transcribe: &Transcribe) -> Vec<String> {
    let mut names = Vec::new();
    for (case, request) in start_requests() {
        transcribe
            .call("StartMedicalScribeJob", request.clone())
            .await
            .ok(case);
        names.push(job_name(&request));
    }
    names
}

#[tokio::test]
async fn the_listing_reports_each_stored_job_as_its_own_summary() {
    let transcribe = Transcribe::new();
    create_named_resources(&transcribe).await;
    start_the_table(&transcribe).await;

    // One wait for the whole table, not one per row.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in start_requests() {
        let name = job_name(&request);
        let fetched = get(&transcribe, &name).await;
        let job = fetched.ok(case)["MedicalScribeJob"].clone();

        let listed = list(&transcribe, json!({"MaxResults": 100})).await;
        let body = listed.ok(case).clone();
        let summary = summaries_of(&body)
            .iter()
            .find(|summary| summary["MedicalScribeJobName"] == json!(name.clone()))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the job must appear in the listing, got {body:?}")
            });

        // The summary is a projection of the stored record: every member
        // `MedicalScribeJobSummary` carries, and nothing else
        // (`smithy_query(query="shape", shape="MedicalScribeJobSummary")`) — so the
        // media, settings, channel definitions, output locations and tags of a job are
        // read through `GetMedicalScribeJob`, not from a listing. `en-US` is the one
        // value of `MedicalScribeLanguageCode`.
        assert_eq!(
            summary,
            without_nulls(json!({
                "MedicalScribeJobName": name.clone(),
                "CreationTime": job["CreationTime"].clone(),
                "StartTime": job["StartTime"].clone(),
                "CompletionTime": job["CompletionTime"].clone(),
                "LanguageCode": json!("en-US"),
                "MedicalScribeJobStatus": json!("COMPLETED")
            })),
            "case {case:?}: unexpected summary, got {summary:?}"
        );
        // An accepted job never fails here, so no summary reports a FailureReason.
        assert!(
            summary["FailureReason"].is_null(),
            "case {case:?}: an accepted job has no FailureReason, got {:?}",
            summary["FailureReason"]
        );
    }

    // Listing is a REPORT: a second listing answers exactly what the first did.
    let first = list(&transcribe, json!({"MaxResults": 100})).await;
    let second = list(&transcribe, json!({"MaxResults": 100})).await;
    assert_eq!(
        first.ok("first listing"),
        second.ok("second listing"),
        "a listing must not change what a later listing sees, got {:?} then {:?}",
        first.body,
        second.body
    );
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
            .call("StartMedicalScribeJob", start_request(&name))
            .await
            .ok(&name);
        created.push(name);
    }

    // Newest first: the reverse of the creation order.
    let newest_first: Vec<String> = created.iter().rev().cloned().collect();
    let all = list(&transcribe, json!({"MaxResults": 100})).await;
    assert_eq!(
        names_of(all.ok("unpaged listing")),
        newest_first,
        "the listing must be newest first, got {:?}",
        all.body["MedicalScribeJobSummaries"]
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
            let page_names = names_of(body);
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
        names_of(body).len(),
        5,
        "the documented default page size is 5, got {:?}",
        body["MedicalScribeJobSummaries"]
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
        ("-", newest_first.clone()),
        ("zeta", Vec::new()),
    ];
    for (needle, expected) in contains_cases {
        let filtered = list(
            &transcribe,
            json!({"JobNameContains": needle, "MaxResults": 100}),
        )
        .await;
        assert_eq!(
            &names_of(filtered.ok(needle)),
            expected,
            "case {needle:?}: unexpected JobNameContains result, got {:?}",
            filtered.body["MedicalScribeJobSummaries"]
        );
    }

    // The `Status` filter selects only jobs in that status, is echoed back, and the
    // four modelled statuses together still cover every job. Waiting past the
    // transition first keeps the partition from being read mid-transition, and the
    // assertion is on the UNION of the names rather than on a sum, so it cannot depend
    // on where the clock is between two of the calls.
    std::thread::sleep(PAST_THE_TRANSITION);
    let mut covered: Vec<String> = Vec::new();
    // Every value of `MedicalScribeJobStatus`
    // (`smithy_query(query="shape", shape="MedicalScribeJobStatus")`).
    for status in ["QUEUED", "IN_PROGRESS", "COMPLETED", "FAILED"] {
        let filtered = list(&transcribe, json!({"Status": status, "MaxResults": 100})).await;
        let body = filtered.ok(status);
        assert_eq!(
            body["Status"],
            json!(status),
            "case {status:?}: the Status filter must be echoed back, got {:?}",
            body["Status"]
        );
        for summary in summaries_of(body) {
            assert_eq!(
                summary["MedicalScribeJobStatus"],
                json!(status),
                "case {status:?}: the filter must return only jobs in that status, \
                 got {summary:?}"
            );
        }
        covered.extend(names_of(body));
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
    let page = names_of(body);
    assert!(
        page.len() == 2 && page.iter().all(|name| name.starts_with("alpha")),
        "a composed filtered page must hold two alpha jobs, got {page:?}"
    );
    assert!(
        body["NextToken"].is_string(),
        "a truncated filtered page must carry a NextToken, got {:?}",
        body["NextToken"]
    );
    // An unfiltered listing carries no `Status` at all: the member is the filter the
    // request named, not a value the service invents.
    assert!(
        all.body["Status"].is_null(),
        "an unfiltered listing echoes no Status, got {:?}",
        all.body["Status"]
    );
}

#[tokio::test]
async fn a_next_token_this_service_did_not_hand_out_is_refused() {
    let transcribe = Transcribe::new();

    // Neither an unparseable cursor nor a well-formed one naming a sequence that
    // cannot exist (creation sequences start at 1) is accepted: silently listing from
    // the top would hide a caller's paging bug.
    for token in ["not-a-cursor", "0|listing-minimal", "|", "-1|listing-minimal"] {
        let refused = list(&transcribe, json!({"NextToken": token})).await;
        refused.refused(token, 400, "BadRequestException");
    }

    // A token this service DID hand out walks on from where the page stopped.
    create_named_resources(&transcribe).await;
    let names = start_the_table(&transcribe).await;
    let first = list(&transcribe, json!({"MaxResults": 3})).await;
    let token = first.ok("first page")["NextToken"]
        .as_str()
        .expect("a truncated page carries a NextToken")
        .to_string();
    let second = list(&transcribe, json!({"NextToken": token, "MaxResults": 3})).await;
    assert_eq!(
        names_of(second.ok("second page")),
        vec![names[0].clone()],
        "the second page must hold the one job the first page left, got {:?}",
        second.body["MedicalScribeJobSummaries"]
    );
}

#[tokio::test]
async fn the_listing_reports_only_the_scribe_family_and_only_what_is_stored() {
    let transcribe = Transcribe::new();

    // An empty store lists an empty page and no cursor, rather than refusing.
    let empty = list(&transcribe, json!({"MaxResults": 100})).await;
    let body = empty.ok("empty listing");
    assert_eq!(
        body["MedicalScribeJobSummaries"],
        json!([]),
        "an account that started no Medical Scribe job lists nothing, got {:?}",
        body["MedicalScribeJobSummaries"]
    );
    assert!(
        body["NextToken"].is_null(),
        "a listing that is not truncated carries no NextToken, got {:?}",
        body["NextToken"]
    );

    // Jobs of OTHER families under the same names are not Medical Scribe jobs: each
    // family is keyed on its own store.
    create_named_resources(&transcribe).await;
    let scribe = start_the_table(&transcribe).await;
    for name in &scribe {
        transcribe
            .call(
                "StartTranscriptionJob",
                json!({
                    "TranscriptionJobName": name.clone(),
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/not-scribe.wav"}
                }),
            )
            .await
            .ok(name);
        transcribe
            .call(
                "StartMedicalTranscriptionJob",
                json!({
                    "MedicalTranscriptionJobName": name.clone(),
                    "LanguageCode": "en-US",
                    "Media": {"MediaFileUri": "s3://example-media/not-scribe.wav"},
                    "OutputBucketName": "example-medical-output",
                    "Specialty": "PRIMARYCARE",
                    "Type": "DICTATION"
                }),
            )
            .await
            .ok(name);
    }

    let listed = list(&transcribe, json!({"MaxResults": 100})).await;
    let mut listed_names = names_of(listed.ok("listing beside the other families"));
    listed_names.sort();
    let mut expected = scribe.clone();
    expected.sort();
    assert_eq!(
        listed_names, expected,
        "the listing must report exactly the Medical Scribe jobs, got {:?}",
        listed.body["MedicalScribeJobSummaries"]
    );

    // A refused start stores nothing, so it reaches no listing: exactly one of
    // `ShowSpeakerLabels` and `ChannelIdentification` may be true.
    transcribe
        .call(
            "StartMedicalScribeJob",
            json!({
                "MedicalScribeJobName": "refused-listing",
                "Media": {"MediaFileUri": "s3://example-media/consult.flac"},
                "OutputBucketName": "example-scribe-output",
                "DataAccessRoleArn": ROLE_ARN,
                "Settings": {
                    "ShowSpeakerLabels": true,
                    "MaxSpeakerLabels": 2,
                    "ChannelIdentification": true
                }
            }),
        )
        .await
        .refused("both partitioning modes at once", 400, "BadRequestException");
    let after = list(
        &transcribe,
        json!({"JobNameContains": "refused-listing", "MaxResults": 100}),
    )
    .await;
    assert_eq!(
        after.ok("listing after a refused start")["MedicalScribeJobSummaries"],
        json!([]),
        "a refused start must reach no listing, got {:?}",
        after.body["MedicalScribeJobSummaries"]
    );

    // A deleted job leaves the listing: the listing is the store, not a log of what
    // was ever started. A delete needs a terminal status, so the clock runs out first.
    std::thread::sleep(PAST_THE_TRANSITION);
    let deleted = scribe[0].clone();
    transcribe
        .call(
            "DeleteMedicalScribeJob",
            json!({"MedicalScribeJobName": deleted.clone()}),
        )
        .await
        .ok(&deleted);
    let remaining = list(&transcribe, json!({"MaxResults": 100})).await;
    let names = names_of(remaining.ok("listing after a delete"));
    assert!(
        !names.contains(&deleted) && names.len() == scribe.len() - 1,
        "case {deleted:?}: a deleted job must leave the listing, got {names:?}"
    );

    // A `MaxResults` past each published bound is refused by the boundary validator,
    // before the handler runs: `MaxResults` is `@range(min: 1, max: 100)`.
    for max_results in [0, 101] {
        let case = format!("MaxResults={max_results}");
        list(&transcribe, json!({"MaxResults": max_results}))
            .await
            .refused(&case, 400, "ValidationException");
    }
}
