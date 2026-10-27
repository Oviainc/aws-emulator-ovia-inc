// SPDX-License-Identifier: CC0-1.0

//! What a `StartTranscriptionJob` request asks for about its OUTPUT ENCRYPTION, end to
//! end over the wire and down to the record the service stores for the job.
//!
//! THE RULE UNDER TEST: a start request's modelled members are all kept on the job
//! record the service stores — including the ones the job's own output shape does not
//! project — so `OutputEncryptionKMSKeyId` and `KMSEncryptionContext` are recorded
//! exactly as asked for, for every value their own constraints admit, and nothing
//! reports them back, because `TranscriptionJob` models neither member
//! (`smithy_query(query="shape", shape="TranscriptionJob")`). A value outside those
//! constraints is refused at the boundary and leaves no record at all.
//!
//! The rows span what the rule generalises over: whether each member is asked for at
//! all, the form of the key (an ARN, a bare key id, an alias), each published bound of
//! `KMSKeyId` (`@length(min: 1, max: 2048)`) and of `KMSEncryptionContextMap`
//! (`@length(min: 1, max: 10)`, keys and values `@length(min: 1, max: 2000)` with
//! `@pattern("^[\\x20-\\x7E]+$")`) and one past each bound, and where the transcript
//! would go (the caller's bucket or the service-managed one).
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so the awsJson1_1 envelope and the boundary constraint validation are
//! exercised too — and the request's wire envelope and the answer assertions are the
//! transcription-job suite's harness (`crate::transcription_jobs`), reused rather than
//! written a second time. The emulator is stood up here rather than taken from that
//! harness because these tests read the STORED record, which needs the shared engine
//! the service was mounted on.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_transcribe_transcribe::state::{
    service_state, TranscriptionJobRecord, DEFAULT_REGION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::transcription_jobs::{dispatch, Reply};

/// Long enough for every clock-owed transition of a job started during the test to have
/// fallen due: a job's `IN_PROGRESS -> COMPLETED` is one span of
/// `crate::state::JOB_TRANSITION_DELAY` (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`, `rt`
/// and `rt-multi-thread` features only, so `tokio::time::sleep` is not available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The status the awsJson1_1 envelope answers every client refusal with.
const CLIENT_ERROR_STATUS: u16 = 400;

/// The error a boundary constraint violation is refused with, before the handler runs.
const BOUNDARY_CODE: &str = "ValidationException";

/// One emulator: the service mounted on a single shared engine behind the router, with
/// the engine kept so a test can read the record the service stored.
struct Emulator {
    router: Arc<MultiServiceRouter>,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_transcribe_transcribe::EmulatedService::new(Arc::clone(&engine)),
        );
        Self {
            router: Arc::new(router),
            engine,
        }
    }

    /// One unsigned awsJson1_1 call, which the service serves in [`DEFAULT_REGION`].
    async fn call(&self, operation: &str, input: Value) -> Reply {
        dispatch(&self.router, operation, input, None).await
    }

    /// The record this service stored for the transcription job `name`, if any.
    fn stored(&self, name: &str) -> Option<TranscriptionJobRecord> {
        let state = service_state(&*self.engine).expect("this service's own store");
        let store = state.lock();
        store.transcription_jobs.get(DEFAULT_REGION, name).cloned()
    }
}

/// `GetTranscriptionJob` for `name`.
async fn get(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("GetTranscriptionJob", json!({"TranscriptionJobName": name}))
        .await
}

/// The job name of a start request.
fn job_name(request: &Value) -> String {
    request["TranscriptionJobName"]
        .as_str()
        .expect("every row names its job")
        .to_string()
}

/// What the request asked for as `OutputEncryptionKMSKeyId`: the expectation IS the
/// request, which is the whole of the rule under test.
fn asked_key(request: &Value) -> Option<String> {
    request
        .get("OutputEncryptionKMSKeyId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// What the request asked for as `KMSEncryptionContext`, as the map a record holds.
fn asked_context(request: &Value) -> Option<HashMap<String, String>> {
    request.get("KMSEncryptionContext").map(|context| {
        context
            .as_object()
            .expect("an encryption context is a JSON object")
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .expect("an encryption context value is a string")
                        .to_string(),
                )
            })
            .collect()
    })
}

/// A `KMSEncryptionContext` of `pairs` entries, each inside the published key and value
/// bounds.
fn context_of(pairs: usize) -> Value {
    let mut map = Map::new();
    for index in 1..=pairs {
        map.insert(format!("context-key-{index}"), json!(format!("v{index}")));
    }
    Value::Object(map)
}

/// A `KMSKeyId` of exactly `length` characters, inside its own `@pattern`
/// (`^[A-Za-z0-9][A-Za-z0-9:_/+=,@.-]{0,2048}$`).
fn key_of_length(length: usize) -> String {
    format!("k{}", "0".repeat(length - 1))
}

/// A printable string of exactly `length` characters, inside the published pattern of a
/// `KMSEncryptionContextMap` key and value (`^[\x20-\x7E]+$`).
fn printable_of_length(length: usize) -> String {
    "c".repeat(length)
}

/// A `KMSEncryptionContext` of the single pair `key`:`value`.
fn context_pair(key: String, value: String) -> Value {
    let mut map = Map::new();
    map.insert(key, json!(value));
    Value::Object(map)
}

/// The class of output-encryption choice a `StartTranscriptionJob` request can make,
/// one row per axis the rule generalises over: whether each of the two members is asked
/// for, the form of the key, each published bound of the key's length and of the
/// context map's size and of its keys' and values' lengths, and whether the transcript
/// would go to the caller's bucket or the service-managed one.
fn accepted_rows() -> Vec<(String, Value)> {
    vec![
        (
            "no output encryption asked for at all".to_string(),
            json!({
                "TranscriptionJobName": "kms-absent",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
                "OutputBucketName": "example-output"
            }),
        ),
        (
            "a key ARN and no encryption context, output in the caller's bucket".to_string(),
            json!({
                "TranscriptionJobName": "kms-arn",
                "LanguageCode": "en-US",
                "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
                "OutputBucketName": "example-output",
                "OutputKey": "transcripts/audio.json",
                "OutputEncryptionKMSKeyId":
                    "arn:aws:kms:us-west-2:111122223333:key/1234abcd-12ab-34cd-56ef-1234567890ab"
            }),
        ),
        (
            "a bare key id and one context pair, output in the SERVICE-managed bucket"
                .to_string(),
            json!({
                "TranscriptionJobName": "kms.bare-id",
                "LanguageCode": "fr-FR",
                "Media": {"MediaFileUri": "s3://example-media/call.mp3"},
                "OutputEncryptionKMSKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
                "KMSEncryptionContext": {"team": "speech"}
            }),
        ),
        (
            "a key alias and the published maximum of 10 context pairs".to_string(),
            json!({
                "TranscriptionJobName": "kms_alias",
                "LanguageCode": "de-DE",
                "Media": {"MediaFileUri": "s3://example-media/meeting.flac"},
                "OutputBucketName": "example-output",
                "OutputEncryptionKMSKeyId": "alias/aws/transcribe",
                "KMSEncryptionContext": context_of(10)
            }),
        ),
        (
            "a key at the published maximum length, a context pair at the published \
             maximum key and value length"
                .to_string(),
            json!({
                "TranscriptionJobName": "kms-bounds",
                "LanguageCode": "en-GB",
                "Media": {"MediaFileUri": "s3://example-media/lecture.ogg"},
                "OutputBucketName": "example-output",
                "OutputEncryptionKMSKeyId": key_of_length(2_048),
                "KMSEncryptionContext":
                    context_pair(printable_of_length(2_000), printable_of_length(2_000))
            }),
        ),
        (
            "a key at the published minimum length and an encryption context with no \
             key asked for"
                .to_string(),
            json!({
                "TranscriptionJobName": "kms-minimal",
                "Media": {"MediaFileUri": "s3://example-media/clip.webm"},
                "IdentifyLanguage": true,
                "OutputEncryptionKMSKeyId": key_of_length(1),
                "KMSEncryptionContext": {"a": "b"}
            }),
        ),
    ]
}

/// The output-encryption values the members' own constraints do NOT admit, one row per
/// bound: the key one past each of its published length bounds and outside its pattern,
/// the context map one past each of its published size bounds, and a context value one
/// past its length bound and outside its pattern.
fn refused_rows() -> Vec<(String, Value)> {
    vec![
        (
            "a key one past the published maximum length".to_string(),
            json!({"OutputEncryptionKMSKeyId": key_of_length(2_049)}),
        ),
        (
            "a key one below the published minimum length".to_string(),
            json!({"OutputEncryptionKMSKeyId": ""}),
        ),
        (
            "a key carrying a character outside its published pattern".to_string(),
            json!({"OutputEncryptionKMSKeyId": "key id"}),
        ),
        (
            "an encryption context one past the published maximum size".to_string(),
            json!({"KMSEncryptionContext": context_of(11)}),
        ),
        (
            "an encryption context one below the published minimum size".to_string(),
            json!({"KMSEncryptionContext": {}}),
        ),
        (
            "a context value one past the published maximum length".to_string(),
            json!({
                "KMSEncryptionContext":
                    context_pair("team".to_string(), printable_of_length(2_001))
            }),
        ),
        (
            "a context value one below the published minimum length".to_string(),
            json!({"KMSEncryptionContext": {"team": ""}}),
        ),
        (
            "a context value carrying a character outside the published pattern".to_string(),
            json!({"KMSEncryptionContext": {"team": "speech\n"}}),
        ),
    ]
}

/// A start request under `name` with the members of `members` set on it.
fn start_request(name: &str, members: &Value) -> Value {
    let mut request = json!({
        "TranscriptionJobName": name,
        "LanguageCode": "en-US",
        "Media": {"MediaFileUri": "s3://example-media/audio.wav"},
        "OutputBucketName": "example-output"
    });
    let object = request
        .as_object_mut()
        .expect("a start request is a JSON object");
    for (member, value) in members
        .as_object()
        .expect("a row's members are a JSON object")
    {
        object.insert(member.clone(), value.clone());
    }
    request
}

/// What a job answers about the two members: nothing, whichever operation is asked —
/// the `TranscriptionJob` shape models neither.
fn assert_not_reported(case: &str, job: &Value) {
    for member in ["OutputEncryptionKMSKeyId", "KMSEncryptionContext"] {
        assert!(
            job.get(member).is_none(),
            "case {case:?}: TranscriptionJob models no {member}, so a job must not \
             report one, got {:?}",
            job.get(member)
        );
    }
}

#[tokio::test]
async fn a_started_jobs_output_encryption_is_recorded_exactly_as_it_was_asked_for() {
    let emulator = Emulator::new();

    for (case, request) in accepted_rows() {
        let case = case.as_str();
        let started = emulator
            .call("StartTranscriptionJob", request.clone())
            .await;
        let job = started.ok(case)["TranscriptionJob"].clone();
        assert_not_reported(case, &job);

        let name = job_name(&request);
        let record = emulator.stored(&name).unwrap_or_else(|| {
            panic!("case {case:?}: an accepted start must leave a record under {name:?}")
        });
        assert_eq!(
            record.output_encryption_kms_key_id,
            asked_key(&request),
            "case {case:?}: the record must hold the key the start asked for, got {:?}",
            record.output_encryption_kms_key_id
        );
        assert_eq!(
            record.kms_encryption_context,
            asked_context(&request),
            "case {case:?}: the record must hold the encryption context the start asked \
             for, got {:?}",
            record.kms_encryption_context
        );

        // Reading the job back changes neither member: a read reports the record, it
        // does not rewrite what the start asked for.
        let fetched = get(&emulator, &name).await;
        assert_not_reported(case, &fetched.ok(case)["TranscriptionJob"]);
        let after_read = emulator
            .stored(&name)
            .unwrap_or_else(|| panic!("case {case:?}: the record must still be there"));
        assert_eq!(
            (
                after_read.output_encryption_kms_key_id,
                after_read.kms_encryption_context
            ),
            (asked_key(&request), asked_context(&request)),
            "case {case:?}: a read must leave the recorded output encryption alone"
        );
    }
}

#[tokio::test]
async fn the_recorded_output_encryption_outlives_the_jobs_own_clock() {
    let emulator = Emulator::new();

    // One row per way a job can reach a terminal status with an encryption choice
    // recorded: a transcript in the caller's bucket and one in the service-managed
    // bucket, each with and without an encryption context.
    for (case, request) in accepted_rows() {
        let case = case.as_str();
        emulator
            .call("StartTranscriptionJob", request.clone())
            .await
            .ok(case);
    }
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in accepted_rows() {
        let case = case.as_str();
        let name = job_name(&request);
        let job = get(&emulator, &name).await.ok(case)["TranscriptionJob"].clone();
        assert_eq!(
            job["TranscriptionJobStatus"],
            json!("COMPLETED"),
            "case {case:?}: the clock must have completed the job, got {:?}",
            job["TranscriptionJobStatus"]
        );
        assert_not_reported(case, &job);

        let record = emulator
            .stored(&name)
            .unwrap_or_else(|| panic!("case {case:?}: a completed job is still stored"));
        assert_eq!(
            (
                record.output_encryption_kms_key_id,
                record.kms_encryption_context
            ),
            (asked_key(&request), asked_context(&request)),
            "case {case:?}: completing a job must leave the recorded output encryption \
             as the start asked for it"
        );
    }
}

#[tokio::test]
async fn an_output_encryption_value_outside_its_published_bounds_stores_nothing() {
    let emulator = Emulator::new();

    for (index, (case, members)) in refused_rows().into_iter().enumerate() {
        let case = case.as_str();
        let name = format!("kms-refused-{index}");
        let refused = emulator
            .call("StartTranscriptionJob", start_request(&name, &members))
            .await;
        refused.refused(case, CLIENT_ERROR_STATUS, BOUNDARY_CODE);

        assert!(
            emulator.stored(&name).is_none(),
            "case {case:?}: a refused start must store no record, got {:?}",
            emulator.stored(&name)
        );
        get(&emulator, &name)
            .await
            .refused(case, CLIENT_ERROR_STATUS, "BadRequestException");
    }
}
