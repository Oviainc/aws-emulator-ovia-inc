// SPDX-License-Identifier: CC0-1.0

//! End-to-end behaviour of `UpdateProfileWithAgent` over the real awsJson1_0 wire,
//! read back through `GetDataTransformationProfile`.
//!
//! The rule under test: one call serves ONE TURN of a multi-turn agent conversation
//! over a stored data transformation profile. It resolves the profile (a 404 naming
//! the profile and the DRAFT the turn resolved when no record holds it), records the
//! turn against the conversation the request names — minting a `ConversationId` when
//! it names none — and answers HTTP 200 with an `application/vnd.amazon.eventstream`
//! body: a `ProgressEvent`, then the agent's `TextMessageEvent` (or an in-band
//! `ErrorEvent` for a confirmation the conversation is not waiting for), then an
//! `EndOfOutputEvent` carrying the final response, the conversation id, and the
//! `Options` the next turn may answer with. A `confirmation_response` turn that names
//! the APPLYING option is the only turn that writes: it stamps the profile's DRAFT
//! with the agent's `ChangeDescription` and the instant it ran, and leaves the DRAFT's
//! mapping files exactly as they were.
//!
//! The rows differ along what the rule generalises over: the message `Type`, whether
//! the call carries a `ConversationId` and whether that id is one the service minted
//! or one the caller chose, the published bounds of `ConversationId` and of the
//! message `Body` (and one past each), which option a confirmation answers, how many
//! mapping files the profile holds, and its source format.
//!
//! Every call below goes through `MultiServiceRouter::dispatch` — the same entry point
//! `core/router` serves — so routing, deserialization, the modelled constraints, the
//! event-stream framing and the error envelope are all exercised, not bypassed.

use std::sync::Arc;

use aws_smithy_eventstream::frame::read_message_from;
use aws_smithy_types::event_stream::HeaderValue;
use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_healthlake_healthlake::scaffolded::wire_http_status;
use service_com_amazonaws_healthlake_healthlake::state::{
    agent_change_description, agent_nothing_to_confirm_message, agent_unoffered_option_message,
    profile_version_not_found_message, AGENT_APPLY_OPTION, AGENT_DISCARD_OPTION,
    AGENT_ERROR_VALIDATION, AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE, AGENT_MESSAGE_TYPE_NORMAL,
    DRAFT_PROFILE_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::data_transformation_profile::{mapping, mapping_of, modelled_message, reported_mapping};
use crate::fhir_datastore_lifecycle::{error_code, string_at};

/// The media type an event-stream body carries.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The three frame headers an SDK's event-stream decoder reads.
const MESSAGE_TYPE: &str = ":message-type";
const EVENT_TYPE: &str = ":event-type";

/// The `AgentOutputMessageStream` members this operation frames, as
/// `smithy_query(query="shape",shape="AgentOutputMessageStream")` names them.
const PROGRESS_EVENT: &str = "ProgressEvent";
const TEXT_MESSAGE_EVENT: &str = "TextMessageEvent";
const ERROR_EVENT: &str = "ErrorEvent";
const END_OF_OUTPUT_EVENT: &str = "EndOfOutputEvent";

/// The emulated HealthLake service behind the production router, reached with the raw
/// response so an event-stream body can be decoded frame by frame.
struct Agent {
    router: MultiServiceRouter,
}

impl Agent {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(
            service_com_amazonaws_healthlake_healthlake::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one awsJson1_0 call, returning the whole response.
    async fn call(&self, operation: &str, body: &Value) -> http::Response<Bytes> {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("HealthLake.{operation}"))
            .body(Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        self.router.dispatch(request).await
    }

    /// Dispatch a JSON call that must succeed, returning its parsed body.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let response = self.call(operation, body).await;
        let status = response.status().as_u16();
        let payload: Value =
            serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert!(
            (200..300).contains(&status),
            "case {case:?}: {operation} must succeed, got {status}: {payload}"
        );
        payload
    }

    /// Create a profile holding `content` and return its `ProfileId`.
    async fn profile(
        &self,
        case: &str,
        name: &str,
        source_format: &str,
        content: &std::collections::BTreeMap<String, String>,
    ) -> String {
        let entries: Map<String, Value> = content
            .iter()
            .map(|(path, body)| (path.clone(), Value::String(body.clone())))
            .collect();
        let created = self
            .ok(
                case,
                "CreateDataTransformationProfile",
                &json!({
                    "ProfileName": name,
                    "SourceFormat": source_format,
                    "Source": { "ProfileMapping": { "ProfileMapping": Value::Object(entries) } },
                }),
            )
            .await;
        string_at(&created, "ProfileId")
    }

    /// The DRAFT `GetDataTransformationProfile` reports for `profile_id`.
    async fn draft(&self, case: &str, profile_id: &str) -> Value {
        self.ok(
            case,
            "GetDataTransformationProfile",
            &json!({ "ProfileId": profile_id, "ProfileVersion": DRAFT_PROFILE_VERSION }),
        )
        .await
    }

    /// One agent turn that must be served: returns its decoded frames.
    async fn turn(&self, case: &str, request: &Value) -> Vec<Frame> {
        let response = self.call("UpdateProfileWithAgent", request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "case {case:?}: a served turn must answer 200, got {status}: {:?}",
            String::from_utf8_lossy(response.body())
        );
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            content_type, EVENT_STREAM,
            "case {case:?}: expected a {EVENT_STREAM} body, got {content_type:?}"
        );
        decode_frames(case, response.body())
    }

    /// One agent turn that must be refused: returns `(status, error code, body)`.
    async fn refused(&self, case: &str, request: &Value) -> (u16, String, Value) {
        let response = self.call("UpdateProfileWithAgent", request).await;
        let status = response.status().as_u16();
        let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert!(
            status >= 400,
            "case {case:?}: the turn must be refused, got {status}: {payload}"
        );
        (status, error_code(&payload), payload)
    }
}

/// One decoded frame of a response stream.
#[derive(Debug, Clone)]
struct Frame {
    event_type: String,
    payload: Value,
}

/// Decode a response body into its frames, asserting the frame headers an SDK decoder
/// requires of every one.
fn decode_frames(case: &str, body: &[u8]) -> Vec<Frame> {
    let mut buffer = Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let read = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
        };
        assert_eq!(
            read(MESSAGE_TYPE).as_deref(),
            Some("event"),
            "case {case:?}: every frame of this union is a data event, got {:?}",
            read(MESSAGE_TYPE)
        );
        let event_type = read(EVENT_TYPE)
            .unwrap_or_else(|| panic!("case {case:?}: a frame named no {EVENT_TYPE}"));
        let payload: Value = serde_json::from_slice(message.payload()).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the {event_type} payload must be JSON ({error}), got {:?}",
                String::from_utf8_lossy(message.payload())
            )
        });
        frames.push(Frame {
            event_type,
            payload,
        });
    }
    frames
}

/// The event types of a stream, in order.
fn event_types(frames: &[Frame]) -> Vec<&str> {
    frames.iter().map(|frame| frame.event_type.as_str()).collect()
}

/// The one frame of `event_type` a stream carries.
fn only(case: &str, frames: &[Frame], event_type: &str) -> Value {
    let matching: Vec<&Frame> = frames
        .iter()
        .filter(|frame| frame.event_type == event_type)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one {event_type}, got {:?}",
        event_types(frames)
    );
    matching[0].payload.clone()
}

/// A string member of a frame payload.
fn frame_string(case: &str, payload: &Value, member: &str) -> String {
    payload[member]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the frame must carry a string {member}, got {payload}")
        })
        .to_string()
}

/// The `Options` of an `EndOfOutputEvent`, or an empty list when it carries none.
fn options(payload: &Value) -> Vec<String> {
    payload["Options"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One `UpdateProfileWithAgent` request body.
fn turn_request(profile_id: &str, message_type: &str, body: &str, conversation: Option<&str>) -> Value {
    let mut request = json!({
        "ProfileId": profile_id,
        "InputMessage": { "Body": body, "Type": message_type },
    });
    if let Some(conversation) = conversation {
        request["ConversationId"] = json!(conversation);
    }
    request
}

/// One row of the turn table: how the profile is shaped and how the conversation is
/// addressed.
struct Row {
    case: &'static str,
    source_format: &'static str,
    content: std::collections::BTreeMap<String, String>,
    /// The caller's first message. The modelled `AgentMessageString` bound is
    /// 1..40960 characters, so a single character and a 40 960-character message are
    /// both at a published edge.
    body: String,
    /// The `ConversationId` the FIRST turn carries, or `None` to let the service mint
    /// one. The modelled `ConversationIdString` bound is 1..36 characters, so a
    /// one-character and a 36-character id are both at a published edge.
    conversation: Option<String>,
    /// Which option the confirmation answers, and whether it is the applying one.
    answer: &'static str,
    applies: bool,
}

/// The turn table. Every row must run the same loop: a `normal` turn offers both
/// options, and the `confirmation_response` turn that answers the APPLYING option is
/// the only one that stamps the DRAFT.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a minted conversation, the applying option, one mapping file",
            source_format: "CSV",
            content: mapping(&[("patients.csv.map", "Patient.id <- patient_id")]),
            body: "map the patient name columns".to_string(),
            conversation: None,
            answer: AGENT_APPLY_OPTION,
            applies: true,
        },
        Row {
            case: "a minted conversation, the discarding option, many mapping files",
            source_format: "CCDA",
            content: mapping_of(12),
            body: "drop the vitals section".to_string(),
            conversation: None,
            answer: AGENT_DISCARD_OPTION,
            applies: false,
        },
        Row {
            case: "a caller-chosen ConversationId at the modelled 1-character minimum",
            source_format: "CCDA",
            content: mapping(&[("sections/problems.map", "Condition.code <- problem/code")]),
            body: "c".to_string(),
            conversation: Some("c".to_string()),
            answer: AGENT_APPLY_OPTION,
            applies: true,
        },
        Row {
            case: "a caller-chosen ConversationId at the modelled 36-character maximum",
            source_format: "CSV",
            content: mapping_of(3),
            body: "b".repeat(40_960),
            conversation: Some("c".repeat(36)),
            answer: AGENT_APPLY_OPTION,
            applies: true,
        },
        Row {
            case: "the applying option answered in a different ASCII case",
            source_format: "CSV",
            content: mapping(&[("encounters.csv.map", "Encounter.id <- encounter_id")]),
            body: "normalise the encounter ids".to_string(),
            conversation: None,
            answer: "YES, RECORD THIS CHANGE ON THE DRAFT",
            applies: true,
        },
    ]
}

/// RULE, over the whole table: a `normal` turn asks and offers the options, and the
/// `confirmation_response` that answers the applying option is the one that stamps the
/// DRAFT — leaving its mapping files untouched either way.
#[tokio::test]
async fn an_agent_turn_asks_then_the_confirmed_turn_stamps_the_draft() {
    let healthlake = Agent::new();

    for row in rows() {
        let case = row.case;
        let profile_id = healthlake
            .profile(case, "agent-profile", row.source_format, &row.content)
            .await;
        let before = healthlake.draft(case, &profile_id).await;
        assert!(
            before["ChangeDescription"].is_null(),
            "case {case:?}: a created DRAFT carries no change description, got {before}"
        );
        let created_at = before["LastUpdatedAt"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: the DRAFT must report LastUpdatedAt"));

        // Turn one: the agent asks, and offers exactly the two options.
        let asked = healthlake
            .turn(
                case,
                &turn_request(
                    &profile_id,
                    AGENT_MESSAGE_TYPE_NORMAL,
                    &row.body,
                    row.conversation.as_deref(),
                ),
            )
            .await;
        assert_eq!(
            event_types(&asked),
            vec![PROGRESS_EVENT, TEXT_MESSAGE_EVENT, END_OF_OUTPUT_EVENT],
            "case {case:?}: a served turn frames progress, text and end-of-output"
        );
        let end = only(case, &asked, END_OF_OUTPUT_EVENT);
        let conversation_id = frame_string(case, &end, "ConversationId");
        if let Some(chosen) = &row.conversation {
            assert_eq!(
                &conversation_id, chosen,
                "case {case:?}: a turn that names a ConversationId keeps it"
            );
        } else {
            assert_eq!(
                conversation_id.len(),
                36,
                "case {case:?}: a minted ConversationId fills the modelled 36-character \
                 maximum, got {conversation_id:?}"
            );
        }
        assert_eq!(
            options(&end),
            vec![
                AGENT_APPLY_OPTION.to_string(),
                AGENT_DISCARD_OPTION.to_string()
            ],
            "case {case:?}: the proposal offers both options, got {end}"
        );
        let expected_question = {
            // The question is composed from the stored profile, so it is read back
            // from the service's own state rather than written out here.
            let draft = healthlake.draft(case, &profile_id).await;
            assert!(
                draft["ChangeDescription"].is_null(),
                "case {case:?}: an unanswered proposal writes nothing, got {draft}"
            );
            frame_string(case, &end, "FinalResponseMessage")
        };
        assert_eq!(
            frame_string(case, &only(case, &asked, TEXT_MESSAGE_EVENT), "Message"),
            expected_question,
            "case {case:?}: the text event carries the turn's whole answer"
        );
        assert!(
            expected_question.contains(AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE),
            "case {case:?}: the question says how to answer it, got {expected_question:?}"
        );
        assert!(
            // Only a message long enough to be distinctive proves anything here: a
            // one-character body at the modelled minimum occurs in English prose by
            // coincidence, so it is checked by the bound rows instead.
            row.body.len() < 8 || !expected_question.contains(row.body.as_str()),
            "case {case:?}: the agent must not quote the @sensitive message back"
        );

        // Turn two: the confirmation.
        let answered = healthlake
            .turn(
                case,
                &turn_request(
                    &profile_id,
                    AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
                    row.answer,
                    Some(&conversation_id),
                ),
            )
            .await;
        assert_eq!(
            event_types(&answered),
            vec![PROGRESS_EVENT, TEXT_MESSAGE_EVENT, END_OF_OUTPUT_EVENT],
            "case {case:?}: an answered turn frames progress, text and end-of-output"
        );
        let answered_end = only(case, &answered, END_OF_OUTPUT_EVENT);
        assert_eq!(
            frame_string(case, &answered_end, "ConversationId"),
            conversation_id,
            "case {case:?}: the answer stays in the same conversation"
        );
        assert!(
            options(&answered_end).is_empty(),
            "case {case:?}: an answered question offers nothing more, got {answered_end}"
        );

        let after = healthlake.draft(case, &profile_id).await;
        assert_eq!(
            reported_mapping(case, &after),
            row.content,
            "case {case:?}: an agent turn never rewrites the DRAFT's mapping files"
        );
        if row.applies {
            assert_eq!(
                after["ChangeDescription"].as_str(),
                Some(agent_change_description(&conversation_id).as_str()),
                "case {case:?}: the confirmed turn stamps the agent's change description, \
                 got {after}"
            );
            let stamped = after["LastUpdatedAt"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: the DRAFT must report LastUpdatedAt"));
            assert!(
                stamped >= created_at,
                "case {case:?}: expected LastUpdatedAt at or after the create's \
                 {created_at}, got {stamped}"
            );
        } else {
            assert!(
                after["ChangeDescription"].is_null(),
                "case {case:?}: a declined turn writes nothing, got {after}"
            );
            assert_eq!(
                after["LastUpdatedAt"].as_f64(),
                Some(created_at),
                "case {case:?}: a declined turn stamps no instant, got {after}"
            );
        }
    }
}

/// The agent's sentences are composed from the STORED profile and the STORED
/// conversation, so each turn's answer is the one its own state names — never a
/// constant independent of what is held.
#[tokio::test]
async fn the_agent_answers_from_the_stored_profile_and_conversation() {
    let healthlake = Agent::new();
    let case = "two profiles, one conversation each";

    for (name, source_format, content) in [
        ("agent-one", "CSV", mapping(&[("a.map", "Patient.id <- id")])),
        ("agent-two", "CCDA", mapping_of(5)),
    ] {
        let profile_id = healthlake
            .profile(case, name, source_format, &content)
            .await;
        let asked = healthlake
            .turn(
                case,
                &turn_request(&profile_id, AGENT_MESSAGE_TYPE_NORMAL, "please help", None),
            )
            .await;
        let end = only(case, &asked, END_OF_OUTPUT_EVENT);
        let conversation_id = frame_string(case, &end, "ConversationId");
        let question = frame_string(case, &end, "FinalResponseMessage");
        assert!(
            question.contains(name),
            "case {case:?}: the question names the profile it read, got {question:?}"
        );
        assert!(
            question.contains(&content.len().to_string()),
            "case {case:?}: the question states how many files the DRAFT holds, got \
             {question:?}"
        );
        let progress = frame_string(case, &only(case, &asked, PROGRESS_EVENT), "Message");
        assert!(
            progress.contains(&profile_id),
            "case {case:?}: the progress event names the profile it read, got {progress:?}"
        );
        assert!(
            progress.contains("Turn 1"),
            "case {case:?}: the first turn of a conversation reports itself as turn 1, got \
             {progress:?}"
        );

        // The second turn of the SAME conversation reports itself as turn 2, which is
        // what makes the conversation multi-turn rather than stateless.
        let again = healthlake
            .turn(
                case,
                &turn_request(
                    &profile_id,
                    AGENT_MESSAGE_TYPE_NORMAL,
                    "and one more thing",
                    Some(&conversation_id),
                ),
            )
            .await;
        let progress = frame_string(case, &only(case, &again, PROGRESS_EVENT), "Message");
        assert!(
            progress.contains("Turn 2"),
            "case {case:?}: the second turn of a conversation reports itself as turn 2, got \
             {progress:?}"
        );
    }
}

/// A `confirmation_response` the conversation is not waiting for is reported IN BAND:
/// the request and the profile were both fine, so the stream begins and carries an
/// `ErrorEvent` with the modelled `AgentErrorType` value instead of the agent's text —
/// and nothing is written.
#[tokio::test]
async fn an_unexpected_confirmation_is_reported_in_band() {
    let healthlake = Agent::new();
    let case = "confirmations the conversation is not waiting for";
    let content = mapping(&[("a.map", "Patient.id <- id")]);
    let profile_id = healthlake
        .profile(case, "agent-in-band", "CSV", &content)
        .await;
    let other_id = healthlake
        .profile(case, "agent-neighbour", "CSV", &content)
        .await;

    // A proposal on the neighbour profile, so the conversation IS waiting — but about
    // a different profile.
    let neighbour = healthlake
        .turn(
            case,
            &turn_request(&other_id, AGENT_MESSAGE_TYPE_NORMAL, "a change", None),
        )
        .await;
    let neighbour_conversation = frame_string(
        case,
        &only(case, &neighbour, END_OF_OUTPUT_EVENT),
        "ConversationId",
    );
    // A proposal on the profile under test, in its own conversation.
    let own = healthlake
        .turn(
            case,
            &turn_request(&profile_id, AGENT_MESSAGE_TYPE_NORMAL, "a change", None),
        )
        .await;
    let own_conversation =
        frame_string(case, &only(case, &own, END_OF_OUTPUT_EVENT), "ConversationId");

    // (row, request, the sentence the in-band error must carry, the options it must
    // re-offer)
    let offered = vec![
        AGENT_APPLY_OPTION.to_string(),
        AGENT_DISCARD_OPTION.to_string(),
    ];
    let rows: Vec<(&str, Value, String, Vec<String>)> = vec![
        (
            "a confirmation in a conversation nothing was proposed in",
            turn_request(
                &profile_id,
                AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
                AGENT_APPLY_OPTION,
                Some("a-conversation-with-no-proposal"),
            ),
            agent_nothing_to_confirm_message(&profile_id, "a-conversation-with-no-proposal"),
            Vec::new(),
        ),
        (
            "a confirmation naming a profile the proposal was not about",
            turn_request(
                &profile_id,
                AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
                AGENT_APPLY_OPTION,
                Some(&neighbour_conversation),
            ),
            agent_nothing_to_confirm_message(&profile_id, &neighbour_conversation),
            Vec::new(),
        ),
        (
            "a confirmation naming none of the options offered",
            turn_request(
                &profile_id,
                AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
                "maybe later",
                Some(&own_conversation),
            ),
            agent_unoffered_option_message(&offered),
            offered.clone(),
        ),
    ];

    for (row, request, expected, reoffered) in rows {
        let frames = healthlake.turn(row, &request).await;
        assert_eq!(
            event_types(&frames),
            vec![PROGRESS_EVENT, ERROR_EVENT, END_OF_OUTPUT_EVENT],
            "case {row:?}: an in-band failure frames an ErrorEvent instead of the text"
        );
        let error = only(row, &frames, ERROR_EVENT);
        assert_eq!(
            frame_string(row, &error, "Error"),
            AGENT_ERROR_VALIDATION,
            "case {row:?}: the in-band error names the modelled AgentErrorType, got {error}"
        );
        assert_eq!(
            frame_string(row, &error, "Message"),
            expected,
            "case {row:?}: the in-band error says why the turn could not be acted on"
        );
        let end = only(row, &frames, END_OF_OUTPUT_EVENT);
        assert_eq!(
            frame_string(row, &end, "FinalResponseMessage"),
            expected,
            "case {row:?}: the final response carries the same sentence"
        );
        assert_eq!(
            options(&end),
            reoffered,
            "case {row:?}: a conversation still waiting re-offers its options, got {end}"
        );
        let draft = healthlake.draft(row, &profile_id).await;
        assert!(
            draft["ChangeDescription"].is_null(),
            "case {row:?}: an in-band failure writes nothing, got {draft}"
        );
        assert_eq!(
            reported_mapping(row, &draft),
            content,
            "case {row:?}: an in-band failure leaves the DRAFT's files alone"
        );
    }

    // The proposal survived every in-band failure above, so answering it still works.
    let case = "the confirmation after the in-band failures";
    healthlake
        .turn(
            case,
            &turn_request(
                &profile_id,
                AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
                AGENT_APPLY_OPTION,
                Some(&own_conversation),
            ),
        )
        .await;
    let draft = healthlake.draft(case, &profile_id).await;
    assert_eq!(
        draft["ChangeDescription"].as_str(),
        Some(agent_change_description(&own_conversation).as_str()),
        "case {case:?}: the proposal is still answerable, got {draft}"
    );
}

/// Every turn this service refuses BEFORE the stream begins, and what each is refused
/// with. A row whose expected code is `None` is refused by the modelled constraints,
/// where only the status is this slice's to assert.
fn refusal_rows(profile_id: &str) -> Vec<(&'static str, Value, u16, Option<&'static str>)> {
    vec![
        (
            "a well-formed ProfileId no profile has",
            turn_request(
                "ffffffffffffffffffffffffffffffff",
                AGENT_MESSAGE_TYPE_NORMAL,
                "a change",
                None,
            ),
            wire_http_status(404),
            Some("ResourceNotFoundException"),
        ),
        (
            "a ProfileId that is not 32 hexadecimal characters",
            turn_request("not-a-profile-id", AGENT_MESSAGE_TYPE_NORMAL, "a change", None),
            400,
            None,
        ),
        (
            "a ProfileId in uppercase hexadecimal, outside the modelled pattern",
            turn_request(
                "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
                AGENT_MESSAGE_TYPE_NORMAL,
                "a change",
                None,
            ),
            400,
            None,
        ),
        (
            "no InputMessage at all, the modelled required member",
            json!({ "ProfileId": profile_id }),
            400,
            None,
        ),
        (
            "an InputMessage with no Type, the modelled required member",
            json!({ "ProfileId": profile_id, "InputMessage": { "Body": "a change" } }),
            400,
            None,
        ),
        (
            "a message Type outside the modelled AgentInputMessageType value set",
            turn_request(profile_id, "shout", "a change", None),
            400,
            None,
        ),
        (
            "an empty message Body, below the modelled 1-character minimum",
            turn_request(profile_id, AGENT_MESSAGE_TYPE_NORMAL, "", None),
            400,
            None,
        ),
        (
            "a message Body one character past the modelled 40960-character maximum",
            turn_request(
                profile_id,
                AGENT_MESSAGE_TYPE_NORMAL,
                &"b".repeat(40_961),
                None,
            ),
            400,
            None,
        ),
        (
            "an empty ConversationId, below the modelled 1-character minimum",
            turn_request(profile_id, AGENT_MESSAGE_TYPE_NORMAL, "a change", Some("")),
            400,
            None,
        ),
        (
            "a ConversationId one character past the modelled 36-character maximum",
            turn_request(
                profile_id,
                AGENT_MESSAGE_TYPE_NORMAL,
                "a change",
                Some(&"c".repeat(37)),
            ),
            400,
            None,
        ),
    ]
}

/// Every refused turn is refused with the modelled status and a JSON error envelope —
/// not an event stream — and writes nothing.
#[tokio::test]
async fn refused_turns_never_begin_a_stream() {
    let healthlake = Agent::new();
    let case = "refusals";
    let content = mapping(&[("a.map", "Patient.id <- id")]);
    let profile_id = healthlake
        .profile(case, "agent-refusals", "CSV", &content)
        .await;

    for (row, request, expected_status, expected_code) in refusal_rows(&profile_id) {
        let (status, code, payload) = healthlake.refused(row, &request).await;
        assert_eq!(
            status, expected_status,
            "case {row:?}: expected HTTP {expected_status}, got {status}: {payload}"
        );
        if let Some(expected_code) = expected_code {
            assert_eq!(
                code.as_str(),
                expected_code,
                "case {row:?}: expected {expected_code}, got {payload}"
            );
        }
        assert!(
            modelled_message(&payload).is_some(),
            "case {row:?}: a refusal carries the modelled Message member, got {payload}"
        );
        if expected_code == Some("ResourceNotFoundException") {
            let named = string_at(&request, "ProfileId");
            assert_eq!(
                modelled_message(&payload),
                Some(
                    profile_version_not_found_message(&named, DRAFT_PROFILE_VERSION).as_str()
                ),
                "case {row:?}: an unknown profile is named beside the DRAFT the turn \
                 resolved, got {payload}"
            );
        }
        let draft = healthlake.draft(row, &profile_id).await;
        assert!(
            draft["ChangeDescription"].is_null(),
            "case {row:?}: a refused turn writes nothing, got {draft}"
        );
    }

    // A turn still works after every refusal above.
    let case = "the turn after the refusals";
    let frames = healthlake
        .turn(
            case,
            &turn_request(&profile_id, AGENT_MESSAGE_TYPE_NORMAL, "a change", None),
        )
        .await;
    assert_eq!(
        event_types(&frames),
        vec![PROGRESS_EVENT, TEXT_MESSAGE_EVENT, END_OF_OUTPUT_EVENT],
        "case {case:?}: the turn after the refusals is served"
    );
}

/// Each of the three outcomes a served turn can have answers with its OWN sentence,
/// and every one of them names the profile and the version it is about — so a caller
/// reading the stream can tell the outcomes apart and sees the state each answer was
/// computed from.
#[tokio::test]
async fn the_three_turn_outcomes_answer_three_distinct_state_naming_sentences() {
    let healthlake = Agent::new();
    let case = "the proposal, declined and applied sentences";
    let name = "agent-wording";
    let content = mapping_of(4);
    let profile_id = healthlake.profile(case, name, "CCDA", &content).await;

    let asked = healthlake
        .turn(
            case,
            &turn_request(&profile_id, AGENT_MESSAGE_TYPE_NORMAL, "a change", None),
        )
        .await;
    let end = only(case, &asked, END_OF_OUTPUT_EVENT);
    let conversation_id = frame_string(case, &end, "ConversationId");
    let proposal = frame_string(case, &end, "FinalResponseMessage");

    let declined = healthlake
        .turn(
            case,
            &turn_request(
                &profile_id,
                AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
                AGENT_DISCARD_OPTION,
                Some(&conversation_id),
            ),
        )
        .await;
    let declined = frame_string(
        case,
        &only(case, &declined, END_OF_OUTPUT_EVENT),
        "FinalResponseMessage",
    );

    // Ask again in the SAME conversation, then answer the applying option.
    healthlake
        .turn(
            case,
            &turn_request(
                &profile_id,
                AGENT_MESSAGE_TYPE_NORMAL,
                "a change",
                Some(&conversation_id),
            ),
        )
        .await;
    let applied = healthlake
        .turn(
            case,
            &turn_request(
                &profile_id,
                AGENT_MESSAGE_TYPE_CONFIRMATION_RESPONSE,
                AGENT_APPLY_OPTION,
                Some(&conversation_id),
            ),
        )
        .await;
    let applied = frame_string(
        case,
        &only(case, &applied, END_OF_OUTPUT_EVENT),
        "FinalResponseMessage",
    );

    let answers = [
        ("the proposal", &proposal),
        ("the declined answer", &declined),
        ("the applied answer", &applied),
    ];
    for (step, sentence) in answers {
        assert!(
            sentence.contains(name),
            "case {case:?}: {step} must name the profile it is about, got {sentence:?}"
        );
        assert!(
            sentence.contains(&DRAFT_PROFILE_VERSION.to_string()),
            "case {case:?}: {step} must name the version it is about, got {sentence:?}"
        );
    }
    for (left_step, left) in answers {
        for (right_step, right) in answers {
            assert!(
                left_step == right_step || left != right,
                "case {case:?}: {left_step} and {right_step} must not read the same, got \
                 {left:?}"
            );
        }
    }

    // The applied answer is the one the DRAFT backs: only that turn stamped it.
    let draft = healthlake.draft(case, &profile_id).await;
    assert_eq!(
        draft["ChangeDescription"].as_str(),
        Some(agent_change_description(&conversation_id).as_str()),
        "case {case:?}: the applied turn is the one whose change the DRAFT carries, got \
         {draft}"
    );
    assert_eq!(
        reported_mapping(case, &draft),
        content,
        "case {case:?}: no turn of the conversation rewrote the DRAFT's mapping files"
    );
}
