// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of HOW `StartMeetingTranscription` words the refusal of a
//! `TranscriptionConfiguration` that does not name exactly one engine.
//!
//! The rule under test: the choice between the two engines' settings blocks is EXCLUSIVE,
//! and every violation of it — the block missing, or both blocks present — is refused with
//! the one fixed sentence the service serves for that class, naming only the modelled
//! member names `EngineTranscribeSettings`, `EngineTranscribeMedicalSettings` and
//! `TranscriptionConfiguration`. Nothing of the request is spliced into it: not the meeting
//! it named, not the settings the blocks carried, and not which half of the exclusive
//! choice was broken.
//!
//! The table spans the class the rule generalises over: both halves of the violation
//! (neither block, both blocks), the contents of the blocks that are there (a stated
//! language, an identified language, optional vocabularies and regions, each medical
//! specialty/type pair), configurations whose inner blocks would THEMSELVES break a
//! per-engine combination rule (the choice is judged first, so the sentence is still this
//! one), and the meeting the call named (a live meeting in two media regions, and an id
//! naming no meeting at all — the configuration is judged before the store is consulted).
//!
//! Two control rows keep the sentence specific to its class: a configuration naming exactly
//! one engine correctly is ACCEPTED, and one naming exactly one engine while breaking that
//! engine's own combination rule is refused with a different sentence.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `chime` signing name
/// this service publishes (`OPERATIONS.md`, `## Wire protocol`). The scope is how the
/// router picks the claiming service; the signature itself is never checked here, since the
/// harness turns request authentication off.
const SIGNED_FOR_CHIME: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// The sentence the exclusive choice between the two engines is refused with.
const ENGINE_CHOICE_MESSAGE: &str = "Exactly one out of EngineTranscribeSettings and \
     EngineTranscribeMedicalSettings must be specified in the TranscriptionConfiguration";

/// A well-formed `GuidString` (the modelled pattern
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`) that names no meeting, so the
/// request reaches the handler rather than being refused by the boundary validator.
const UNKNOWN_MEETING_ID: &str = "11111111-2222-4333-8444-555555555555";

/// The values of the model's `TranscribeMedicalSpecialty` enum
/// (`smithy_query(query="shape",shape="TranscribeMedicalSpecialty",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`).
const MEDICAL_SPECIALTIES: &[&str] = &[
    "PRIMARYCARE",
    "CARDIOLOGY",
    "NEUROLOGY",
    "ONCOLOGY",
    "RADIOLOGY",
    "UROLOGY",
];

/// The values of the model's `TranscribeMedicalType` enum (same `smithy_query`, with
/// `shape="TranscribeMedicalType"`).
const MEDICAL_TYPES: &[&str] = &["CONVERSATION", "DICTATION"];

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The refusal's `Message` member.
    fn message(&self) -> Option<String> {
        self.body
            .get("Message")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// Send one request through the router and reduce the answer to a [`Reply`].
async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<&Value>) -> Reply {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(value).expect("a request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://meetings-chime.us-east-1.amazonaws.com{path}"))
        .header("authorization", SIGNED_FOR_CHIME)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    // restJson1 carries the wire error code in `x-amzn-errortype`; fall back to the body's
    // `__type` so a missing header never reads as "no error code".
    let error_code = header_code.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
    });
    Reply {
        status,
        error_code,
        body,
    }
}

/// The emulator under test: ONE shared engine with this service mounted on it, reached the
/// way the production router reaches it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Create a meeting in `media_region` and answer its minted `MeetingId`.
async fn new_meeting(router: &MultiServiceRouter, token: &str, media_region: &str) -> String {
    let reply = call(
        router,
        "POST",
        "/meetings",
        Some(&json!({
            "ClientRequestToken": token,
            "MediaRegion": media_region,
            "ExternalMeetingId": format!("ext-{token}"),
        })),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "CreateMeeting {token:?}: expected 200, got {} with {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("Meeting")
        .and_then(|meeting| meeting.get("MeetingId"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateMeeting {token:?} returned no MeetingId"))
        .to_string()
}

/// Ask `StartMeetingTranscription` to put `configuration` in force for `meeting_id`.
async fn start_transcription(
    router: &MultiServiceRouter,
    meeting_id: &str,
    configuration: &Value,
) -> Reply {
    call(
        router,
        "POST",
        &format!("/meetings/{meeting_id}/transcription?operation=start"),
        Some(&json!({ "TranscriptionConfiguration": configuration })),
    )
    .await
}

/// A valid Amazon Transcribe Medical settings block.
fn medical(specialty: &str, medical_type: &str) -> Value {
    json!({
        "LanguageCode": "en-US",
        "Specialty": specialty,
        "Type": medical_type,
    })
}

/// Every configuration that breaks the exclusive choice, paired with the case name.
///
/// Half the rows name NEITHER engine and half name BOTH, and the blocks they carry differ
/// in which language mode, which optional members and which medical specialty/type pair
/// they use — including blocks that break a per-engine rule on their own.
fn engine_choice_violations() -> Vec<(String, Value)> {
    let mut rows: Vec<(String, Value)> = vec![
        ("an empty configuration".to_string(), json!({})),
        (
            "both engines, each in its simplest valid form".to_string(),
            json!({
                "EngineTranscribeSettings": { "LanguageCode": "en-US" },
                "EngineTranscribeMedicalSettings": medical("PRIMARYCARE", "CONVERSATION"),
            }),
        ),
        (
            "both engines, the transcribe block identifying the language".to_string(),
            json!({
                "EngineTranscribeSettings": {
                    "IdentifyLanguage": true,
                    "LanguageOptions": "en-US,es-US",
                    "PreferredLanguage": "en-US",
                },
                "EngineTranscribeMedicalSettings": medical("ONCOLOGY", "DICTATION"),
            }),
        ),
        (
            "both engines, with optional members riding along".to_string(),
            json!({
                "EngineTranscribeSettings": {
                    "LanguageCode": "en-GB",
                    "Region": "eu-west-2",
                    "VocabularyName": "vocab-a",
                    "VocabularyFilterName": "filter-a",
                    "VocabularyFilterMethod": "mask",
                    "EnablePartialResultsStabilization": true,
                    "PartialResultsStability": "high",
                },
                "EngineTranscribeMedicalSettings": json!({
                    "LanguageCode": "en-US",
                    "Specialty": "RADIOLOGY",
                    "Type": "DICTATION",
                    "VocabularyName": "vocab-med",
                    "Region": "ca-central-1",
                    "ContentIdentificationType": "PHI",
                }),
            }),
        ),
        (
            "both engines, the transcribe block breaking a per-engine rule too".to_string(),
            json!({
                "EngineTranscribeSettings": {
                    "IdentifyLanguage": true,
                    "LanguageCode": "en-US",
                },
                "EngineTranscribeMedicalSettings": medical("UROLOGY", "CONVERSATION"),
            }),
        ),
    ];
    // One row per modelled medical specialty/type pair, so the sentence is shown not to
    // depend on anything inside the block that is there.
    for specialty in MEDICAL_SPECIALTIES {
        for medical_type in MEDICAL_TYPES {
            rows.push((
                format!("both engines, medical {specialty}/{medical_type}"),
                json!({
                    "EngineTranscribeSettings": { "LanguageCode": "en-US" },
                    "EngineTranscribeMedicalSettings": medical(specialty, medical_type),
                }),
            ));
        }
    }
    rows
}

/// Every violation of the exclusive engine choice is refused with HTTP 400
/// `BadRequestException` carrying the one fixed sentence for that class, whichever half of
/// the choice it broke, whatever the blocks it carried held, and whichever meeting it named.
#[tokio::test]
async fn a_configuration_that_names_no_single_engine_is_refused_with_the_fixed_message() {
    let router = router();
    let live_east = new_meeting(&router, "tok-choice-east", "us-east-1").await;
    let live_west = new_meeting(&router, "tok-choice-west", "eu-west-1").await;
    // Every distinct sentence served over the whole table: the rule says exactly one.
    let mut served: BTreeSet<String> = BTreeSet::new();

    for (case, configuration) in engine_choice_violations() {
        for (target, meeting_id) in [
            ("a live meeting in us-east-1", live_east.as_str()),
            ("a live meeting in eu-west-1", live_west.as_str()),
            ("an id naming no meeting", UNKNOWN_MEETING_ID),
        ] {
            let reply = start_transcription(&router, meeting_id, &configuration).await;
            assert_eq!(
                (reply.status, reply.error_code.as_deref()),
                (400, Some("BadRequestException")),
                "case {case:?} against {target}: expected the modelled 400 BadRequestException, \
                 got {} {:?} with {:?}",
                reply.status,
                reply.error_code,
                reply.body
            );
            assert_eq!(
                reply.message().as_deref(),
                Some(ENGINE_CHOICE_MESSAGE),
                "case {case:?} against {target}: expected message {ENGINE_CHOICE_MESSAGE:?}, \
                 got {:?}",
                reply.message()
            );
            // Nothing of the request is in the sentence.
            let message = reply.message().unwrap_or_default();
            for value in [meeting_id, "en-US", "vocab-", "PRIMARYCARE"] {
                assert!(
                    !message.contains(value),
                    "case {case:?} against {target}: the message must not carry the request \
                     value {value:?}, got {message:?}"
                );
            }
            served.insert(message);
        }
    }

    assert_eq!(
        served,
        BTreeSet::from([ENGINE_CHOICE_MESSAGE.to_string()]),
        "the whole class must serve exactly one sentence, got {served:?}"
    );
}

/// The sentence belongs to the exclusive-choice class alone: a configuration naming exactly
/// one engine correctly is ACCEPTED, and one naming exactly one engine while breaking that
/// engine's own combination rule is refused with a different sentence.
#[tokio::test]
async fn naming_exactly_one_engine_is_not_refused_with_the_engine_choice_message() {
    let router = router();
    let meeting_id = new_meeting(&router, "tok-choice-control", "us-west-2").await;

    let accepted: Vec<(&str, Value)> = vec![
        (
            "only the transcribe block, language stated",
            json!({ "EngineTranscribeSettings": { "LanguageCode": "en-US" } }),
        ),
        (
            "only the transcribe block, language identified",
            json!({
                "EngineTranscribeSettings": {
                    "IdentifyLanguage": true,
                    "LanguageOptions": "en-US,fr-FR",
                },
            }),
        ),
        (
            "only the medical block",
            json!({ "EngineTranscribeMedicalSettings": medical("NEUROLOGY", "CONVERSATION") }),
        ),
    ];
    for (case, configuration) in accepted {
        let reply = start_transcription(&router, &meeting_id, &configuration).await;
        assert_eq!(
            (reply.status, reply.error_code.as_deref()),
            (200, None),
            "case {case:?}: naming exactly one engine must be accepted, got {} {:?} with {:?}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }

    // Exactly one engine, but that engine's own rule is broken: still a 400
    // BadRequestException, worded for the rule it broke rather than for the choice.
    let reply = start_transcription(
        &router,
        &meeting_id,
        &json!({ "EngineTranscribeSettings": { "IdentifyLanguage": true, "LanguageCode": "en-US" } }),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (400, Some("BadRequestException")),
        "case \"one engine breaking its own rule\": expected the modelled 400 \
         BadRequestException, got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    let message = reply.message().unwrap_or_default();
    assert_ne!(
        message, ENGINE_CHOICE_MESSAGE,
        "case \"one engine breaking its own rule\": the exclusive-choice sentence must not be \
         served for another rule, got {message:?}"
    );
}
