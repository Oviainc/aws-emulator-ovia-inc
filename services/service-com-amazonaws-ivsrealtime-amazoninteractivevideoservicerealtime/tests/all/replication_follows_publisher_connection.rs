// SPDX-License-Identifier: CC0-1.0

//! What a replication does when the publisher it replicates leaves the stage it publishes
//! in: `DisconnectParticipant` on that publisher, and what every reader of the replication,
//! its participants and its stages answers afterwards.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body — never
//! an internal call. There are two exceptions, both the stage's DATA plane, which no
//! operation of this control plane serves: bringing a participant INTO a stage is
//! `state::join_stage_session`, exactly as the `DisconnectParticipant` and `ListParticipants`
//! suites beside this one do it, and running a reconnect window out is moving the deadline
//! the service recorded up to the present — the one value its materialisation compares the
//! clock against — so a window of up to five minutes is observed without waiting one out.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A REPLICATION RUNS ONLY WHILE ITS PUBLISHER IS CONNECTED. The publisher leaving the
//!    session it publishes in puts every replication it sources on a deadline one
//!    `reconnectWindowSeconds` after that instant — the model's own rule, "if the
//!    participant disconnects and then reconnects within the specified interval, replication
//!    will continue to be `ACTIVE`" — so with the documented default of 0 the end is owed at
//!    the instant the publisher went and is already written down when the call answers.
//! 2. WHEN THE WINDOW HAS RUN OUT the replication ends exactly as `StopParticipantReplication`
//!    ends it: the replica reports the other value of `ReplicationState`, the participant the
//!    replication put into the destination stage is `DISCONNECTED` there, that stage reports
//!    no active session once it held nobody else, and the end is the publisher's
//!    `REPLICATION_STOPPED` occurrence naming that destination.
//! 3. INSIDE THE WINDOW NOTHING HAS ENDED, and a publisher back inside it takes the deadline
//!    away: the replication goes on being `ACTIVE` with its replica still in the destination
//!    stage, even once the window it was given has long run out.
//! 4. ONLY THE REPLICATIONS OF THAT PUBLISHER END. Another publisher replicated from the
//!    same stage into the same destination keeps running, and so does its replica's record.
//! 5. THE WINDOW IS THE MODELLED ONE: `ReconnectWindowSeconds` is an integer in `0..300`, so
//!    each published bound is accepted and a value past either is the modelled
//!    `ValidationException` (HTTP 400).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The two values `ReplicationState` accepts, from
/// `smithy_query(query="shape",shape="ReplicationState")`: the state a start leaves a
/// replication in and the one its end leaves it in.
const ACTIVE: &str = "ACTIVE";
const STOPPED: &str = "STOPPED";

/// The two `ParticipantState` values of
/// `smithy_query(query="shape",shape="ParticipantState")`.
const CONNECTED: &str = "CONNECTED";
const DISCONNECTED: &str = "DISCONNECTED";

/// The occurrence name the end of a replication is recorded under, from
/// `smithy_query(query="shape",shape="EventName")`.
const REPLICATION_STOPPED: &str = "REPLICATION_STOPPED";

/// `ReconnectWindowSeconds`' modelled `@range`, from
/// `smithy_query(query="operation",operation="StartParticipantReplication")`: `0..300`.
const RECONNECT_WINDOW_MIN: i64 = 0;
const RECONNECT_WINDOW_MAX: i64 = 300;

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it.
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

    /// A publisher of `stage_arn`, from a token issued for it: the identity the model says a
    /// `participantId` comes from ("assigned by IVS and returned by
    /// `CreateParticipantToken`").
    async fn publisher(&self, case: &str, stage_arn: &str) -> String {
        let request = json!({ "stageArn": stage_arn });
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Start one replication, answering the status and body unchanged.
    async fn start(&self, request: &Value) -> (u16, Value) {
        self.call("StartParticipantReplication", request).await
    }

    /// Start one replication, asserting it was accepted.
    async fn start_ok(&self, case: &str, request: &Value) {
        let (status, response) = self.start(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// Disconnect one participant from one stage, asserting the call was accepted.
    async fn disconnect(&self, case: &str, stage_arn: &str, participant_id: &str) {
        let request = json!({ "stageArn": stage_arn, "participantId": participant_id });
        let (status, response) = self.call("DisconnectParticipant", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisconnectParticipant {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// The one replica of `participant_id` replicated into `destination_stage_arn`, as
    /// `ListParticipantReplicas` reports it.
    async fn replica(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
        destination_stage_arn: &str,
    ) -> Value {
        let request = json!({
            "sourceStageArn": source_stage_arn,
            "participantId": participant_id,
        });
        let (status, response) = self.call("ListParticipantReplicas", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantReplicas {request} must answer HTTP 200, \
             got {status} {response}"
        );
        let replicas = response["replicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: replicas is @required, got {response}"))
            .clone();
        let matching: Vec<&Value> = replicas
            .iter()
            .filter(|replica| member(replica, "destinationStageArn") == destination_stage_arn)
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: exactly one replica of {participant_id} must name \
             {destination_stage_arn}, got {replicas:?}"
        );
        matching[0].clone()
    }

    /// The `replicationState` the listing reports for one replica.
    async fn replication_state(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
        destination_stage_arn: &str,
    ) -> String {
        let replica = self
            .replica(case, source_stage_arn, participant_id, destination_stage_arn)
            .await;
        member(&replica, "replicationState")
    }

    /// The `activeSessionId` `GetStage` reports for `stage_arn`, the empty string once that
    /// stage has no session running.
    async fn active_session(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage on {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
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
        let (status, response) = self.call("GetParticipant", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetParticipant {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participant"], "state")
    }

    /// What `GetParticipant` reports one participant's replication to be.
    async fn participant_replication(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> (String, String) {
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
        (
            member(&response["participant"], "replicationType"),
            member(&response["participant"], "replicationState"),
        )
    }

    /// The occurrences `ListParticipantEvents` reports for one participant of one session,
    /// in occurrence order.
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
        let (status, response) = self.call("ListParticipantEvents", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantEvents {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["events"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: events is @required, got {response}"))
            .clone()
    }

    /// Record a participant joining `stage_arn` as a publisher, the way the stage's data
    /// plane records one — the only seam these cases reach into, because no operation of
    /// this control plane puts a participant into a stage.
    fn join(&self, case: &str, stage_arn: &str, participant_id: &str) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let joined = state::JoinedParticipant {
            participant_id: participant_id.to_string(),
            published: true,
            ..Default::default()
        };
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// The deadline the service recorded for one replication: the instant the clock ends it
    /// at because its publisher has gone, or `None` while that publisher is connected.
    fn deadline(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
        destination_stage_arn: &str,
    ) -> Option<SystemTime> {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let store = handle.lock();
        store
            .participant_replicas
            .get(source_stage_arn)
            .unwrap_or_else(|| panic!("case {case:?}: {source_stage_arn} must hold a replication"))
            .iter()
            .find(|replica| {
                replica.participant_id == participant_id
                    && replica.destination_stage_arn == destination_stage_arn
            })
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {source_stage_arn} must hold the replication of \
                     {participant_id} into {destination_stage_arn}"
                )
            })
            .reconnect_deadline
    }

    /// Bring every deadline the service recorded for `participant_id`'s replications up to
    /// the present, so what it does when a reconnect window runs out is observable without
    /// waiting one out, and answer how many it moved.
    ///
    /// Refuses to invent one: the deadline has to be there already, which is rule 1.
    fn run_out_the_window(&self, case: &str, source_stage_arn: &str, participant_id: &str) -> usize {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let replications = store
            .participant_replicas
            .get_mut(source_stage_arn)
            .unwrap_or_else(|| panic!("case {case:?}: {source_stage_arn} must hold a replication"));
        let mut moved = 0;
        for replication in replications
            .iter_mut()
            .filter(|replication| replication.participant_id == participant_id)
        {
            assert!(
                replication.reconnect_deadline.is_some(),
                "case {case:?}: the replication of {participant_id} into {} must already \
                 carry the deadline its publisher's departure earned it",
                replication.destination_stage_arn
            );
            replication.reconnect_deadline = Some(SystemTime::now());
            moved += 1;
        }
        moved
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
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

/// The three members every `StartParticipantReplication` request carries, with the window
/// the case asked for when it asked for one.
fn start_request(
    source: &str,
    destination: &str,
    participant: &str,
    window: Option<i64>,
) -> Value {
    let mut request = json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": participant,
    });
    if let Some(window) = window {
        request["reconnectWindowSeconds"] = json!(window);
    }
    request
}

/// One row of the class "the publisher of a replication leaves the stage it publishes in".
/// The rows differ in what the rule generalises over: the window the start asked to
/// reconnect in, how many stages the publisher is replicated into, whether another
/// publisher shares the destination stage, and whether that stage holds a participant of
/// its own.
#[derive(Clone, Copy, Debug)]
struct Case {
    case: &'static str,
    /// `reconnectWindowSeconds`, or `None` to omit the member — which is the default the
    /// model documents ("Default: 0").
    window: Option<i64>,
    /// How many destination stages the publisher is replicated into.
    destinations: usize,
    /// Whether a SECOND publisher of the same source stage is replicated into the first
    /// destination stage.
    other_publisher: bool,
    /// Whether the first destination stage holds a participant of its own, so the replica
    /// leaving does not empty that stage.
    local_participant: bool,
}

impl Case {
    /// The window this case's replications are given, as the span the clock measures: the
    /// member it asked for, or the documented default when it asked for none.
    fn window(self) -> Duration {
        let seconds = self
            .window
            .unwrap_or(i64::from(state::DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS));
        Duration::from_secs(u64::try_from(seconds).expect("a window is never negative"))
    }
}

/// The rows this suite spans the class with.
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "the smallest start request the model allows, so the documented default window",
            window: None,
            destinations: 1,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "a start that asked for the minimum window the model allows",
            window: Some(RECONNECT_WINDOW_MIN),
            destinations: 1,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "a start that asked for a one-second window",
            window: Some(1),
            destinations: 1,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "a start that asked for a window halfway up the modelled range",
            window: Some(RECONNECT_WINDOW_MAX / 2),
            destinations: 1,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "a start that asked for the maximum window the model allows",
            window: Some(RECONNECT_WINDOW_MAX),
            destinations: 1,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "one publisher replicated into three stages, with the default window",
            window: None,
            destinations: 3,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "one publisher replicated into two stages, with a window to run out",
            window: Some(30),
            destinations: 2,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "two publishers replicated into one destination stage",
            window: None,
            destinations: 1,
            other_publisher: true,
            local_participant: false,
        },
        Case {
            case: "a destination stage that holds a participant of its own",
            window: Some(10),
            destinations: 1,
            other_publisher: false,
            local_participant: true,
        },
        Case {
            case: "two publishers and a local participant, with a window to run out",
            window: Some(RECONNECT_WINDOW_MAX),
            destinations: 2,
            other_publisher: true,
            local_participant: true,
        },
    ]
}

/// Everything one row set up: the stages, the publisher and what else is in them.
struct Arranged {
    source: String,
    destinations: Vec<String>,
    publisher: String,
    /// The second publisher of the source stage, replicated into the first destination
    /// stage, when the row has one.
    bystander: Option<String>,
    /// The participant of the first destination stage's own, when the row has one.
    local: Option<String>,
    source_session: String,
    /// The session of each destination stage, in the same order as [`Self::destinations`].
    destination_sessions: Vec<String>,
}

/// Build one row's stages and replications, and assert the start left every replication
/// running with its replica in the destination stage — the state the departure acts on.
async fn arrange(emulator: &RealTime, row: Case) -> Arranged {
    let case = row.case;
    let source = emulator.stage(case, "source").await;
    let publisher = emulator.publisher(case, &source).await;
    let mut destinations = Vec::new();
    for index in 0..row.destinations {
        let destination = emulator.stage(case, &format!("destination-{index}")).await;
        emulator
            .start_ok(
                case,
                &start_request(&source, &destination, &publisher, row.window),
            )
            .await;
        destinations.push(destination);
    }
    let bystander = if row.other_publisher {
        let bystander = emulator.publisher(case, &source).await;
        emulator
            .start_ok(
                case,
                &start_request(&source, &destinations[0], &bystander, row.window),
            )
            .await;
        Some(bystander)
    } else {
        None
    };
    let local = if row.local_participant {
        let local = emulator.publisher(case, &destinations[0]).await;
        emulator.join(case, &destinations[0], &local);
        Some(local)
    } else {
        None
    };

    let source_session = emulator.active_session(case, &source).await;
    let mut destination_sessions = Vec::new();
    for destination in &destinations {
        destination_sessions.push(emulator.active_session(case, destination).await);
    }
    for (destination, session) in destinations.iter().zip(&destination_sessions) {
        let state = emulator
            .replication_state(case, &source, &publisher, destination)
            .await;
        assert_eq!(
            state, ACTIVE,
            "case {case:?}: a started replication into {destination} must report {ACTIVE}, \
             got {state:?}"
        );
        let replica = emulator
            .participant_state(case, destination, session, &publisher)
            .await;
        assert_eq!(
            replica, CONNECTED,
            "case {case:?}: the replica in {destination} must be {CONNECTED}, got {replica:?}"
        );
    }
    Arranged {
        source,
        destinations,
        publisher,
        bystander,
        local,
        source_session,
        destination_sessions,
    }
}

/// Assert every replication of `arranged.publisher` has ended the way a stop ends one, and
/// that nothing else in the two stages did.
async fn assert_ended(emulator: &RealTime, row: Case, arranged: &Arranged) {
    let case = row.case;
    for (destination, session) in arranged
        .destinations
        .iter()
        .zip(&arranged.destination_sessions)
    {
        // RULE 2: the replication reports the other value of the modelled list.
        let state = emulator
            .replication_state(case, &arranged.source, &arranged.publisher, destination)
            .await;
        assert_eq!(
            state, STOPPED,
            "case {case:?}: the replication into {destination} must report {STOPPED} once its \
             publisher has been gone for its whole window, got {state:?}"
        );
        // RULE 2: the replica the replication put into that stage has left it.
        let replica = emulator
            .participant_state(case, destination, session, &arranged.publisher)
            .await;
        assert_eq!(
            replica, DISCONNECTED,
            "case {case:?}: the replica in {destination} must be {DISCONNECTED}, got {replica:?}"
        );
        // RULE 2: the end is an occurrence of the publisher's own session, naming the
        // destination the ended replication named.
        let events = emulator
            .events(
                case,
                &arranged.source,
                &arranged.source_session,
                &arranged.publisher,
            )
            .await;
        let ends: Vec<&Value> = events
            .iter()
            .filter(|event| {
                member(event, "name") == REPLICATION_STOPPED
                    && member(event, "destinationStageArn") == *destination
            })
            .collect();
        assert_eq!(
            ends.len(),
            1,
            "case {case:?}: the publisher's session must record exactly one \
             {REPLICATION_STOPPED} naming {destination}, got {events:?}"
        );
        assert_eq!(
            member(ends[0], "destinationSessionId"),
            *session,
            "case {case:?}: that occurrence must name the destination session, got {:?}",
            ends[0]
        );
    }

    // RULE 2: a destination stage the replica leaving emptied reports no session; one that
    // still holds somebody keeps the session it was running.
    for (index, destination) in arranged.destinations.iter().enumerate() {
        let held_by_others = index == 0 && (arranged.bystander.is_some() || arranged.local.is_some());
        let active = emulator.active_session(case, destination).await;
        if held_by_others {
            assert_eq!(
                active, arranged.destination_sessions[index],
                "case {case:?}: {destination} still holds a participant, so it must keep the \
                 session it was running, got {active:?}"
            );
        } else {
            assert!(
                active.is_empty(),
                "case {case:?}: {destination} was emptied by the replica leaving, so it must \
                 report no active session, got {active:?}"
            );
        }
    }

    // RULE 4: another publisher's replication into the same stage is untouched, replica and
    // all, and so is a participant of the destination stage's own.
    if let Some(bystander) = &arranged.bystander {
        let state = emulator
            .replication_state(case, &arranged.source, bystander, &arranged.destinations[0])
            .await;
        assert_eq!(
            state, ACTIVE,
            "case {case:?}: the replication of {bystander}, whose publisher never left, must \
             still report {ACTIVE}, got {state:?}"
        );
        let replica = emulator
            .participant_state(
                case,
                &arranged.destinations[0],
                &arranged.destination_sessions[0],
                bystander,
            )
            .await;
        assert_eq!(
            replica, CONNECTED,
            "case {case:?}: the replica of {bystander} must still be {CONNECTED}, \
             got {replica:?}"
        );
    }
    if let Some(local) = &arranged.local {
        let state = emulator
            .participant_state(
                case,
                &arranged.destinations[0],
                &arranged.destination_sessions[0],
                local,
            )
            .await;
        assert_eq!(
            state, CONNECTED,
            "case {case:?}: the destination stage's own participant {local} must still be \
             {CONNECTED}, got {state:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rules 1, 2 and 4 — the publisher leaves, and the window decides when the
// replications it sources end
// ---------------------------------------------------------------------------

/// The departure of the publisher puts every replication it sources on a deadline one window
/// later, and when that window has run out each of them has ended exactly as a stop ends one.
#[tokio::test]
async fn a_publisher_leaving_ends_the_replications_it_sources_one_window_later() {
    for row in cases() {
        let case = row.case;
        let emulator = RealTime::new();
        let arranged = arrange(&emulator, row).await;

        let before = SystemTime::now();
        emulator
            .disconnect(case, &arranged.source, &arranged.publisher)
            .await;
        let after = SystemTime::now();

        // RULE 1, first reading: the replication of a publisher that is still there is owed
        // nothing at all.
        if let Some(bystander) = &arranged.bystander {
            let deadline = emulator.deadline(
                case,
                &arranged.source,
                bystander,
                &arranged.destinations[0],
            );
            assert!(
                deadline.is_none(),
                "case {case:?}: the replication of {bystander}, whose publisher never left, \
                 must be owed no end, got {deadline:?}"
            );
        }

        if row.window().is_zero() {
            // With the window the model documents as the default, the end is owed at the
            // very instant the publisher went, so the call that disconnected it has already
            // written every end down and no replication is left waiting for one.
            for destination in &arranged.destinations {
                let deadline =
                    emulator.deadline(case, &arranged.source, &arranged.publisher, destination);
                assert!(
                    deadline.is_none(),
                    "case {case:?}: the replication into {destination} has already ended, so \
                     it must wait for nothing, got {deadline:?}"
                );
            }
            assert_ended(&emulator, row, &arranged).await;
            continue;
        }

        // RULE 1, second reading: a window the start asked for leaves every replication of
        // that publisher owed its end one window after the instant the publisher went.
        for destination in &arranged.destinations {
            let deadline = emulator.deadline(
                case,
                &arranged.source,
                &arranged.publisher,
                destination,
            );
            assert!(
                deadline.is_some_and(|due| due >= before + row.window()
                    && due <= after + row.window()),
                "case {case:?}: the replication into {destination} must be owed its end one \
                 {:?} after the publisher went (between {:?} and {:?}), got {deadline:?}",
                row.window(),
                before + row.window(),
                after + row.window()
            );
        }

        // RULE 3: inside the window nothing has ended — the replication is still running
        // and its replica is still in the destination stage.
        for (destination, session) in arranged
            .destinations
            .iter()
            .zip(&arranged.destination_sessions)
        {
            let state = emulator
                .replication_state(case, &arranged.source, &arranged.publisher, destination)
                .await;
            assert_eq!(
                state, ACTIVE,
                "case {case:?}: inside the {:?} window the replication into {destination} must \
                 still report {ACTIVE}, got {state:?}",
                row.window()
            );
            let replica = emulator
                .participant_state(case, destination, session, &arranged.publisher)
                .await;
            assert_eq!(
                replica, CONNECTED,
                "case {case:?}: inside the window the replica in {destination} must still be \
                 {CONNECTED}, got {replica:?}"
            );
            let active = emulator.active_session(case, destination).await;
            assert_eq!(
                active, *session,
                "case {case:?}: inside the window {destination} must keep the session its \
                 replica is in, got {active:?}"
            );
        }

        // The window runs out, and the service walks every step itself.
        let moved = emulator.run_out_the_window(case, &arranged.source, &arranged.publisher);
        assert_eq!(
            moved,
            arranged.destinations.len(),
            "case {case:?}: every replication of the publisher must carry a deadline to run out"
        );
        assert_ended(&emulator, row, &arranged).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 3 — a publisher back inside its window keeps replicating
// ---------------------------------------------------------------------------

/// "If the participant disconnects and then reconnects within the specified interval,
/// replication will continue to be `ACTIVE`": a publisher back in the stage it publishes in
/// takes away the end its replications were owed, and they go on running however long the
/// window it was given has since been.
#[tokio::test]
async fn a_publisher_back_inside_its_window_keeps_its_replications_running() {
    // Only the rows with a window to come back inside: a replication whose window is zero
    // has already ended when the publisher goes, which the case above pins.
    for row in cases().into_iter().filter(|row| !row.window().is_zero()) {
        let case = row.case;
        let emulator = RealTime::new();
        let arranged = arrange(&emulator, row).await;

        // A participant of the source stage that stays, so the session the publisher
        // publishes in survives its departure: "a stage session begins when the first
        // participant joins a stage and ends after the last participant leaves", and a
        // publisher can only come back to a session that is still running.
        let stayer = emulator.publisher(case, &arranged.source).await;
        emulator.join(case, &arranged.source, &stayer);

        emulator
            .disconnect(case, &arranged.source, &arranged.publisher)
            .await;
        for destination in &arranged.destinations {
            assert!(
                emulator
                    .deadline(case, &arranged.source, &arranged.publisher, destination)
                    .is_some(),
                "case {case:?}: the departure must have put the replication into \
                 {destination} on a deadline"
            );
        }

        // The publisher comes back over the stage's data plane, inside its window.
        emulator.join(case, &arranged.source, &arranged.publisher);
        let active = emulator.active_session(case, &arranged.source).await;
        assert_eq!(
            active, arranged.source_session,
            "case {case:?}: the publisher must have come back to the session it published \
             in, got {active:?}"
        );

        for destination in &arranged.destinations {
            let deadline =
                emulator.deadline(case, &arranged.source, &arranged.publisher, destination);
            assert!(
                deadline.is_none(),
                "case {case:?}: a publisher back inside its window must leave the replication \
                 into {destination} owed no end, got {deadline:?}"
            );
            let state = emulator
                .replication_state(case, &arranged.source, &arranged.publisher, destination)
                .await;
            assert_eq!(
                state, ACTIVE,
                "case {case:?}: the replication into {destination} must continue to be \
                 {ACTIVE}, got {state:?}"
            );
        }

        // And the publisher and its replicas are all in their stages again, which is what
        // the participant readers report of the replication from both ends.
        let (publisher_type, publisher_state) = emulator
            .participant_replication(
                case,
                &arranged.source,
                &arranged.source_session,
                &arranged.publisher,
            )
            .await;
        assert_eq!(
            (publisher_type.as_str(), publisher_state.as_str()),
            ("SOURCE", ACTIVE),
            "case {case:?}: the publisher must report a running replication of its own, \
             got {publisher_type:?} {publisher_state:?}"
        );
        for (destination, session) in arranged
            .destinations
            .iter()
            .zip(&arranged.destination_sessions)
        {
            let (replica_type, replica_state) = emulator
                .participant_replication(case, destination, session, &arranged.publisher)
                .await;
            assert_eq!(
                (replica_type.as_str(), replica_state.as_str()),
                ("REPLICA", ACTIVE),
                "case {case:?}: the replica in {destination} must report a running \
                 replication, got {replica_type:?} {replica_state:?}"
            );
            let replica = emulator
                .participant_state(case, destination, session, &arranged.publisher)
                .await;
            assert_eq!(
                replica, CONNECTED,
                "case {case:?}: the replica in {destination} must still be {CONNECTED}, \
                 got {replica:?}"
            );
        }

        // Going away a SECOND time puts the replications back on their window, which is the
        // same rule applied again rather than a one-shot deadline.
        emulator
            .disconnect(case, &arranged.source, &arranged.publisher)
            .await;
        for destination in &arranged.destinations {
            assert!(
                emulator
                    .deadline(case, &arranged.source, &arranged.publisher, destination)
                    .is_some(),
                "case {case:?}: a second departure must put the replication into \
                 {destination} on a deadline again"
            );
        }
        emulator.run_out_the_window(case, &arranged.source, &arranged.publisher);
        assert_ended(&emulator, row, &arranged).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 5 — the window is the modelled one
// ---------------------------------------------------------------------------

/// Each published bound of `ReconnectWindowSeconds` is accepted and a value past either is
/// the modelled `ValidationException`, so no case above asks for a window the model refuses.
#[tokio::test]
async fn the_reconnect_window_is_the_modelled_range() {
    // Each published bound, and one past each of them.
    let rows: [(i64, bool); 4] = [
        (RECONNECT_WINDOW_MIN, true),
        (RECONNECT_WINDOW_MAX, true),
        (RECONNECT_WINDOW_MIN - 1, false),
        (RECONNECT_WINDOW_MAX + 1, false),
    ];

    for (window, accepted) in rows {
        let case = format!("reconnectWindowSeconds {window}");
        let case = case.as_str();
        let emulator = RealTime::new();
        let source = emulator.stage(case, "source").await;
        let destination = emulator.stage(case, "destination").await;
        let publisher = emulator.publisher(case, &source).await;

        let request = start_request(&source, &destination, &publisher, Some(window));
        let (status, response) = emulator.start(&request).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: a window the modelled range admits must be accepted, \
                 got {status} {response}"
            );
            let state = emulator
                .replication_state(case, &source, &publisher, &destination)
                .await;
            assert_eq!(
                state, ACTIVE,
                "case {case:?}: the accepted start must leave the replication {ACTIVE}, \
                 got {state:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a window outside the modelled range must be refused with \
                 HTTP 400, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: that refusal must be the modelled ValidationException, \
                 got {response}"
            );
        }
    }
}
