// SPDX-License-Identifier: CC0-1.0

//! How this service answers over the wire, status-wise.
//!
//! Amazon Transcribe speaks `awsJson1_1` (`smithy_query(query="service_info")`), an
//! RPC protocol, so every REST-shaped trait in its model is inert on the wire —
//! `smithy_query(query="operation", ...)` reports `httpBindingsInert: true` for every
//! operation — and neither the per-shape `@httpError` values it records for client
//! errors (`NotFoundException` 404, `ConflictException` 409, `LimitExceededException`
//! 429) nor the `@http` `code` values it records for successes (201, 204) are statuses
//! the service emits. Three rules are pinned here, each over a SET of inputs rather
//! than one recorded call:
//!
//! 1. Every client error answers HTTP 400 and names its shape in the body's
//!    `__type`; only a server error keeps the 5xx the model gives it.
//! 2. Every success answers HTTP 200.
//! 3. A request naming a resource this account does not hold is refused with
//!    `BadRequestException` — the shape whose own model documentation covers "the
//!    entity you're trying to delete doesn't exist" — and the refusal names the KIND
//!    of resource, never the name the caller asked about.
//!
//! The harness (the emulator, the awsJson1_1 call and the answer accessors) is the
//! transcription-job suite's, reused rather than stood up a second time.

use serde_json::{json, Value};

use crate::transcription_jobs::Transcribe;

/// The resource kinds this service's own refusals name, one per ARN resource type it
/// stores records of, plus the neutral word for a type it holds none of. The three
/// stored types are the ones `TranscribeArn`'s `<type>/<name>` segment spells
/// (`smithy_query(query="operation", operation="TagResource")`).
const KIND_VOCABULARY: &str = "vocabulary";
const KIND_VOCABULARY_FILTER: &str = "vocabulary filter";
const KIND_JOB: &str = "job";
const KIND_RESOURCE: &str = "resource";

/// The region the unsigned rows of this suite are served in, so a tagging ARN is
/// predictable (the service serves a request with no SigV4 credential scope in its
/// default region).
const REGION: &str = "us-east-1";

/// An Amazon Transcribe ARN of `resource_type` naming `name`, in the form
/// `TranscribeArn`'s `@pattern` pins.
fn arn(resource_type: &str, name: &str) -> String {
    format!(
        "arn:aws:transcribe:{REGION}:{account}:{resource_type}/{name}",
        account = engine::emulator_account_id()
    )
}

/// `wire_status` maps the whole published client range onto one status and leaves a
/// server status alone.
///
/// The inputs are every `httpError` this service's model records, from
/// `smithy_query(query="operation", ...)`: 400 `BadRequestException`, 404
/// `NotFoundException`, 409 `ConflictException`, 429 `LimitExceededException`, 500
/// `InternalFailureException` — plus each bound of the client range and one past it,
/// so the mapping is pinned as a range rather than as a list of the four shapes this
/// model happens to carry.
#[test]
fn the_client_range_maps_to_one_status_and_a_server_status_is_kept() {
    use service_com_amazonaws_transcribe_transcribe::scaffolded::wire_status;

    let rows: &[(&str, u16, u16)] = &[
        ("BadRequestException, the modelled 400", 400, 400),
        ("NotFoundException, the modelled 404", 404, 400),
        ("ConflictException, the modelled 409", 409, 400),
        ("LimitExceededException, the modelled 429", 429, 400),
        ("the lowest client status", 400, 400),
        ("the highest client status", 499, 400),
        ("InternalFailureException, the modelled 500", 500, 500),
        ("one past the client range", 500, 500),
        ("a server status above it", 503, 503),
        ("a redirect, which is below the client range", 301, 301),
    ];

    for (case, modeled, expected) in rows {
        let actual = wire_status(*modeled);
        assert_eq!(
            actual, *expected,
            "case {case:?}: a modelled {modeled} must go on the wire as {expected}, got \
             {actual}"
        );
    }
}

/// Every operation answers this protocol's own success status, 200, whatever `@http`
/// `code` the model records for it.
#[tokio::test]
async fn every_success_answers_this_protocols_status() {
    let transcribe = Transcribe::new();

    // Rows differ along the axis the rule generalises over: the inert `@http` `code`
    // the model records for the operation — 200 (`CreateVocabulary`,
    // `StartTranscriptionJob`, `GetVocabulary`, `TagResource`), 201
    // (`CreateVocabularyFilter`) and 204 (`UntagResource`, `DeleteVocabularyFilter`,
    // `DeleteTranscriptionJob`), from `smithy_query(query="operation", ...)`.
    let setup: Vec<(&str, &str, Value)> = vec![
        (
            "CreateVocabulary, modelled code 200",
            "CreateVocabulary",
            json!({
                "VocabularyName": "status-vocab",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        ),
        (
            "CreateVocabularyFilter, modelled code 201",
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "status-filter",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        ),
        (
            "StartTranscriptionJob, modelled code 200",
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "status-job",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
            }),
        ),
    ];
    for (case, operation, request) in setup {
        transcribe
            .call(operation, request)
            .await
            .ok_status(case, 200);
    }

    // One row per remaining inert code, each on an operation whose success this
    // emulator can reach: a read (200), a tag write (200), a tag delete (204) and a
    // resource delete (204).
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "GetVocabulary, modelled code 200",
            "GetVocabulary",
            json!({"VocabularyName": "status-vocab"}),
        ),
        (
            "TagResource, modelled code 200",
            "TagResource",
            json!({
                "ResourceArn": arn("vocabulary", "status-vocab"),
                "Tags": [{"Key": "team", "Value": "speech"}]
            }),
        ),
        (
            "UntagResource, modelled code 204",
            "UntagResource",
            json!({
                "ResourceArn": arn("vocabulary", "status-vocab"),
                "TagKeys": ["team"]
            }),
        ),
        (
            "DeleteVocabularyFilter, modelled code 204",
            "DeleteVocabularyFilter",
            json!({"VocabularyFilterName": "status-filter"}),
        ),
    ];

    for (case, operation, request) in rows {
        transcribe.call(operation, request).await.ok_status(case, 200);
    }
}

/// Every operation that can be asked about a resource this account does not hold
/// answers HTTP 400 `BadRequestException`, whatever family the resource belongs to and
/// whether the operation reads, writes or deletes.
#[tokio::test]
async fn a_resource_this_account_does_not_hold_is_a_bad_request() {
    let transcribe = Transcribe::new();

    // Rows differ along what the rule generalises over: the operation (read, update,
    // delete, and each of the three tagging operations), the resource family, and —
    // for the tagging operations, which address a resource by ARN — the ARN's resource
    // type, including one type this emulator stores no records of.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "GetVocabulary for a vocabulary that was never created",
            "GetVocabulary",
            json!({"VocabularyName": "never-created"}),
            KIND_VOCABULARY,
        ),
        (
            "UpdateVocabulary for a vocabulary that was never created",
            "UpdateVocabulary",
            json!({
                "VocabularyName": "never-created",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            KIND_VOCABULARY,
        ),
        (
            "DeleteVocabulary for a vocabulary that was never created",
            "DeleteVocabulary",
            json!({"VocabularyName": "never-created"}),
            KIND_VOCABULARY,
        ),
        (
            "GetVocabularyFilter for a filter that was never created",
            "GetVocabularyFilter",
            json!({"VocabularyFilterName": "never-created"}),
            KIND_VOCABULARY_FILTER,
        ),
        (
            "UpdateVocabularyFilter for a filter that was never created",
            "UpdateVocabularyFilter",
            json!({"VocabularyFilterName": "never-created", "Words": ["crud"]}),
            KIND_VOCABULARY_FILTER,
        ),
        (
            "DeleteVocabularyFilter for a filter that was never created",
            "DeleteVocabularyFilter",
            json!({"VocabularyFilterName": "never-created"}),
            KIND_VOCABULARY_FILTER,
        ),
        (
            "GetTranscriptionJob for a job that was never started",
            "GetTranscriptionJob",
            json!({"TranscriptionJobName": "never-started"}),
            KIND_JOB,
        ),
        (
            "DeleteTranscriptionJob for a job that was never started",
            "DeleteTranscriptionJob",
            json!({"TranscriptionJobName": "never-started"}),
            KIND_JOB,
        ),
        (
            "ListTagsForResource for a job ARN that names nothing",
            "ListTagsForResource",
            json!({"ResourceArn": arn("transcription-job", "never-started")}),
            KIND_JOB,
        ),
        (
            "TagResource for a job ARN that names nothing",
            "TagResource",
            json!({
                "ResourceArn": arn("transcription-job", "never-started"),
                "Tags": [{"Key": "team", "Value": "speech"}]
            }),
            KIND_JOB,
        ),
        (
            "UntagResource for a job ARN that names nothing",
            "UntagResource",
            json!({
                "ResourceArn": arn("transcription-job", "never-started"),
                "TagKeys": ["team"]
            }),
            KIND_JOB,
        ),
        (
            "ListTagsForResource for a vocabulary ARN that names nothing",
            "ListTagsForResource",
            json!({"ResourceArn": arn("vocabulary", "never-created")}),
            KIND_VOCABULARY,
        ),
        (
            "ListTagsForResource for a filter ARN that names nothing",
            "ListTagsForResource",
            json!({"ResourceArn": arn("vocabulary-filter", "never-created")}),
            KIND_VOCABULARY_FILTER,
        ),
        (
            "ListTagsForResource for a resource type this emulator holds none of",
            "ListTagsForResource",
            json!({"ResourceArn": arn("language-model", "any-model")}),
            KIND_RESOURCE,
        ),
    ];

    for (case, operation, request, kind) in rows {
        let refused = transcribe.call(operation, request.clone()).await;
        refused.refused(case, 400, "BadRequestException");

        // The refusal names the KIND of resource and does not echo the name or ARN the
        // caller asked about: these texts are the service's own, not a rendering of
        // the request.
        let message = refused.error_message(case);
        assert!(
            message.contains(&format!("The requested {kind} couldn't be found")),
            "case {case:?}: the refusal must name the {kind:?} kind, got {message:?}"
        );
        for asked_about in request.as_object().expect("an object").values() {
            if let Some(asked_about) = asked_about.as_str() {
                assert!(
                    !message.contains(asked_about),
                    "case {case:?}: the refusal must not echo the requested \
                     {asked_about:?}, got {message:?}"
                );
            }
        }
    }
}

/// A name already in use is refused with `ConflictException` — the shape the model
/// gives a duplicate name — carried on HTTP 400, for each family whose create or start
/// claims a name.
#[tokio::test]
async fn a_name_already_in_use_is_a_conflict_on_http_400() {
    let transcribe = Transcribe::new();

    // Rows differ in the family whose name space is claimed, and so in the operation
    // that claims it and the success status it answers.
    let rows: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "a second CreateVocabulary of one name",
            "CreateVocabulary",
            json!({
                "VocabularyName": "claimed",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            200,
            KIND_VOCABULARY,
        ),
        (
            "a second CreateVocabularyFilter of one name",
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "claimed",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
            200,
            KIND_VOCABULARY_FILTER,
        ),
        (
            "a second StartTranscriptionJob of one name",
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "claimed",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"}
            }),
            200,
            KIND_JOB,
        ),
    ];

    for (case, operation, request, success, kind) in rows {
        transcribe
            .call(operation, request.clone())
            .await
            .ok_status(case, success);

        let conflict = transcribe.call(operation, request).await;
        conflict.refused(case, 400, "ConflictException");
        let message = conflict.error_message(case);
        assert!(
            message.contains(&format!("The requested {kind} name already exists")),
            "case {case:?}: the conflict must name the {kind:?} kind, got {message:?}"
        );
        assert!(
            !message.contains("claimed"),
            "case {case:?}: the conflict must not echo the requested name, got \
             {message:?}"
        );
    }
}

/// An exceeded quota — the client error this service models with `@httpError` 429 —
/// also answers HTTP 400, with `LimitExceededException` in `__type`.
///
/// The ceiling is this emulator's own `state::MAX_VOCABULARIES`, so the row fills it
/// exactly and then asks for one more.
#[tokio::test]
async fn an_exceeded_quota_is_http_400_too() {
    let transcribe = Transcribe::new();
    let ceiling = service_com_amazonaws_transcribe_transcribe::state::MAX_VOCABULARIES;

    for index in 0..ceiling {
        transcribe
            .call(
                "CreateVocabularyFilter",
                json!({
                    "VocabularyFilterName": format!("quota-{index}"),
                    "LanguageCode": "en-US",
                    "Words": ["crud"]
                }),
            )
            .await
            .ok_status("a filter within the quota", 200);
    }

    let over = transcribe
        .call(
            "CreateVocabularyFilter",
            json!({
                "VocabularyFilterName": "quota-over",
                "LanguageCode": "en-US",
                "Words": ["crud"]
            }),
        )
        .await;
    over.refused("one filter past the quota", 400, "LimitExceededException");
}

/// No client error of this service answers a per-shape `@httpError` status: across a
/// mixed set of refusals — a missing resource, a duplicate name, a request the
/// service's own rules reject and one the model's constraint traits reject — every
/// answer is HTTP 400 carrying a modelled error code.
#[tokio::test]
async fn every_client_refusal_answers_http_400_with_a_modelled_code() {
    let transcribe = Transcribe::new();
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "taken",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("the create whose name the conflict row re-uses");

    // Rows differ in WHY the request is refused, which is the axis the status rule
    // generalises over: each is a different error shape, with a different modelled
    // `@httpError`.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a vocabulary that does not exist",
            "GetVocabulary",
            json!({"VocabularyName": "never-created"}),
            "BadRequestException",
        ),
        (
            "a name already in use",
            "CreateVocabulary",
            json!({
                "VocabularyName": "taken",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
            "ConflictException",
        ),
        (
            "a create that says nowhere its terms come from",
            "CreateVocabulary",
            json!({"VocabularyName": "no-content", "LanguageCode": "en-US"}),
            "BadRequestException",
        ),
        (
            "a paging token this service never handed out",
            "ListVocabularies",
            json!({"NextToken": "not-a-token"}),
            "BadRequestException",
        ),
        (
            "a name outside the modelled pattern, refused at the boundary",
            "GetVocabulary",
            json!({"VocabularyName": "has spaces"}),
            "ValidationException",
        ),
        (
            "an ARN outside the modelled pattern, refused at the boundary",
            "ListTagsForResource",
            json!({"ResourceArn": "not-an-arn"}),
            "ValidationException",
        ),
    ];

    for (case, operation, request, code) in rows {
        let refused = transcribe.call(operation, request).await;
        refused.refused(case, 400, code);
    }
}
