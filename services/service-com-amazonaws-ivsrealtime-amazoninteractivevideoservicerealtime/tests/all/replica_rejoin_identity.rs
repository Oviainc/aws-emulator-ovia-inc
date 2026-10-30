// SPDX-License-Identifier: CC0-1.0

//! What a participant record reports after a SECOND join of the same participant into a
//! session that still holds its record — the rejoin path of
//! `state::join_stage_session`.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs: a
//! participant record reports the identity of the join it holds, not of the one before
//! it, so a rejoin takes over the attributes that join supplies — the whole map, since
//! every join supplies one — and the user id when it names one, exactly as it takes over
//! every other member the join carries. For a replica that means the attributes the
//! replication merged for it: `StartParticipantReplicationRequest$attributes` is
//! "application-provided attributes to set on the replicated participant in the
//! destination stage", "merged with any attributes set for this participant when creating
//! the token", with the request's values replacing the token's where the keys overlap.
//!
//! The first suite drives that through the public API alone: a replica whose replication
//! stopped leaves its destination session, but the session SURVIVES while it holds anybody
//! else, so replicating the same publisher into that stage again rejoins the record the
//! session still holds — and `GetParticipant`, the only operation that reports a
//! participant's attributes, must answer the second replication's merged map.
//!
//! The second suite reaches the same rejoin the way a stage's data plane does
//! (`state::join_stage_session` / `state::leave_stage_session` — nothing in the control
//! plane makes a participant join a stage), so the rule is pinned for a plain rejoin and
//! not only for a replication.
//!
//! Every wire case goes through the router's own `dispatch`, so what is asserted is the
//! HTTP behaviour an SDK client sees.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;
use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The two values the modelled `ParticipantState` carries, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const CONNECTED: &str = "CONNECTED";
const DISCONNECTED: &str = "DISCONNECTED";

/// One IVS RealTime emulator over one shared engine, plus the engine handle a data-plane
/// join is recorded through.
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
    /// naming `ivs`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00";
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.us-east-1.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// A stage, created the way a client creates one, reported by its ARN.
    async fn stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name:?} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// A publisher of `stage_arn` whose token carries `attributes`: the identity the model
    /// says a `participantId` comes from ("assigned by IVS and returned by
    /// `CreateParticipantToken`").
    async fn publisher(&self, case: &str, stage_arn: &str, attributes: &Value) -> String {
        let request = json!({ "stageArn": stage_arn, "attributes": attributes });
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Start one replication, asserting it was accepted.
    async fn replicate(&self, case: &str, request: &Value) {
        let (status, response) = self.call("StartParticipantReplication", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// Stop one replication, asserting the stop was accepted.
    async fn stop(&self, case: &str, request: &Value) {
        let (status, response) = self.call("StopParticipantReplication", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// The `activeSessionId` `GetStage` reports for `stage_arn`, empty when the stage has
    /// no session running.
    async fn active_session(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage on {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
    }

    /// The `Participant` `GetParticipant` reports for one participant of one session.
    async fn participant(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Value {
        let request = json!({
            "stageArn": stage_arn,
            "sessionId": session_id,
            "participantId": participant_id,
        });
        let (status, response) = self.call("GetParticipant", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetParticipant {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["participant"].clone()
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does.
    fn join(&self, case: &str, stage_arn: &str, joined: state::JoinedParticipant) {
        let participant_id = joined.participant_id.clone();
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// Record a participant leaving a session, the way the stage's data plane does.
    fn leave(&self, case: &str, stage_arn: &str, session_id: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::leave_stage_session(&mut store, stage_arn, session_id, participant_id),
            "case {case:?}: the departure of {participant_id:?} must be recorded"
        );
    }
}

/// A string member, read as the empty string when absent — what a client sees for a member
/// the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The map a JSON object holds, as a comparable map; anything that is not an object — an
/// absent member, or the `Value::Null` a row uses for "left off the request" — is the
/// empty map.
fn as_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// The `attributes` a reported participant carries: the member is omitted for a
/// participant with none, which is the empty map.
fn reported_attributes(participant: &Value) -> BTreeMap<String, String> {
    as_map(participant.get("attributes").unwrap_or(&Value::Null))
}

/// What a replica's attributes are: the publisher's token's own, with the request's over
/// them — "if there is overlap in keys, the values in these attributes are replaced".
fn merged(token: &Value, requested: &Value) -> BTreeMap<String, String> {
    let mut merged = as_map(token);
    merged.extend(as_map(requested));
    merged
}

/// The three members every replication request names, plus the `attributes` the row asks
/// for when it asks for any (`Value::Null` leaves the optional member off the request).
fn replication_request(
    source: &str,
    destination: &str,
    participant: &str,
    attributes: &Value,
) -> Value {
    let mut request = json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": participant,
    });
    if !attributes.is_null() {
        request["attributes"] = attributes.clone();
    }
    request
}

/// One row of the re-replication class: what the publisher's token carries, what the first
/// replication asked for, and what the second one asks for.
struct RereplicationCase {
    case: &'static str,
    token_attributes: Value,
    first: Value,
    second: Value,
}

/// The rows this suite spans the class with: each differs from the others in what the rule
/// generalises over — whether the second map replaces a key, names different keys
/// entirely, is empty, is absent, grows or shrinks in count, and whether the token has
/// attributes of its own for the request to be merged over.
fn rereplication_cases() -> Vec<RereplicationCase> {
    vec![
        RereplicationCase {
            case: "the same key with another value",
            token_attributes: json!({}),
            first: json!({ "seat": "left" }),
            second: json!({ "seat": "right" }),
        },
        RereplicationCase {
            case: "a disjoint key set",
            token_attributes: json!({}),
            first: json!({ "seat": "left" }),
            second: json!({ "row": "3" }),
        },
        RereplicationCase {
            case: "fewer keys than before",
            token_attributes: json!({}),
            first: json!({ "seat": "left", "row": "3", "tier": "gold" }),
            second: json!({ "tier": "silver" }),
        },
        RereplicationCase {
            case: "more keys than before",
            token_attributes: json!({}),
            first: json!({ "seat": "left" }),
            second: json!({ "seat": "aisle", "row": "12", "tier": "gold", "band": "b" }),
        },
        RereplicationCase {
            case: "an empty map, where the first replication set some",
            token_attributes: json!({}),
            first: json!({ "seat": "left" }),
            second: json!({}),
        },
        RereplicationCase {
            case: "the optional member left off the second request",
            token_attributes: json!({}),
            first: json!({ "seat": "left" }),
            second: Value::Null,
        },
        RereplicationCase {
            case: "none first, then some",
            token_attributes: json!({}),
            first: Value::Null,
            second: json!({ "seat": "left", "row": "3" }),
        },
        RereplicationCase {
            case: "merged over the token's own, replacing one of its keys",
            token_attributes: json!({ "seat": "left", "band": "a" }),
            first: json!({ "seat": "middle" }),
            second: json!({ "seat": "right" }),
        },
        RereplicationCase {
            case: "a second request that sets nothing, leaving the token's own",
            token_attributes: json!({ "band": "a" }),
            first: json!({ "seat": "left" }),
            second: json!({}),
        },
        RereplicationCase {
            case: "a longer value under the same key",
            token_attributes: json!({}),
            first: json!({ "seat": "l" }),
            second: json!({ "seat": "left-of-the-stage-and-then-some" }),
        },
    ]
}

// ---------------------------------------------------------------------------
// A re-replication into a session that still holds the replica's record
// ---------------------------------------------------------------------------

/// THE RULE, over the public API: a second `StartParticipantReplication` of the same
/// publisher into a destination stage whose session SURVIVED its first replica's departure
/// rejoins the record that session still holds, and `GetParticipant` reports the attributes
/// that second request merged — never the first one's. The publisher's own record in the
/// stage it publishes in keeps the attributes its token was issued with throughout.
#[tokio::test]
async fn a_re_replication_reports_the_attributes_it_set_on_the_replica() {
    let emulator = RealTime::new();

    for (index, row) in rereplication_cases().into_iter().enumerate() {
        let case = row.case;
        let source = emulator.stage(case, &format!("source-{index}")).await;
        let destination = emulator.stage(case, &format!("destination-{index}")).await;
        // A second publisher replicated into the same destination stage: it is what keeps
        // the destination session open when the replica under test leaves it, which is the
        // state the rejoin path is reached from.
        let keeper = emulator.publisher(case, &source, &json!({})).await;
        let publisher = emulator
            .publisher(case, &source, &row.token_attributes)
            .await;

        emulator
            .replicate(
                case,
                &replication_request(&source, &destination, &keeper, &Value::Null),
            )
            .await;
        let session = emulator.active_session(case, &destination).await;

        // The first replication: the replica joins the destination session for the first
        // time, carrying what this request merged.
        emulator
            .replicate(
                case,
                &replication_request(&source, &destination, &publisher, &row.first),
            )
            .await;
        let replica = emulator
            .participant(case, &destination, &session, &publisher)
            .await;
        let expected = merged(&row.token_attributes, &row.first);
        assert_eq!(
            reported_attributes(&replica),
            expected,
            "case {case:?}: the first replication's merged attributes must be reported, \
             expected {expected:?}, got {replica}"
        );

        // The replica leaves, and the destination session survives because the keeper is
        // still in it — so the record the next replication finds is the one this stop left
        // behind.
        emulator
            .stop(
                case,
                &replication_request(&source, &destination, &publisher, &Value::Null),
            )
            .await;
        let after_stop = emulator.active_session(case, &destination).await;
        assert_eq!(
            after_stop, session,
            "case {case:?}: the destination session must survive the replica's departure, \
             expected {session:?}, got {after_stop:?}"
        );
        let departed = emulator
            .participant(case, &destination, &session, &publisher)
            .await;
        assert_eq!(
            member(&departed, "state"),
            DISCONNECTED,
            "case {case:?}: the stopped replica must have left the destination session, \
             got {departed}"
        );

        // The re-replication, into that same session: its attributes are what the replica
        // reports from now on.
        emulator
            .replicate(
                case,
                &replication_request(&source, &destination, &publisher, &row.second),
            )
            .await;
        let rejoined_session = emulator.active_session(case, &destination).await;
        assert_eq!(
            rejoined_session, session,
            "case {case:?}: the re-replication must rejoin the surviving session, \
             expected {session:?}, got {rejoined_session:?}"
        );
        let replica = emulator
            .participant(case, &destination, &session, &publisher)
            .await;
        assert_eq!(
            member(&replica, "state"),
            CONNECTED,
            "case {case:?}: the re-replicated participant must be back in the destination \
             session, got {replica}"
        );
        let expected = merged(&row.token_attributes, &row.second);
        assert_eq!(
            reported_attributes(&replica),
            expected,
            "case {case:?}: the re-replication's merged attributes must be reported, \
             expected {expected:?}, got {replica}"
        );

        // The publisher end of the same replication carries the identity its token was
        // issued with, which no replication's attributes replace.
        let source_session = emulator.active_session(case, &source).await;
        let publishing = emulator
            .participant(case, &source, &source_session, &publisher)
            .await;
        let expected = as_map(&row.token_attributes);
        assert_eq!(
            reported_attributes(&publishing),
            expected,
            "case {case:?}: the publisher keeps its token's attributes, \
             expected {expected:?}, got {publishing}"
        );
    }
}

// ---------------------------------------------------------------------------
// The same rejoin, over a stage's data plane
// ---------------------------------------------------------------------------

/// One row of the plain-rejoin class: the identity each of the two joins carries.
struct RejoinCase {
    case: &'static str,
    first_user_id: Option<&'static str>,
    first_attributes: &'static [(&'static str, &'static str)],
    second_user_id: Option<&'static str>,
    second_attributes: &'static [(&'static str, &'static str)],
    /// What the record must report after the second join: the attributes it carried, and
    /// the user id it named or — the optional-member rule the rest of a rejoin follows —
    /// the earlier one when it named none.
    expected_user_id: &'static str,
}

/// THE SAME RULE, reached the way a stage's data plane reaches it: a participant that left
/// a session and joins it again reports the attributes of the join it holds, and the user
/// id that join names.
#[tokio::test]
async fn a_rejoin_reports_the_identity_the_join_carried() {
    let rows = [
        RejoinCase {
            case: "another value under the same key, and another user id",
            first_user_id: Some("ann"),
            first_attributes: &[("seat", "left")],
            second_user_id: Some("bo"),
            second_attributes: &[("seat", "right")],
            expected_user_id: "bo",
        },
        RejoinCase {
            case: "no attributes and no user id on the second join",
            first_user_id: Some("ann"),
            first_attributes: &[("seat", "left"), ("row", "3")],
            second_user_id: None,
            second_attributes: &[],
            expected_user_id: "ann",
        },
        RejoinCase {
            case: "no identity at all first, then a full one",
            first_user_id: None,
            first_attributes: &[],
            second_user_id: Some("cy"),
            second_attributes: &[("seat", "aisle"), ("row", "12"), ("tier", "gold")],
            expected_user_id: "cy",
        },
        RejoinCase {
            case: "a disjoint key set under an unchanged user id",
            first_user_id: Some("dee"),
            first_attributes: &[("a", "1")],
            second_user_id: Some("dee"),
            second_attributes: &[("b", "2")],
            expected_user_id: "dee",
        },
    ];

    let emulator = RealTime::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let stage = emulator.stage(case, &format!("rejoin-{index}")).await;
        let subject = format!("subject-{index}");
        // Somebody else in the session, so the session survives the subject's departure:
        // "a stage session ends after the last participant leaves".
        emulator.join(
            case,
            &stage,
            state::JoinedParticipant {
                participant_id: format!("keeper-{index}"),
                ..Default::default()
            },
        );
        let session = emulator.active_session(case, &stage).await;

        emulator.join(
            case,
            &stage,
            state::JoinedParticipant {
                participant_id: subject.clone(),
                user_id: row.first_user_id.map(str::to_string),
                attributes: attributes_of(row.first_attributes),
                ..Default::default()
            },
        );
        emulator.leave(case, &stage, &session, &subject);
        emulator.join(
            case,
            &stage,
            state::JoinedParticipant {
                participant_id: subject.clone(),
                user_id: row.second_user_id.map(str::to_string),
                attributes: attributes_of(row.second_attributes),
                ..Default::default()
            },
        );

        let participant = emulator.participant(case, &stage, &session, &subject).await;
        let expected: BTreeMap<String, String> = row
            .second_attributes
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_attributes(&participant),
            expected,
            "case {case:?}: a rejoin reports the attributes it carried, \
             expected {expected:?}, got {participant}"
        );
        assert_eq!(
            member(&participant, "userId"),
            row.expected_user_id,
            "case {case:?}: a rejoin reports the user id it named, \
             expected {:?}, got {participant}",
            row.expected_user_id
        );
        assert_eq!(
            member(&participant, "state"),
            CONNECTED,
            "case {case:?}: the rejoined participant is in the session, got {participant}"
        );
    }
}

/// The attributes a join carries, as the state struct holds them.
fn attributes_of(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}
