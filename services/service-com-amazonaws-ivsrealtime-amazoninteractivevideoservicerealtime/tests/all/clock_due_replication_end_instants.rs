// SPDX-License-Identifier: CC0-1.0

//! WHEN a replication the clock ended is reported to have ended, and when everything that
//! end drags with it is reported to have happened.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status and the JSON body — never an internal
//! call. The one exception is the stage's DATA plane, which no operation of this control
//! plane serves: a participant reaches a stage with the token it was issued
//! (`state::join_stage_session`) and goes when its client drops off
//! (`state::leave_stage_session`), the seam the `DisconnectParticipant` and
//! `ListParticipants` suites beside this one reach through. That seam takes the instant the
//! connection changed at, which is how a case puts a departure a whole window in the PAST
//! without waiting one out.
//!
//! The RULE under test, exercised over a SET of inputs rather than on one request: a change
//! this service owes to the CLOCK is recorded as having happened AT the instant it fell due,
//! never at the instant of the call that happens to materialise it — so every value a caller
//! reads is a function of the recorded instants alone, and a caller reading late is told
//! exactly what one reading on time was told. For the end of a replication whose publisher
//! stayed away for its whole `reconnectWindowSeconds` that means:
//!
//! 1. the publisher's `REPLICATION_STOPPED` occurrence carries `eventTime` = the deadline;
//! 2. the replica's departure from the destination stage — its `PUBLISH_STOPPED` and `LEFT`
//!    occurrences, and the `endTime` of a destination session it emptied, which "ends after
//!    the last participant leaves the stage" — carries that same instant;
//! 3. a composition of a destination stage that departure left empty counts the published
//!    `COMPOSITION_EMPTY_STAGE_TIMEOUT` from that instant, so one that ran out before the
//!    read has already ended, with the `endTime` that deadline earned it.
//!
//! The departure of the publisher itself is recorded at the instant it happened for the same
//! reason, which case 1 asserts beside the end it earned.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region every case in this suite signs for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, and so the one every ARN built by
/// hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The two `ReplicationState` values, from
/// `smithy_query(query="shape",shape="ReplicationState")`.
const ACTIVE: &str = "ACTIVE";
const STOPPED: &str = "STOPPED";

/// The two `ParticipantState` values, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const DISCONNECTED: &str = "DISCONNECTED";

/// The occurrence names this suite asserts, from
/// `smithy_query(query="shape",shape="EventName")`.
const EVENT_REPLICATION_STOPPED: &str = "REPLICATION_STOPPED";
const EVENT_PUBLISH_STOPPED: &str = "PUBLISH_STOPPED";
const EVENT_LEFT: &str = "LEFT";

/// The `CompositionState` values this suite asserts, from
/// `smithy_query(query="shape",shape="CompositionState")`.
const COMPOSITION_STARTING: &str = "STARTING";
const COMPOSITION_ACTIVE: &str = "ACTIVE";
const COMPOSITION_STOPPING: &str = "STOPPING";
const COMPOSITION_STOPPED: &str = "STOPPED";

/// How long after its stage was left empty a composition's end begins, read from the crate
/// so this suite cannot drift from the span the service enforces — one whole end inside the
/// minute the model publishes (`state::COMPOSITION_EMPTY_STAGE_TIMEOUT`).
const STOP_DUE_AFTER: Duration = state::COMPOSITION_EMPTY_STAGE_STOP_SPAN;

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it,
/// plus the engine handle the data-plane connections are recorded through.
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
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `operation` with the modelled success asserted, answering the body.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} {body} must answer HTTP 200, got {status} {response}"
        );
        response
    }

    /// Create a stage and answer its ARN.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let response = self
            .ok(case, "CreateStage", &json!({ "name": name }))
            .await;
        member(&response["stage"], "arn")
    }

    /// Create an encoder configuration and answer its ARN.
    async fn create_encoder_configuration(&self, case: &str) -> String {
        let response = self
            .ok(case, "CreateEncoderConfiguration", &json!({ "name": "enc" }))
            .await;
        member(&response["encoderConfiguration"], "arn")
    }

    /// Issue a participant token for `stage_arn` and answer the participant id it carries.
    async fn create_token(&self, case: &str, stage_arn: &str) -> String {
        let response = self
            .ok(
                case,
                "CreateParticipantToken",
                &json!({ "stageArn": stage_arn }),
            )
            .await;
        member(&response["participantToken"], "participantId")
    }

    /// Record a participant joining `stage_arn` at this instant, the way the stage's data
    /// plane does.
    fn join(&self, case: &str, stage_arn: &str, participant_id: &str, published: bool) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let joined = state::JoinedParticipant {
            participant_id: participant_id.to_string(),
            published,
            ..Default::default()
        };
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// Record a participant's client dropping off the data plane of `stage_arn` AT `at` —
    /// which is how a case observes a departure that happened a whole window ago without
    /// waiting that window out.
    fn leave(&self, case: &str, stage_arn: &str, session_id: &str, id: &str, at: SystemTime) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        assert!(
            state::leave_stage_session_at(&mut store, stage_arn, session_id, id, at),
            "case {case:?}: the departure of {id:?} from {session_id:?} must be recorded"
        );
    }

    /// The `activeSessionId` `GetStage` reports, the empty string once the stage has no
    /// session running.
    async fn active_session(&self, case: &str, stage_arn: &str) -> String {
        let response = self
            .ok(case, "GetStage", &json!({ "arn": stage_arn }))
            .await;
        member(&response["stage"], "activeSessionId")
    }

    /// Replicate the publisher `participant_id` of `source` into `destination`, with the
    /// reconnect window the case asked for when it asked for one.
    async fn replicate(
        &self,
        case: &str,
        source: &str,
        destination: &str,
        participant_id: &str,
        window: Option<i64>,
    ) {
        let mut request = replication_request(source, destination, participant_id);
        if let Some(window) = window {
            request["reconnectWindowSeconds"] = json!(window);
        }
        self.ok(case, "StartParticipantReplication", &request).await;
    }

    /// `StartComposition` against `stage_arn` with one destination naming `encoder`, and
    /// answer the composition's ARN.
    async fn start_composition(&self, case: &str, stage_arn: &str, encoder: &str) -> String {
        let request = json!({
            "stageArn": stage_arn,
            "destinations": [{
                "channel": {
                    "channelArn": arn("channel", "live"),
                    "encoderConfigurationArn": encoder,
                },
            }],
        });
        let response = self.ok(case, "StartComposition", &request).await;
        member(&response["composition"], "arn")
    }

    /// The occurrences `ListParticipantEvents` reports for one participant of one session.
    async fn events(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Vec<Value> {
        let request = json!({
            "stageArn": stage_arn,
            "sessionId": session_id,
            "participantId": participant_id,
        });
        let response = self.ok(case, "ListParticipantEvents", &request).await;
        response["events"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: events is @required, got {response}"))
            .clone()
    }

    /// The `StageSession` `GetStageSession` reports.
    async fn stage_session(&self, case: &str, stage_arn: &str, session_id: &str) -> Value {
        let request = json!({ "stageArn": stage_arn, "sessionId": session_id });
        let response = self.ok(case, "GetStageSession", &request).await;
        response["stageSession"].clone()
    }

    /// The summary `ListStageSessions` reports for one session of `stage_arn`.
    async fn listed_session(&self, case: &str, stage_arn: &str, session_id: &str) -> Value {
        let response = self
            .ok(case, "ListStageSessions", &json!({ "stageArn": stage_arn }))
            .await;
        response["stageSessions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|session| member(session, "sessionId") == session_id)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {stage_arn} must list the session {session_id}")
            })
    }

    /// The `replicationState` `ListParticipantReplicas` reports for one replication.
    async fn replication_state(
        &self,
        case: &str,
        source: &str,
        participant_id: &str,
        destination: &str,
    ) -> String {
        let request = json!({ "sourceStageArn": source, "participantId": participant_id });
        let response = self.ok(case, "ListParticipantReplicas", &request).await;
        let replicas = response["replicas"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|replica| member(replica, "destinationStageArn") == destination);
        member(
            &replicas.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {source} must hold the replication of {participant_id} \
                     into {destination}, got {response}"
                )
            }),
            "replicationState",
        )
    }

    /// The `state` `GetParticipant` reports for one participant of one session.
    async fn participant_state(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> String {
        let request = json!({
            "stageArn": stage_arn,
            "sessionId": session_id,
            "participantId": participant_id,
        });
        let response = self.ok(case, "GetParticipant", &request).await;
        member(&response["participant"], "state")
    }

    /// `GetComposition`, whose read of a composition the clock reclaimed is a 404.
    async fn composition(&self, case: &str, arn: &str) -> Value {
        let response = self.ok(case, "GetComposition", &json!({ "arn": arn })).await;
        response["composition"].clone()
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

/// An IVS ARN of `resource_type` in [`REGION`], naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// The three members both ends of a replication name it by.
fn replication_request(source: &str, destination: &str, participant_id: &str) -> Value {
    json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": participant_id,
    })
}

/// Seconds since the epoch of an instant, the unit every assertion here compares in.
fn epoch_secs(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The epoch-second instant a modelled timestamp member denotes.
///
/// Every instant this service reports targets a shape carrying
/// `@timestampFormat("date-time")`, so the rendering on its restJson1 wire IS the ISO-8601
/// string and a bare epoch number is not a timestamp this suite accepts;
/// `tests/all/date_time_members_wire_form.rs` is where that rendering is pinned.
fn timestamp_secs(case: &str, what: &str, value: &Value) -> i64 {
    let rendered = value.as_str().unwrap_or_else(|| {
        panic!("case {case:?}: {what} must be an ISO-8601 timestamp string, got {value}")
    });
    DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!("case {case:?}: {what} {rendered:?} is not an ISO-8601 instant: {error}")
        })
        .secs()
}

/// The one occurrence of `name` among `events`, with `destinationStageArn` narrowing it to
/// the end of one replication when the case replicates into more than one stage.
fn occurrence(case: &str, events: &[Value], name: &str, destination: Option<&str>) -> Value {
    let matching: Vec<&Value> = events
        .iter()
        .filter(|event| member(event, "name") == name)
        .filter(|event| {
            destination.is_none_or(|stage| member(event, "destinationStageArn") == stage)
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: exactly one {name} occurrence must be reported for \
         {destination:?}, got {events:?}"
    );
    matching[0].clone()
}

/// Assert a reported instant is the one the clock made the change due at, and so is NOT the
/// instant of the call that materialised it.
fn assert_reported_at(case: &str, what: &str, value: &Value, due: SystemTime, call: SystemTime) {
    let reported = timestamp_secs(case, what, value);
    assert_eq!(
        reported,
        epoch_secs(due),
        "case {case:?}: {what} must be the instant the change fell due ({}), \
         got {reported} (the materialising call ran at {})",
        epoch_secs(due),
        epoch_secs(call)
    );
    assert!(
        reported < epoch_secs(call) - 1,
        "case {case:?}: {what} must be strictly earlier than the call that materialised \
         it ({}), got {reported}",
        epoch_secs(call)
    );
}

// ---------------------------------------------------------------------------
// Rules 1 and 2: the end, and the replica's departure, are stamped at the deadline
// ---------------------------------------------------------------------------

/// Which reporting operation the case lets materialise the end the clock already owed.
///
/// Each is `@readonly` and each runs the replication clock, so the end is written down by
/// whichever of them a client happens to call first.
#[derive(Clone, Copy, Debug)]
enum Materialiser {
    ListParticipantEvents,
    GetStage,
    GetStageSession,
    ListStageSessions,
    ListParticipantReplicas,
}

/// One row of the class "a replication's end fell due while nobody was calling". The rows
/// differ in what the rule generalises over: how long ago the end fell due, the reconnect
/// window the start asked for (so a differently-computed deadline), how many destination
/// stages the publisher is replicated into, whether a destination stage holds a participant
/// of its own (so whether the replica's departure ends that session), and which reporting
/// operation materialises the end.
#[derive(Clone, Copy, Debug)]
struct Case {
    case: &'static str,
    /// How long before the materialising call the publisher's client dropped off.
    gone_for: Duration,
    /// `reconnectWindowSeconds`, or `None` to omit it — the default the model documents
    /// ("Default: 0"). Always shorter than [`Self::gone_for`], so the end is already owed.
    window: Option<i64>,
    /// How many destination stages the publisher is replicated into.
    destinations: usize,
    /// Whether each destination stage holds a participant that joined it in its own right.
    local_participant: bool,
    materialiser: Materialiser,
}

const CASES: &[Case] = &[
    Case {
        case: "default window, read two seconds late",
        gone_for: Duration::from_secs(2),
        window: None,
        destinations: 1,
        local_participant: false,
        materialiser: Materialiser::ListParticipantEvents,
    },
    Case {
        case: "window of zero, read a minute and a half late",
        gone_for: Duration::from_secs(90),
        window: Some(0),
        destinations: 1,
        local_participant: false,
        materialiser: Materialiser::GetStageSession,
    },
    Case {
        case: "short window, read long after it ran out",
        gone_for: Duration::from_secs(30),
        window: Some(5),
        destinations: 1,
        local_participant: false,
        materialiser: Materialiser::ListStageSessions,
    },
    Case {
        case: "the longest window the model accepts, run out",
        gone_for: Duration::from_secs(600),
        window: Some(300),
        destinations: 1,
        local_participant: false,
        materialiser: Materialiser::GetStage,
    },
    Case {
        case: "two destination stages",
        gone_for: Duration::from_secs(45),
        window: None,
        destinations: 2,
        local_participant: false,
        materialiser: Materialiser::ListParticipantReplicas,
    },
    Case {
        case: "destination stage keeps a participant of its own",
        gone_for: Duration::from_secs(45),
        window: None,
        destinations: 1,
        local_participant: true,
        materialiser: Materialiser::ListParticipantEvents,
    },
];

/// The whole fixture of one row: the stages, the sessions and the publisher, with the
/// publisher's client already gone and its end already owed but unmaterialised.
struct Fixture {
    source: String,
    source_session: String,
    publisher: String,
    destinations: Vec<String>,
    destination_sessions: Vec<String>,
    /// The instant the publisher's client dropped off.
    left_at: SystemTime,
    /// The instant each replication's end fell due: the departure plus its window.
    due_at: SystemTime,
}

/// Build the fixture of `case`: a publisher replicated into every destination stage, then
/// taken off the data plane `gone_for` ago, with NO call in between.
async fn seed(emulator: &RealTime, case: &Case) -> Fixture {
    let window = case.window.unwrap_or(0);
    assert!(
        case.gone_for > Duration::from_secs(window as u64),
        "case {:?}: the row must leave the publisher away for longer than its window",
        case.case
    );

    let source = emulator.create_stage(case.case, "source").await;
    // A bystander keeps the source session open, so the publisher's own departure is not
    // also the end of the session its occurrences are read from.
    let bystander = emulator.create_token(case.case, &source).await;
    emulator.join(case.case, &source, &bystander, false);
    let publisher = emulator.create_token(case.case, &source).await;
    emulator.join(case.case, &source, &publisher, true);
    let source_session = emulator.active_session(case.case, &source).await;

    let mut destinations = Vec::new();
    let mut destination_sessions = Vec::new();
    for index in 0..case.destinations {
        let destination = emulator
            .create_stage(case.case, &format!("destination-{index}"))
            .await;
        if case.local_participant {
            let local = emulator.create_token(case.case, &destination).await;
            emulator.join(case.case, &destination, &local, false);
        }
        emulator
            .replicate(
                case.case,
                &source,
                &destination,
                &publisher,
                case.window,
            )
            .await;
        let session = emulator.active_session(case.case, &destination).await;
        assert_eq!(
            emulator
                .replication_state(case.case, &source, &publisher, &destination)
                .await,
            ACTIVE,
            "case {:?}: a replication that has just started is {ACTIVE}",
            case.case
        );
        destinations.push(destination);
        destination_sessions.push(session);
    }

    // The publisher's client dropped off `gone_for` ago, and nothing has called since.
    let left_at = SystemTime::now() - case.gone_for;
    emulator.leave(
        case.case,
        &source,
        &source_session,
        &publisher,
        left_at,
    );
    let due_at = left_at + Duration::from_secs(window as u64);
    Fixture {
        source,
        source_session,
        publisher,
        destinations,
        destination_sessions,
        left_at,
        due_at,
    }
}

/// Let `case`'s chosen reporting operation materialise what the clock owes, and answer the
/// instant that call was made at.
async fn materialise(emulator: &RealTime, case: &Case, fixture: &Fixture) -> SystemTime {
    let at = SystemTime::now();
    match case.materialiser {
        Materialiser::ListParticipantEvents => {
            emulator
                .events(
                    case.case,
                    &fixture.source,
                    &fixture.source_session,
                    &fixture.publisher,
                )
                .await;
        }
        Materialiser::GetStage => {
            emulator.active_session(case.case, &fixture.source).await;
        }
        Materialiser::GetStageSession => {
            emulator
                .stage_session(case.case, &fixture.source, &fixture.source_session)
                .await;
        }
        Materialiser::ListStageSessions => {
            emulator
                .listed_session(case.case, &fixture.source, &fixture.source_session)
                .await;
        }
        Materialiser::ListParticipantReplicas => {
            emulator
                .replication_state(
                    case.case,
                    &fixture.source,
                    &fixture.publisher,
                    &fixture.destinations[0],
                )
                .await;
        }
    }
    at
}

#[tokio::test]
async fn clock_due_replication_end_is_stamped_at_the_deadline_not_at_the_late_read() {
    for case in CASES {
        let emulator = RealTime::new();
        let fixture = seed(&emulator, case).await;
        let call_at = materialise(&emulator, case, &fixture).await;

        // Rule 1: the publisher's own departure, and the end of each replication it
        // sourced, are reported at the instants they happened.
        let source_events = emulator
            .events(
                case.case,
                &fixture.source,
                &fixture.source_session,
                &fixture.publisher,
            )
            .await;
        assert_reported_at(
            case.case,
            "the publisher's PUBLISH_STOPPED eventTime",
            &occurrence(case.case, &source_events, EVENT_PUBLISH_STOPPED, None)["eventTime"],
            fixture.left_at,
            call_at,
        );
        assert_reported_at(
            case.case,
            "the publisher's LEFT eventTime",
            &occurrence(case.case, &source_events, EVENT_LEFT, None)["eventTime"],
            fixture.left_at,
            call_at,
        );

        for (index, destination) in fixture.destinations.iter().enumerate() {
            let session = &fixture.destination_sessions[index];
            assert_eq!(
                emulator
                    .replication_state(case.case, &fixture.source, &fixture.publisher, destination)
                    .await,
                STOPPED,
                "case {:?}: the replication into {destination} is over, so it reports \
                 {STOPPED}",
                case.case
            );
            assert_reported_at(
                case.case,
                "the REPLICATION_STOPPED eventTime",
                &occurrence(
                    case.case,
                    &source_events,
                    EVENT_REPLICATION_STOPPED,
                    Some(destination),
                )["eventTime"],
                fixture.due_at,
                call_at,
            );

            // Rule 2: the replica left the destination stage when the end fell due, so its
            // own occurrences there carry that instant and the record is DISCONNECTED.
            let replica_events = emulator
                .events(case.case, destination, session, &fixture.publisher)
                .await;
            for name in [EVENT_PUBLISH_STOPPED, EVENT_LEFT] {
                assert_reported_at(
                    case.case,
                    &format!("the replica's {name} eventTime"),
                    &occurrence(case.case, &replica_events, name, None)["eventTime"],
                    fixture.due_at,
                    call_at,
                );
            }
            assert_eq!(
                emulator
                    .participant_state(case.case, destination, session, &fixture.publisher)
                    .await,
                DISCONNECTED,
                "case {:?}: the replica of {} is out of {destination}, so it reports \
                 {DISCONNECTED}",
                case.case,
                fixture.publisher
            );

            // And a session the replica emptied "ends after the last participant leaves
            // the stage", which was that same instant — reported identically by the read
            // and by the listing. A session still holding a participant of its own has
            // not ended at all.
            let read = emulator.stage_session(case.case, destination, session).await;
            let listed = emulator.listed_session(case.case, destination, session).await;
            if case.local_participant {
                for (what, reported) in [("GetStageSession", &read), ("ListStageSessions", &listed)]
                {
                    assert!(
                        reported["endTime"].is_null(),
                        "case {:?}: {destination} still holds a participant of its own, so \
                         {what} reports no endTime, got {reported}",
                        case.case
                    );
                }
            } else {
                assert_reported_at(
                    case.case,
                    "the destination session's endTime (GetStageSession)",
                    &read["endTime"],
                    fixture.due_at,
                    call_at,
                );
                assert_reported_at(
                    case.case,
                    "the destination session's endTime (ListStageSessions)",
                    &listed["endTime"],
                    fixture.due_at,
                    call_at,
                );
                assert_eq!(
                    emulator.active_session(case.case, destination).await,
                    "",
                    "case {:?}: {destination} was emptied by the replica leaving, so it \
                     reports no active session",
                    case.case
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the empty-stage timeout of the destination runs from that same deadline
// ---------------------------------------------------------------------------

/// One row of the class "a composition's stage was emptied by a replication end the clock
/// owed". The rows differ in whether that end fell due long enough ago for the published
/// timeout to have run out as well, and in whether the departure emptied the stage at all.
#[derive(Clone, Copy, Debug)]
struct CompositionCase {
    case: &'static str,
    /// How long before the materialising call the publisher's client dropped off, and so —
    /// the window being the documented default of 0 — how long ago the end fell due.
    gone_for: Duration,
    /// Whether the destination stage holds a participant of its own, so the replica
    /// leaving does not empty it.
    local_participant: bool,
    /// Whether the composition's end has already begun by the time of the read.
    ended: bool,
}

const COMPOSITION_CASES: &[CompositionCase] = &[
    CompositionCase {
        case: "end fell due well inside the published timeout",
        gone_for: Duration::from_secs(10),
        local_participant: false,
        ended: false,
    },
    CompositionCase {
        case: "end fell due long enough ago for the timeout to have run out",
        gone_for: Duration::from_secs(50),
        local_participant: false,
        ended: true,
    },
    CompositionCase {
        case: "end fell due even longer ago",
        gone_for: Duration::from_secs(55),
        local_participant: false,
        ended: true,
    },
    CompositionCase {
        case: "the stage was never emptied, so nothing is owed",
        gone_for: Duration::from_secs(50),
        local_participant: true,
        ended: false,
    },
];

#[tokio::test]
async fn destination_empty_stage_timeout_runs_from_the_deadline_the_end_fell_due_at() {
    for composition_case in COMPOSITION_CASES {
        let case = Case {
            case: composition_case.case,
            gone_for: composition_case.gone_for,
            window: None,
            destinations: 1,
            local_participant: composition_case.local_participant,
            materialiser: Materialiser::GetStage,
        };
        let emulator = RealTime::new();

        // The composition is started while the replica is still in the destination stage,
        // so nothing is owed it until that replica goes.
        let source = emulator.create_stage(case.case, "source").await;
        let bystander = emulator.create_token(case.case, &source).await;
        emulator.join(case.case, &source, &bystander, false);
        let publisher = emulator.create_token(case.case, &source).await;
        emulator.join(case.case, &source, &publisher, true);
        let source_session = emulator.active_session(case.case, &source).await;
        let destination = emulator.create_stage(case.case, "destination").await;
        if case.local_participant {
            let local = emulator.create_token(case.case, &destination).await;
            emulator.join(case.case, &destination, &local, false);
        }
        emulator
            .replicate(case.case, &source, &destination, &publisher, None)
            .await;
        let encoder = emulator.create_encoder_configuration(case.case).await;
        let composition = emulator
            .start_composition(case.case, &destination, &encoder)
            .await;

        let left_at = SystemTime::now() - case.gone_for;
        emulator.leave(case.case, &source, &source_session, &publisher, left_at);

        // The one call that materialises everything the clock owed, late.
        let call_at = SystemTime::now();
        emulator.active_session(case.case, &source).await;
        let reported = emulator.composition(case.case, &composition).await;
        let state = member(&reported, "state");
        if composition_case.ended {
            assert!(
                state == COMPOSITION_STOPPING || state == COMPOSITION_STOPPED,
                "case {:?}: the stage was left empty at {} and the timeout ran out before \
                 this read, so the composition's end has begun, got {reported}",
                case.case,
                epoch_secs(left_at)
            );
            assert_reported_at(
                case.case,
                "the composition's endTime",
                &reported["endTime"],
                left_at + STOP_DUE_AFTER,
                call_at,
            );
        } else {
            assert!(
                state == COMPOSITION_STARTING || state == COMPOSITION_ACTIVE,
                "case {:?}: the published timeout has not run out, so the composition is \
                 still composing, got {reported}",
                case.case
            );
            assert!(
                reported["endTime"].is_null(),
                "case {:?}: a composition that is still composing reports no endTime, \
                 got {reported}",
                case.case
            );
        }
    }
}
