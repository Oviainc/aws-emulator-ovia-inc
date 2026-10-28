// SPDX-License-Identifier: CC0-1.0

//! The WIRE FORM of every timestamp this service reports.
//!
//! RULE: a member whose model target carries `@timestampFormat("date-time")` goes onto the
//! restJson1 wire as an RFC 3339 / ISO-8601 date-time STRING — never as a bare epoch
//! number — and the instant it denotes is the same whichever operation reports it. The
//! rule is about the MEMBER, not about any one operation: it holds for every such member of
//! every shape this service serves, at every nesting depth.
//!
//! Where the catalogue comes from: every timestamp member of `com.amazonaws.ivsrealtime`
//! targets either `Time` or `ParticipantTokenExpirationTime`, and
//! `smithy_query(query="shape", shape="Composition,Destination,CompositionSummary,DestinationSummary,StageSession,StageSessionSummary,Participant,ParticipantSummary,Event,ExchangedParticipantToken,ParticipantToken")`
//! reports `"timestampFormat": "date-time"` on each of their timestamp members, whose own
//! docs say the value is "returned as a string".
//!
//! Three checks, because they can fail apart:
//!
//! * over the wire, through the same `MultiServiceRouter` the emulator binary serves, for
//!   every operation of this service that reports a timestamp (rows differing along the
//!   operation, the shape, the member name and the nesting depth);
//! * a SWEEP of every response body, which refuses a number under any of the five member
//!   names at any depth — so a member this suite does not name individually cannot slip
//!   back to the epoch form;
//! * on the member SCHEMAS, which is the one thing the JSON codec consults
//!   (`Schema::timestamp_format()`), for every `date-time` member this service declares —
//!   including `ExchangedParticipantToken`'s, which no occurrence this emulator records
//!   carries yet, so filling one in later cannot reintroduce the epoch-number form.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the token it was issued, which is
//! `state::join_stage_session` / `state::leave_stage_session`. So the session, participant
//! and event timestamps are brought about through those, reaching the store through this
//! suite's own engine handle, exactly as the `ListParticipants` and `GetStageSession`
//! suites beside it do.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::ops::types as rt;
use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region every case signs for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// Every member NAME a `@timestampFormat("date-time")` member of this service goes on the
/// wire under: `startTime`/`endTime` (`Composition`, `Destination`, `CompositionSummary`,
/// `DestinationSummary`, `StageSession`, `StageSessionSummary`), `firstJoinTime`
/// (`Participant`, `ParticipantSummary`), `eventTime` (`Event`) and `expirationTime`
/// (`ParticipantToken`, `ExchangedParticipantToken`) — the catalogue cited in the module
/// note. The sweep below refuses a number under any of them at any depth.
const DATE_TIME_MEMBERS: [&str; 5] = [
    "startTime",
    "endTime",
    "firstJoinTime",
    "eventTime",
    "expirationTime",
];

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle the data-plane seeding writes through.
struct RealTime {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in [`REGION`].
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{REGION}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation`, assert it was served, and sweep the body for a date-time member
    /// rendered as a number before handing it back.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must answer HTTP 200, got {status} {response}"
        );
        sweep(&format!("{case} / {operation}"), &response);
        response
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does.
    fn join(&self, case: &str, stage_arn: &str, participant_id: &str, published: bool) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(
                &mut store,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.to_string(),
                    published,
                    attributes: HashMap::new(),
                    ..Default::default()
                },
            )
            .is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// Record a participant leaving a session, the way the stage's data plane does.
    fn leave(&self, case: &str, stage_arn: &str, session_id: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::leave_stage_session(&mut store, stage_arn, session_id, participant_id),
            "case {case:?}: the departure of {participant_id:?} from {session_id:?} must be \
             recorded"
        );
    }
}

/// An IVS ARN of `resource_type` in [`REGION`], naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// A string member of a response, as the empty string when the wire omits it.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Assert one value is the RFC 3339 rendering of a real instant, and answer with that
/// instant.
///
/// Four things are checked, because each is a different way the member can be wrong: the
/// JSON type (a string, and in particular NOT a number — the defect this suite exists
/// for), the text (parses under `date-time`), the spelling (the canonical rendering of the
/// instant it denotes, rather than some other spelling of it) and the value (a real
/// instant, not the epoch and not a stringified epoch count).
fn assert_date_time(case: &str, value: &Value) -> DateTime {
    assert!(
        value.is_string(),
        "case {case:?}: a @timestampFormat(\"date-time\") member is a JSON string, got {value}"
    );
    assert!(
        value.as_f64().is_none(),
        "case {case:?}: a @timestampFormat(\"date-time\") member is never a JSON number, \
         got {value}"
    );
    let text = value.as_str().unwrap_or_default();
    let parsed = DateTime::from_str(text, Format::DateTime).unwrap_or_else(|error| {
        panic!("case {case:?}: {text:?} is not an RFC 3339 date-time: {error}")
    });
    let rendered = parsed
        .fmt(Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {text:?} cannot be re-rendered: {error}"));
    assert_eq!(
        rendered, text,
        "case {case:?}: the member must carry the canonical RFC 3339 rendering of its \
         instant, got {text:?}"
    );
    // 2001-09-09T01:46:40Z. Any plausible "now" is past it; epoch 0, and an epoch-seconds
    // count that was stringified rather than formatted, are not.
    assert!(
        parsed.as_secs_f64() > 1_000_000_000.0,
        "case {case:?}: expected a real instant, got {text:?} ({} s)",
        parsed.as_secs_f64()
    );
    parsed
}

/// Walk a whole response body and assert every member named in [`DATE_TIME_MEMBERS`], at
/// any depth, carries the ISO-8601 string form. Answers with the member PATHS it saw, so a
/// caller can assert the nesting it meant to exercise was really reached.
fn sweep(case: &str, body: &Value) -> BTreeSet<String> {
    let mut seen = BTreeSet::new();
    walk(case, "", body, &mut seen);
    seen
}

/// The recursive half of [`sweep`]. A list contributes one path per element position-free
/// (`destinations[].startTime`), so the paths a caller asserts do not depend on how many
/// elements a page happened to hold.
fn walk(case: &str, path: &str, value: &Value, seen: &mut BTreeSet<String>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                let child = if path.is_empty() {
                    name.clone()
                } else {
                    format!("{path}.{name}")
                };
                if DATE_TIME_MEMBERS.contains(&name.as_str()) {
                    if member.is_null() {
                        // An absent instant (`endTime` of something still running) is
                        // reported as no member at all or as null; either is not a number.
                        continue;
                    }
                    assert_date_time(&format!("{case} / {child}"), member);
                    seen.insert(child);
                } else {
                    walk(case, &child, member, seen);
                }
            }
        }
        Value::Array(elements) => {
            for element in elements {
                walk(case, &format!("{path}[]"), element, seen);
            }
        }
        _ => {}
    }
}

/// Assert every path in `expected` was swept, naming the ones that were not.
fn assert_covered(case: &str, seen: &BTreeSet<String>, expected: &[&str]) {
    let missing: Vec<&&str> = expected
        .iter()
        .filter(|path| !seen.contains(**path))
        .collect();
    assert!(
        missing.is_empty(),
        "case {case:?}: these date-time members must appear in the response, got {seen:?} \
         missing {missing:?}"
    );
}

// ---------------------------------------------------------------------------
// Over the wire: the composition times
// ---------------------------------------------------------------------------

/// `Composition`/`Destination` (`StartComposition`, `GetComposition`) and
/// `CompositionSummary`/`DestinationSummary` (`ListCompositions`) report `startTime` — and,
/// once the composition has been stopped, `endTime` — as ISO-8601 strings, at the top level
/// and nested inside a list element, and the three operations agree on the instant.
///
/// The rows differ along what the rule generalises over: how many destinations the
/// composition has and which KIND each is (a low-latency channel or an S3 recording), since
/// a destination's times are reported from its own record.
#[tokio::test]
async fn composition_date_time_members_are_iso_8601_strings() {
    let service = RealTime::new();

    let stage = service
        .ok("fixtures", "CreateStage", &json!({ "name": "timestamps" }))
        .await;
    let stage_arn = member(&stage["stage"], "arn");
    let encoder = service
        .ok(
            "fixtures",
            "CreateEncoderConfiguration",
            &json!({ "name": "enc" }),
        )
        .await;
    let encoder_arn = member(&encoder["encoderConfiguration"], "arn");
    let storage = service
        .ok(
            "fixtures",
            "CreateStorageConfiguration",
            &json!({ "s3": { "bucketName": "composition-recordings" } }),
        )
        .await;
    let storage_arn = member(&storage["storageConfiguration"], "arn");

    let channel = json!({ "channel": { "channelArn": arn("channel", "live-channel") } });
    let recording = json!({
        "s3": {
            "storageConfigurationArn": storage_arn,
            "encoderConfigurationArns": [encoder_arn],
        },
    });
    let cases: Vec<(&str, Value)> = vec![
        (
            "one channel destination",
            json!({ "stageArn": stage_arn, "destinations": [channel.clone()] }),
        ),
        (
            "one S3 recording destination",
            json!({ "stageArn": stage_arn, "destinations": [recording.clone()] }),
        ),
        (
            "both destinations the model allows at once",
            json!({
                "stageArn": stage_arn,
                "destinations": [channel.clone(), recording.clone()],
            }),
        ),
    ];

    for (case, request) in cases {
        let started = service.ok(case, "StartComposition", &request).await;
        let composition = started["composition"].clone();
        let composition_arn = member(&composition, "arn");

        let start_seen = sweep(case, &started);
        assert_covered(
            case,
            &start_seen,
            &[
                "composition.startTime",
                "composition.destinations[].startTime",
            ],
        );
        let started_at = assert_date_time(case, &composition["startTime"]);

        // The same composition through the reader and through the page: the rendering is
        // the member's, so all three carry the same instant.
        let read = service
            .ok(case, "GetComposition", &json!({ "arn": composition_arn }))
            .await;
        let read_composition = read["composition"].clone();
        assert_covered(
            case,
            &sweep(case, &read),
            &[
                "composition.startTime",
                "composition.destinations[].startTime",
            ],
        );
        assert_eq!(
            assert_date_time(case, &read_composition["startTime"]).as_nanos(),
            started_at.as_nanos(),
            "case {case:?}: GetComposition must report the instant StartComposition minted, \
             got {}",
            read_composition["startTime"]
        );

        let listed = service.ok(case, "ListCompositions", &json!({})).await;
        assert_covered(
            case,
            &sweep(case, &listed),
            &[
                "compositions[].startTime",
                "compositions[].destinations[].startTime",
            ],
        );
        let summary = listed["compositions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: compositions is @required, got {listed}"))
            .iter()
            .find(|summary| member(summary, "arn") == composition_arn)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: {composition_arn} must be listed, got {listed}")
            });
        assert_eq!(
            assert_date_time(case, &summary["startTime"]).as_nanos(),
            started_at.as_nanos(),
            "case {case:?}: the summary must report the same instant the read does, got {}",
            summary["startTime"]
        );

        // A still-composing destination reports no end: the member is absent, which is a
        // different answer from a zero epoch number.
        for (index, destination) in read_composition["destinations"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            assert!(
                destination.get("endTime").is_none_or(Value::is_null),
                "case {case:?}: destination {index} is still composing, so it reports no \
                 endTime, got {destination}"
            );
        }

        // Stopping it is what fills `endTime` in — on the composition and on every
        // destination, through the reader and through the page alike.
        service
            .ok(case, "StopComposition", &json!({ "arn": composition_arn }))
            .await;
        let stopped = service
            .ok(case, "GetComposition", &json!({ "arn": composition_arn }))
            .await;
        assert_covered(
            case,
            &sweep(case, &stopped),
            &[
                "composition.startTime",
                "composition.endTime",
                "composition.destinations[].startTime",
                "composition.destinations[].endTime",
            ],
        );
        let ended_at = assert_date_time(case, &stopped["composition"]["endTime"]);
        assert!(
            ended_at.as_nanos() >= started_at.as_nanos(),
            "case {case:?}: a composition cannot end before it began, got {ended_at:?} \
             against {started_at:?}"
        );

        let listed_after_stop = service.ok(case, "ListCompositions", &json!({})).await;
        assert_covered(
            case,
            &sweep(case, &listed_after_stop),
            &[
                "compositions[].startTime",
                "compositions[].endTime",
                "compositions[].destinations[].startTime",
                "compositions[].destinations[].endTime",
            ],
        );
    }
}

// ---------------------------------------------------------------------------
// Over the wire: the stage-session, participant, event and token times
// ---------------------------------------------------------------------------

/// `ParticipantToken$expirationTime` (`CreateParticipantToken`), `StageSession` /
/// `StageSessionSummary` `startTime`/`endTime` (`GetStageSession`, `ListStageSessions`),
/// `Participant` / `ParticipantSummary` `firstJoinTime` (`GetParticipant`,
/// `ListParticipants`) and `Event$eventTime` (`ListParticipantEvents`) are ISO-8601
/// strings, and the reader and the page agree on the instant.
///
/// The rows differ along what the rule generalises over: whether the participant published
/// to the session (which decides how many occurrences it has) and whether the token carried
/// a duration of its own (which decides where its expiry falls).
#[tokio::test]
async fn session_participant_and_token_date_time_members_are_iso_8601_strings() {
    let service = RealTime::new();
    let case = "a session two participants join";

    let stage = service
        .ok(case, "CreateStage", &json!({ "name": "session-times" }))
        .await;
    let stage_arn = member(&stage["stage"], "arn");

    // Two tokens, differing in whether the request asked for a duration: the expiry is
    // reported from the token record either way.
    let token_requests: Vec<(&str, Value, bool)> = vec![
        (
            "the modelled default duration, publishing",
            json!({ "stageArn": stage_arn }),
            true,
        ),
        (
            "a duration the request asked for, only subscribing",
            json!({ "stageArn": stage_arn, "duration": 60 }),
            false,
        ),
    ];

    let mut participants: Vec<(&str, String, bool)> = Vec::new();
    for (token_case, request, published) in token_requests {
        let issued = service
            .ok(token_case, "CreateParticipantToken", &request)
            .await;
        let token = issued["participantToken"].clone();
        assert_covered(
            token_case,
            &sweep(token_case, &issued),
            &["participantToken.expirationTime"],
        );
        assert_date_time(token_case, &token["expirationTime"]);
        participants.push((token_case, member(&token, "participantId"), published));
    }

    for (token_case, participant_id, published) in &participants {
        service.join(token_case, &stage_arn, participant_id, *published);
    }

    let stage_read = service
        .ok(case, "GetStage", &json!({ "arn": stage_arn }))
        .await;
    let session_id = member(&stage_read["stage"], "activeSessionId");
    assert_eq!(
        session_id.len(),
        16,
        "case {case:?}: the joins must have opened a session, got {stage_read}"
    );

    // The session, through the reader and through the page.
    let session_read = service
        .ok(
            case,
            "GetStageSession",
            &json!({ "stageArn": stage_arn, "sessionId": session_id }),
        )
        .await;
    assert_covered(
        case,
        &sweep(case, &session_read),
        &["stageSession.startTime"],
    );
    let session_began = assert_date_time(case, &session_read["stageSession"]["startTime"]);
    assert!(
        session_read["stageSession"]
            .get("endTime")
            .is_none_or(Value::is_null),
        "case {case:?}: an active session reports no endTime, got {session_read}"
    );

    let sessions = service
        .ok(case, "ListStageSessions", &json!({ "stageArn": stage_arn }))
        .await;
    assert_covered(
        case,
        &sweep(case, &sessions),
        &["stageSessions[].startTime"],
    );
    let session_summary = sessions["stageSessions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: stageSessions is @required, got {sessions}"))
        .iter()
        .find(|summary| member(summary, "sessionId") == session_id)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {session_id} must be listed, got {sessions}"));
    assert_eq!(
        assert_date_time(case, &session_summary["startTime"]).as_nanos(),
        session_began.as_nanos(),
        "case {case:?}: the summary must report the instant the read does, got {}",
        session_summary["startTime"]
    );

    // Each participant, through the reader, through the page, and through its occurrences.
    let listed_participants = service
        .ok(
            case,
            "ListParticipants",
            &json!({ "stageArn": stage_arn, "sessionId": session_id }),
        )
        .await;
    assert_covered(
        case,
        &sweep(case, &listed_participants),
        &["participants[].firstJoinTime"],
    );

    for (participant_case, participant_id, _) in &participants {
        let read = service
            .ok(
                participant_case,
                "GetParticipant",
                &json!({
                    "stageArn": stage_arn,
                    "sessionId": session_id,
                    "participantId": participant_id,
                }),
            )
            .await;
        assert_covered(
            participant_case,
            &sweep(participant_case, &read),
            &["participant.firstJoinTime"],
        );
        let joined_at = assert_date_time(participant_case, &read["participant"]["firstJoinTime"]);
        assert!(
            joined_at.as_nanos() >= session_began.as_nanos(),
            "case {participant_case:?}: a participant cannot join before the session it \
             opened began, got {joined_at:?} against {session_began:?}"
        );

        let summary = listed_participants["participants"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {participant_case:?}: participants is @required, got \
                     {listed_participants}"
                )
            })
            .iter()
            .find(|summary| member(summary, "participantId") == *participant_id)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {participant_case:?}: {participant_id} must be listed, got \
                     {listed_participants}"
                )
            });
        assert_eq!(
            assert_date_time(participant_case, &summary["firstJoinTime"]).as_nanos(),
            joined_at.as_nanos(),
            "case {participant_case:?}: the summary must report the instant the read does, \
             got {}",
            summary["firstJoinTime"]
        );

        let events = service
            .ok(
                participant_case,
                "ListParticipantEvents",
                &json!({
                    "stageArn": stage_arn,
                    "sessionId": session_id,
                    "participantId": participant_id,
                }),
            )
            .await;
        assert_covered(
            participant_case,
            &sweep(participant_case, &events),
            &["events[].eventTime"],
        );
        for (index, event) in events["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            let occurred_at = assert_date_time(participant_case, &event["eventTime"]);
            assert!(
                occurred_at.as_nanos() >= session_began.as_nanos(),
                "case {participant_case:?}: occurrence {index} cannot predate the session, \
                 got {occurred_at:?} against {session_began:?}"
            );
        }
    }

    // "A stage session … ends after the last participant leaves the stage": the departure
    // is what fills the session's `endTime` in, on the read and on the page alike.
    for (participant_case, participant_id, _) in &participants {
        service.leave(participant_case, &stage_arn, &session_id, participant_id);
    }

    let ended_session = service
        .ok(
            case,
            "GetStageSession",
            &json!({ "stageArn": stage_arn, "sessionId": session_id }),
        )
        .await;
    assert_covered(
        case,
        &sweep(case, &ended_session),
        &["stageSession.startTime", "stageSession.endTime"],
    );
    let session_ended = assert_date_time(case, &ended_session["stageSession"]["endTime"]);
    assert!(
        session_ended.as_nanos() >= session_began.as_nanos(),
        "case {case:?}: a session cannot end before it began, got {session_ended:?} against \
         {session_began:?}"
    );

    let sessions_after = service
        .ok(case, "ListStageSessions", &json!({ "stageArn": stage_arn }))
        .await;
    assert_covered(
        case,
        &sweep(case, &sessions_after),
        &["stageSessions[].startTime", "stageSessions[].endTime"],
    );
}

// ---------------------------------------------------------------------------
// On the member schemas
// ---------------------------------------------------------------------------

/// The format the JSON codec reads off one member schema.
fn format_of(schema: &Schema) -> Option<TimestampFormat> {
    schema.timestamp_format().map(|carried| carried.format())
}

/// EVERY `date-time` member this service declares reports `@timestampFormat("date-time")`
/// on its own member schema — the one thing the JSON codec consults when it renders a
/// timestamp (`write_timestamp` -> `Schema::timestamp_format()`), and the thing whose
/// absence made the protocol fall back to epoch seconds.
///
/// The catalogue is every timestamp member of every shape of this service's model, from the
/// `smithy_query` call cited in the module note — including
/// `ExchangedParticipantToken$expirationTime`, which only a `TOKEN_EXCHANGED` occurrence
/// carries and which no occurrence this emulator records is, so the wire tests above cannot
/// reach it.
#[test]
fn every_modelled_date_time_member_schema_carries_the_date_time_format() {
    let date_time_members: &[(&str, &Schema)] = &[
        ("Composition$startTime", &rt::Composition::M_START_TIME),
        ("Composition$endTime", &rt::Composition::M_END_TIME),
        ("Destination$startTime", &rt::Destination::M_START_TIME),
        ("Destination$endTime", &rt::Destination::M_END_TIME),
        (
            "CompositionSummary$startTime",
            &rt::CompositionSummary::M_START_TIME,
        ),
        (
            "CompositionSummary$endTime",
            &rt::CompositionSummary::M_END_TIME,
        ),
        (
            "DestinationSummary$startTime",
            &rt::DestinationSummary::M_START_TIME,
        ),
        (
            "DestinationSummary$endTime",
            &rt::DestinationSummary::M_END_TIME,
        ),
        ("StageSession$startTime", &rt::StageSession::M_START_TIME),
        ("StageSession$endTime", &rt::StageSession::M_END_TIME),
        (
            "StageSessionSummary$startTime",
            &rt::StageSessionSummary::M_START_TIME,
        ),
        (
            "StageSessionSummary$endTime",
            &rt::StageSessionSummary::M_END_TIME,
        ),
        (
            "Participant$firstJoinTime",
            &rt::Participant::M_FIRST_JOIN_TIME,
        ),
        (
            "ParticipantSummary$firstJoinTime",
            &rt::ParticipantSummary::M_FIRST_JOIN_TIME,
        ),
        ("Event$eventTime", &rt::Event::M_EVENT_TIME),
        (
            "ExchangedParticipantToken$expirationTime",
            &rt::ExchangedParticipantToken::M_EXPIRATION_TIME,
        ),
        (
            "ParticipantToken$expirationTime",
            &rt::ParticipantToken::M_EXPIRATION_TIME,
        ),
    ];

    for (case, schema) in date_time_members {
        let format = format_of(schema);
        assert_eq!(
            format,
            Some(TimestampFormat::DateTime),
            "case {case:?}: the member schema must carry the date-time format the model \
             declares, got {format:?}"
        );
    }
}
