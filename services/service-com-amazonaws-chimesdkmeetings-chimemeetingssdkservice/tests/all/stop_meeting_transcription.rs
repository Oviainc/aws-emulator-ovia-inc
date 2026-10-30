// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `StopMeetingTranscription`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! `POST /meetings/{MeetingId}/transcription?operation=stop` signed for the `chime`
//! signing name — so what is asserted is what an AWS SDK client would see, not an
//! internal return value.
//!
//! The rule under test: stopping transcription states an OUTCOME — no transcription is
//! running in the meeting it names when it returns — so it clears whatever the meeting
//! was being transcribed with, answers its modelled 200 with an empty body whether one
//! was running or not, disturbs no other meeting and no other part of the meeting it
//! names; and a `MeetingId` that names no stored meeting (never created, or already
//! deleted) is refused with the modelled `NotFoundException`, because there is no meeting
//! whose transcription the call could be about.
//!
//! The tables span the class the rule generalises over: whether a transcription was
//! running and which engine configured it, how many attendees the meeting holds, which
//! media Region it was created in, how many times the stop is repeated, how many meetings
//! are transcribing at once, and the form of the `GuidString` for the absent ids
//! (lower-case hex, upper-case hex, both numeric extremes).
//!
//! A running transcription is placed in the store directly, through this crate's `state`
//! module, because the call that starts one is a scaffold at the time of writing
//! (`StartMeetingTranscription`); the stop path under test is the whole wire path.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::ops::types::{
    EngineTranscribeMedicalSettings, EngineTranscribeSettings, TranscriptionConfiguration,
};
use service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the `chime` signing name
/// this service publishes (`OPERATIONS.md`, `## Wire protocol`). The scope is how the
/// router picks the claiming service; the signature itself is never checked here, since
/// the harness turns request authentication off.
const SIGNED_FOR_CHIME: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTCHIME/20260101/us-east-1/chime/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0000";

/// Well-formed `GuidString`s (modelled pattern
/// `^[a-fA-F0-9]{8}(?:-[a-fA-F0-9]{4}){3}-[a-fA-F0-9]{12}$`, from
/// `smithy_query(query="operation",operation="StopMeetingTranscription",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`)
/// that name no meeting in any store: the pattern's whole character class, spanning
/// lower-case hex, upper-case hex and both of its numeric extremes.
const NEVER_CREATED_IDS: &[(&str, &str)] = &[
    ("lower-case hex", "7d6c5b4a-3928-4716-8a5b-4c3d2e1f0a9b"),
    ("upper-case hex", "7D6C5B4A-3928-4716-8A5B-4C3D2E1F0A9B"),
    ("all zeroes", "00000000-0000-0000-0000-000000000000"),
    ("all f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
];

/// How a meeting that existed and has since been deleted is reported
/// (`crate::state::meeting_ended`, as `not_found_message.rs` pins it).
const ENDED_MESSAGE: &str = "The meeting has ended";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A named member of the body, as a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// The emulator under test: ONE shared engine with this service mounted on it, reached
/// the way the production router reaches it. The engine is kept beside the router so a
/// case can put a running transcription into the store the handler reads.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
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

        let response = self.router.dispatch(request).await;
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
        // restJson1 carries the wire error code in `x-amzn-errortype`; fall back to the
        // body's `__type` so a missing header never reads as "no error code".
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

    /// Create a meeting in `media_region` and return its minted `MeetingId`.
    async fn new_meeting_id(&self, token: &str, media_region: &str) -> String {
        let reply = self
            .call(
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

    async fn create_attendee(&self, meeting_id: &str, external_user_id: &str) -> Reply {
        self.call(
            "POST",
            &format!("/meetings/{meeting_id}/attendees"),
            Some(&json!({ "ExternalUserId": external_user_id })),
        )
        .await
    }

    async fn get_meeting(&self, meeting_id: &str) -> Reply {
        self.call("GET", &format!("/meetings/{meeting_id}"), None)
            .await
    }

    async fn list_attendees(&self, meeting_id: &str) -> Reply {
        self.call("GET", &format!("/meetings/{meeting_id}/attendees"), None)
            .await
    }

    async fn delete_meeting(&self, meeting_id: &str) -> Reply {
        self.call("DELETE", &format!("/meetings/{meeting_id}"), None)
            .await
    }

    async fn stop_transcription(&self, meeting_id: &str) -> Reply {
        self.call(
            "POST",
            &format!("/meetings/{meeting_id}/transcription?operation=stop"),
            None,
        )
        .await
    }

    /// Put a running transcription into the store, as the call that starts one records it.
    fn set_running_transcription(&self, meeting_id: &str, configuration: &TranscriptionConfiguration) {
        let store = state::service_state(&*self.engine).expect("the meeting store is reachable");
        let mut store = store.lock();
        let stored = store
            .meetings
            .get_mut(meeting_id)
            .unwrap_or_else(|| panic!("meeting {meeting_id} is in the store"));
        stored.transcription = Some(configuration.clone());
    }

    /// Whether the store currently holds a running transcription for this meeting.
    fn is_transcribing(&self, meeting_id: &str) -> bool {
        let store = state::service_state(&*self.engine).expect("the meeting store is reachable");
        let store = store.lock();
        store
            .meetings
            .get(meeting_id)
            .is_some_and(|stored| stored.transcription.is_some())
    }
}

/// A transcription configured with the Amazon Transcribe engine.
///
/// `language_code` is a member of `TranscribeLanguageCode` and `region` one of
/// `TranscribeRegion` (`smithy_query(query="shape",
/// shape="TranscribeLanguageCode,TranscribeRegion",
/// model_path="services/service-com-amazonaws-chimesdkmeetings-chimemeetingssdkservice/smithy-model/com-amazonaws-chimesdkmeetings-chimemeetingssdkservice.json")`).
fn transcribe(language_code: &str, region: &str) -> TranscriptionConfiguration {
    TranscriptionConfiguration {
        engine_transcribe_settings: Some(EngineTranscribeSettings {
            language_code: Some(language_code.to_string()),
            region: Some(region.to_string()),
            ..EngineTranscribeSettings::default()
        }),
        engine_transcribe_medical_settings: None,
    }
}

/// A transcription configured with the Amazon Transcribe Medical engine: `specialty` is a
/// member of `TranscribeMedicalSpecialty`, `type` of `TranscribeMedicalType` and
/// `language_code` of `TranscribeMedicalLanguageCode` (same `smithy_query(query="shape")`
/// call as [`transcribe`], on those shapes).
fn transcribe_medical(specialty: &str, kind: &str) -> TranscriptionConfiguration {
    TranscriptionConfiguration {
        engine_transcribe_settings: None,
        engine_transcribe_medical_settings: Some(EngineTranscribeMedicalSettings {
            language_code: "en-US".to_string(),
            specialty: specialty.to_string(),
            r#type: kind.to_string(),
            ..EngineTranscribeMedicalSettings::default()
        }),
    }
}

/// Assert one stop answered the modelled success: 200, no error code, and an empty body
/// (the operation's output shape is `Unit`).
fn assert_modelled_success(case: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (200, None),
        "case {case:?}: expected the modelled 200 with no error code, got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.body,
        Value::Null,
        "case {case:?}: expected an empty body, got {:?}",
        reply.body
    );
}

/// Assert one refusal is the modelled `NotFoundException` carrying `expected`.
fn assert_not_found(case: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        (reply.status, reply.error_code.as_deref()),
        (404, Some("NotFoundException")),
        "case {case:?}: expected the modelled 404 NotFoundException, got {} {:?} with {:?}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        reply.text("Code").as_deref(),
        Some("NotFound"),
        "case {case:?}: expected Code NotFound, got {:?}",
        reply.text("Code")
    );
    assert_eq!(
        reply.text("Message").as_deref(),
        Some(expected),
        "case {case:?}: expected Message {expected:?}, got {:?}",
        reply.text("Message")
    );
}

/// Stopping a meeting's transcription answers the modelled success and leaves the store
/// with no transcription running for that meeting — whether one was running or not, and
/// however many times the call is repeated. Nothing else about the meeting changes.
///
/// The rows differ along whether a transcription was running and which engine configured
/// it, how many attendees the meeting holds, its media Region, and how many stops are
/// issued.
#[tokio::test]
async fn stopping_leaves_no_transcription_running_and_changes_nothing_else() {
    // (case, attendees, media Region, the transcription running when the stop arrives,
    //  how many stops to issue)
    let cases: Vec<(&str, usize, &str, Option<TranscriptionConfiguration>, usize)> = vec![
        (
            "nothing running, no attendees",
            0,
            "us-east-1",
            None,
            1,
        ),
        (
            "nothing running, two attendees, stopped twice",
            2,
            "ap-northeast-1",
            None,
            2,
        ),
        (
            "Amazon Transcribe running, no attendees",
            0,
            "us-west-2",
            Some(transcribe("en-US", "us-west-2")),
            1,
        ),
        (
            "Amazon Transcribe running, one attendee, stopped three times",
            1,
            "eu-central-1",
            Some(transcribe("de-DE", "eu-central-1")),
            3,
        ),
        (
            "Amazon Transcribe Medical running, three attendees, stopped twice",
            3,
            "us-east-1",
            Some(transcribe_medical("PRIMARYCARE", "CONVERSATION")),
            2,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, attendee_count, media_region, running, stops)) in cases.iter().enumerate() {
        let meeting_id = emulator
            .new_meeting_id(&format!("tok-{index}"), media_region)
            .await;
        for attendee in 0..*attendee_count {
            let created = emulator
                .create_attendee(&meeting_id, &format!("user-{index}-{attendee}"))
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: CreateAttendee {attendee} expected 200, got {} with {:?}",
                created.status, created.body
            );
        }
        if let Some(configuration) = running {
            emulator.set_running_transcription(&meeting_id, configuration);
            assert!(
                emulator.is_transcribing(&meeting_id),
                "case {case:?}: the meeting must be transcribing before the stop, got \
                 {:?}",
                emulator.is_transcribing(&meeting_id)
            );
        }
        let before = emulator.get_meeting(&meeting_id).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: GetMeeting before the stop expected 200, got {} with {:?}",
            before.status, before.body
        );

        for attempt in 1..=*stops {
            let reply = emulator.stop_transcription(&meeting_id).await;
            assert_modelled_success(&format!("{case} (stop {attempt})"), &reply);
            assert!(
                !emulator.is_transcribing(&meeting_id),
                "case {case:?}: after stop {attempt} no transcription may be running, got \
                 {:?}",
                emulator.is_transcribing(&meeting_id)
            );

            // The meeting itself is untouched: the same projection, the same attendees.
            let after = emulator.get_meeting(&meeting_id).await;
            assert_eq!(
                after.body, before.body,
                "case {case:?}: after stop {attempt} GetMeeting must report the same \
                 meeting, got {:?} instead of {:?}",
                after.body, before.body
            );
            let listed = emulator.list_attendees(&meeting_id).await;
            let listed_count = listed
                .body
                .get("Attendees")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            assert_eq!(
                listed_count, *attendee_count,
                "case {case:?}: after stop {attempt} ListAttendees expected \
                 {attendee_count} attendees, got {listed_count} in {:?}",
                listed.body
            );
        }
    }
}

/// A stop reaches ONE meeting: another meeting transcribing at the same time keeps the
/// transcription it was given.
///
/// The rows differ along which of the transcribing meetings is stopped, so no row's
/// outcome can come from an ordering of the store.
#[tokio::test]
async fn stopping_one_meeting_leaves_every_other_meeting_transcribing() {
    // (case, the index of the meeting to stop)
    let cases: Vec<(&str, usize)> = vec![
        ("the first of three", 0),
        ("the middle of three", 1),
        ("the last of three", 2),
    ];

    for (case, target) in cases {
        // `ClientRequestToken` is `^[-_a-zA-Z0-9]*$`, so the token is keyed on the row's
        // index rather than its prose name.
        let emulator = Emulator::new();
        let configurations = [
            transcribe("en-US", "us-east-1"),
            transcribe_medical("RADIOLOGY", "DICTATION"),
            transcribe("ja-JP", "ap-northeast-1"),
        ];
        let mut meetings = Vec::new();
        for (index, configuration) in configurations.iter().enumerate() {
            let meeting_id = emulator
                .new_meeting_id(&format!("tok-{target}-{index}"), "us-east-1")
                .await;
            emulator.set_running_transcription(&meeting_id, configuration);
            meetings.push(meeting_id);
        }

        let reply = emulator.stop_transcription(&meetings[target]).await;
        assert_modelled_success(case, &reply);

        for (index, meeting_id) in meetings.iter().enumerate() {
            let transcribing = emulator.is_transcribing(meeting_id);
            assert_eq!(
                transcribing,
                index != target,
                "case {case:?}: meeting {index} expected transcribing={}, got \
                 {transcribing:?}",
                index != target
            );
        }
    }
}

/// A `MeetingId` that names no stored meeting is refused with the modelled
/// `NotFoundException`: there is no meeting whose transcription the call could be about,
/// so this is the one case the call does not report success for.
///
/// The rows differ along the form of the `GuidString` and along whether the id was never
/// created or named a meeting that has since been deleted — the two situations this
/// service words differently.
#[tokio::test]
async fn stopping_transcription_for_a_meeting_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();

    // A meeting that DOES exist and IS transcribing, so no refusal below can be a
    // store-wide failure and none of them may stop this meeting's transcription.
    let present = emulator.new_meeting_id("tok-present", "us-east-1").await;
    emulator.set_running_transcription(&present, &transcribe("en-GB", "eu-west-1"));

    let mut rows: Vec<(String, String, String)> = NEVER_CREATED_IDS
        .iter()
        .map(|(case, id)| {
            (
                format!("never created, {case}"),
                (*id).to_string(),
                format!("Meeting <{id}> not found"),
            )
        })
        .collect();

    // A meeting that existed, was transcribing, and was deleted: the delete ended the
    // meeting, so the stop is refused as a meeting that has ended rather than served.
    let deleted = emulator.new_meeting_id("tok-deleted", "us-west-2").await;
    emulator.set_running_transcription(&deleted, &transcribe_medical("CARDIOLOGY", "CONVERSATION"));
    let gone = emulator.delete_meeting(&deleted).await;
    assert_eq!(
        gone.status, 204,
        "DeleteMeeting on a meeting that exists: expected 204, got {} with {:?}",
        gone.status, gone.body
    );
    rows.push((
        "minted by the service, then deleted".to_string(),
        deleted,
        ENDED_MESSAGE.to_string(),
    ));

    for (case, meeting_id, expected) in &rows {
        let reply = emulator.stop_transcription(meeting_id).await;
        assert_not_found(case, &reply, expected);
    }

    // The meeting that exists kept the transcription it was given.
    assert!(
        emulator.is_transcribing(&present),
        "a refused stop must not touch another meeting's transcription, got {:?}",
        emulator.is_transcribing(&present)
    );
}
