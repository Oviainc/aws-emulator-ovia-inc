// SPDX-License-Identifier: CC0-1.0

//! What the clock owes a composition whose stage is emptied by a call that takes a
//! REPLICATED participant out of it.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body — never
//! an internal call. The one exception is bringing a participant into a stage over its data
//! plane, which no operation of this control plane does: that is
//! `state::join_stage_session`, the seam the `DisconnectParticipant` and `ListParticipants`
//! suites beside this one join through.
//!
//! The rule under test: whenever the stage a composition composites comes to hold no
//! connected participant, every composition of that stage is owed its stop one
//! `state::COMPOSITION_EMPTY_STAGE_STOP_SPAN` after THAT instant — whichever call left it
//! empty, a departure of a participant that joined the stage or of a replica a replication
//! put into it — and when that deadline runs out the composition stops and, one
//! `state::COMPOSITION_RETENTION_SPAN` later, the ephemeral resource is deleted. While the
//! stage still holds one, nothing is owed, and a stage
//! somebody is still publishing in is not emptied by a replication out of it ending.
//!
//! Read as three cases:
//!
//! 1. A STAGE A REPLICATION LEFT WITHOUT PARTICIPANTS PUTS ITS COMPOSITIONS ON A DEADLINE,
//!    measured from the call that emptied it, and not before: while any participant of that
//!    stage is still connected — another replica, or one that joined over the data plane —
//!    the composition is owed nothing and goes on composing.
//! 2. WHEN THAT TIMEOUT RUNS OUT the composition ends exactly as a caller's
//!    `StopComposition` ends it, the ephemeral resource is deleted (so the read is the
//!    modelled 404) and the encoder configuration its destination named can be deleted
//!    again.
//! 3. A SOURCE STAGE IS NOT EMPTIED BY A REPLICATION ENDING: the publisher is in the stage
//!    it publishes in whether or not anything replicates it, so a composition of that stage
//!    is owed no stop.
//!
//! The timeout AWS publishes is a MINUTE, which no case waits out: the case that observes
//! what happens when it runs out moves the stored deadline — the one value the service's
//! materialisation compares the clock against — up to the present, after asserting the
//! service recorded it, and then watches the service walk every step itself.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries, and so the account every ARN built
/// by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every case in this suite signs for.
const REGION: &str = "us-east-1";

/// How long after its stage was left empty a composition is owed its stop, read from the
/// crate so this suite cannot drift from the span the service enforces. It is one whole end
/// — the stop and the retention of the stopped record — inside the minute the model
/// publishes (`state::COMPOSITION_EMPTY_STAGE_TIMEOUT`).
const STOP_DUE_AFTER: Duration = state::COMPOSITION_EMPTY_STAGE_STOP_SPAN;

/// The span the service gives the `STOPPING` -> `STOPPED` step of a stop, read from the
/// crate for the same reason.
const STOP_SPAN: Duration = state::COMPOSITION_STOP_SPAN;

/// How long the service goes on reporting a stopped composition before the ephemeral
/// resource is deleted, read from the crate for the same reason.
const RETENTION: Duration = state::COMPOSITION_RETENTION_SPAN;

/// Added to every sleep, so a span this suite waits out has certainly elapsed whatever
/// scheduling delay the test host adds.
const SLACK: Duration = Duration::from_millis(400);

// The `CompositionState` values this suite asserts, from
// `smithy_query(query="shape",shape="CompositionState")`:
//   STARTING | ACTIVE | STOPPING | FAILED | STOPPED
const STATE_STARTING: &str = "STARTING";
const STATE_ACTIVE: &str = "ACTIVE";
const STATE_STOPPING: &str = "STOPPING";
const STATE_STOPPED: &str = "STOPPED";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it,
/// plus the engine handle the data-plane joins are recorded through.
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

    /// Create a stage and answer its ARN.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name:?} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Create an encoder configuration and answer its ARN.
    async fn create_encoder_configuration(&self, case: &str) -> String {
        let (status, response) = self
            .call("CreateEncoderConfiguration", &json!({ "name": "enc" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["encoderConfiguration"], "arn")
    }

    /// Issue a participant token for `stage_arn` and answer the participant id it carries —
    /// the id the holder of that token is known by in that stage.
    async fn create_token(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self
            .call("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken on {stage_arn} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does.
    fn join(&self, case: &str, stage_arn: &str, participant_id: &str) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let joined = state::JoinedParticipant {
            participant_id: participant_id.to_string(),
            ..Default::default()
        };
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// Replicate the publisher `participant_id` of `source` into `destination`, asserting
    /// the start was accepted — which is what puts a participant into `destination`.
    async fn replicate(&self, case: &str, source: &str, destination: &str, participant_id: &str) {
        let request = replication_request(source, destination, participant_id);
        let (status, response) = self.call("StartParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// Stop that replication, asserting the modelled success, and answer the window the call
    /// was made in.
    async fn stop_replication(
        &self,
        case: &str,
        source: &str,
        destination: &str,
        participant_id: &str,
    ) -> (SystemTime, SystemTime) {
        let request = replication_request(source, destination, participant_id);
        let before = SystemTime::now();
        let (status, response) = self.call("StopParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
        (before, SystemTime::now())
    }

    /// `DisconnectParticipant`, asserting the modelled success, and answering the window the
    /// call was made in.
    async fn disconnect(
        &self,
        case: &str,
        stage_arn: &str,
        participant_id: &str,
    ) -> (SystemTime, SystemTime) {
        let before = SystemTime::now();
        let (status, response) = self
            .call(
                "DisconnectParticipant",
                &json!({ "stageArn": stage_arn, "participantId": participant_id }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisconnectParticipant {participant_id} must answer HTTP 200, \
             got {status} {response}"
        );
        (before, SystemTime::now())
    }

    /// `StartComposition` against `stage_arn`, with one destination naming `encoder`, and
    /// answer the composition's ARN.
    async fn start(&self, case: &str, stage_arn: &str, encoder: &str) -> String {
        let request = json!({
            "stageArn": stage_arn,
            "destinations": [{
                "channel": {
                    "channelArn": arn("channel", "live"),
                    "encoderConfigurationArn": encoder,
                },
            }],
        });
        let (status, response) = self.call("StartComposition", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartComposition {request} must answer HTTP 200, \
             got {status} {response}"
        );
        let composition = response["composition"].clone();
        assert_eq!(
            member(&composition, "state"),
            STATE_STARTING,
            "case {case:?}: a composition that has just started is {STATE_STARTING}, \
             got {composition}"
        );
        member(&composition, "arn")
    }

    /// `GetComposition` without asserting the read succeeded — a composition the clock has
    /// reclaimed is read here too, and that read is a 404.
    async fn read(&self, arn: &str) -> (u16, Value) {
        self.call("GetComposition", &json!({ "arn": arn })).await
    }

    /// The ARNs `ListCompositions` reports for this account.
    async fn listed(&self, case: &str) -> Vec<String> {
        let (status, response) = self.call("ListCompositions", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCompositions must answer HTTP 200, got {status} {response}"
        );
        response["compositions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|composition| member(composition, "arn"))
            .collect()
    }

    /// The deadline the service recorded for the composition `arn` names: the instant the
    /// clock owes it the stop its empty stage earned, or `None` while somebody is in that
    /// stage.
    fn deadline(&self, case: &str, arn: &str) -> Option<SystemTime> {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let store = handle.lock();
        store
            .compositions
            .get(arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {arn}"))
            .empty_stage_stop_at
    }

    /// Bring the deadline the service recorded for `arn` up to the present, so what the
    /// service does when the published minute runs out is observable without waiting one
    /// out, and answer the instant it ran out at.
    ///
    /// Refuses to invent one: the deadline has to be there already, which is case 1.
    fn run_out_the_timeout(&self, case: &str, arn: &str) -> Instant {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let composition = store
            .compositions
            .get_mut(arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {arn}"));
        assert!(
            composition.empty_stage_stop_at.is_some(),
            "case {case:?}: {arn} must already carry the deadline its empty stage earned, \
             got {:?}",
            composition.empty_stage_stop_at
        );
        composition.empty_stage_stop_at = Some(SystemTime::now());
        Instant::now()
    }

    /// Move the end the service recorded for `arn` one whole [`RETENTION`] into the past, so
    /// what the service does when a stopped composition's record expires is observable
    /// without waiting that span out.
    ///
    /// Refuses to invent one: the composition's end has to have begun already, by the
    /// deadline the clock ran out or by a caller's stop.
    fn run_out_the_retention(&self, case: &str, arn: &str) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let composition = store
            .compositions
            .get_mut(arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {arn}"));
        let begun = composition.ended_at.is_some()
            || composition
                .empty_stage_stop_at
                .is_some_and(|due| due <= SystemTime::now());
        assert!(
            begun,
            "case {case:?}: {arn} must already be ending for its record to expire, \
             got ended_at {:?} and deadline {:?}",
            composition.ended_at, composition.empty_stage_stop_at
        );
        let shift = RETENTION + STOP_SPAN + SLACK;
        composition.ended_at = composition.ended_at.map(|ended| ended - shift);
        composition.empty_stage_stop_at = composition
            .empty_stage_stop_at
            .map(|deadline| deadline - shift);
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

/// One way the stage a composition composites comes to hold nobody at all, where what put a
/// participant in it was a replication. The rows differ in WHICH call empties the stage, in
/// how many participants — replicas, and ones that joined over the data plane — it had to
/// take out, and in how many source stages replicate into it, which is what the rule
/// generalises over: the deadline is measured from the instant the stage was left empty,
/// whichever call left it that way.
#[derive(Clone, Copy, Debug)]
enum Emptied {
    /// `n` publishers of ONE source stage are replicated into the composited stage and the
    /// replications are stopped one by one: only the stop of the LAST one empties it.
    LastReplicationStopped(usize),
    /// `n` separate source stages each replicate one publisher into the composited stage,
    /// and again only the last stop empties it.
    LastReplicationOfSeveralSourcesStopped(usize),
    /// The composited stage holds a replica and a participant that joined it over the data
    /// plane, and the data-plane one leaves first, so the replication stop empties it.
    ReplicationStoppedAfterTheJoinLeft,
    /// The same two the other way round: the replication is stopped first, so
    /// `DisconnectParticipant` of the data-plane participant empties it.
    JoinDisconnectedAfterTheReplicationStopped,
}

/// The rows every case of this suite is exercised over.
const EMPTIED_ROWS: [Emptied; 5] = [
    Emptied::LastReplicationStopped(1),
    Emptied::LastReplicationStopped(3),
    Emptied::LastReplicationOfSeveralSourcesStopped(2),
    Emptied::ReplicationStoppedAfterTheJoinLeft,
    Emptied::JoinDisconnectedAfterTheReplicationStopped,
];

impl Emptied {
    /// This row's name, as every assertion of this suite reports it.
    fn case(self) -> String {
        format!("{self:?}")
    }

    /// Stand a composition of one stage up as this row describes — the composition starts
    /// while that stage is OCCUPIED, so the only thing that can put it on a deadline is the
    /// departure this row arranges — and answer its ARN beside the window of the call that
    /// left its stage empty.
    ///
    /// Asserts along the way that nothing is owed while the stage still holds somebody.
    async fn arrange(
        self,
        emulator: &RealTime,
        encoder: &str,
    ) -> (String, (SystemTime, SystemTime)) {
        let case = self.case();
        let case = case.as_str();
        let composited = emulator.create_stage(case, "composited").await;

        // The replicas this row puts into the composited stage, each a publisher of a source
        // stage replicated in: one source stage replicating several publishers, or several
        // source stages replicating one each.
        let (sources, per_source) = match self {
            Self::LastReplicationStopped(publishers) => (1, publishers),
            Self::LastReplicationOfSeveralSourcesStopped(sources) => (sources, 1),
            _ => (1, 1),
        };
        let mut replicated: Vec<(String, String)> = Vec::new();
        for index in 0..sources {
            let source = emulator
                .create_stage(case, &format!("source-{index}"))
                .await;
            for _ in 0..per_source {
                let publisher = emulator.create_token(case, &source).await;
                emulator
                    .replicate(case, &source, &composited, &publisher)
                    .await;
                replicated.push((source.clone(), publisher));
            }
        }
        // The data-plane participant the two mixed rows also hold in the composited stage.
        let joined = match self {
            Self::ReplicationStoppedAfterTheJoinLeft
            | Self::JoinDisconnectedAfterTheReplicationStopped => {
                let joined = emulator.create_token(case, &composited).await;
                emulator.join(case, &composited, &joined);
                Some(joined)
            }
            _ => None,
        };

        let composition = emulator.start(case, &composited, encoder).await;
        assert!(
            emulator.deadline(case, &composition).is_none(),
            "case {case:?}: a composition of a stage that holds participants must be owed \
             no stop, got {:?}",
            emulator.deadline(case, &composition)
        );

        // The departures, in this row's order, each asserted to owe nothing until the one
        // that actually empties the stage.
        let mut departures: Vec<Departure> = Vec::new();
        match self {
            Self::LastReplicationStopped(_) | Self::LastReplicationOfSeveralSourcesStopped(_) => {
                for (source, publisher) in &replicated {
                    departures.push(Departure::StopReplication(source.clone(), publisher.clone()));
                }
            }
            Self::ReplicationStoppedAfterTheJoinLeft => {
                departures.push(Departure::Disconnect(
                    joined.clone().expect("this row joined a participant"),
                ));
                departures.push(Departure::StopReplication(
                    replicated[0].0.clone(),
                    replicated[0].1.clone(),
                ));
            }
            Self::JoinDisconnectedAfterTheReplicationStopped => {
                departures.push(Departure::StopReplication(
                    replicated[0].0.clone(),
                    replicated[0].1.clone(),
                ));
                departures.push(Departure::Disconnect(
                    joined.clone().expect("this row joined a participant"),
                ));
            }
        }

        let mut emptied = (SystemTime::now(), SystemTime::now());
        let last = departures.len() - 1;
        for (index, departure) in departures.iter().enumerate() {
            emptied = match departure {
                Departure::StopReplication(source, participant) => {
                    emulator
                        .stop_replication(case, source, &composited, participant)
                        .await
                }
                Departure::Disconnect(participant) => {
                    emulator.disconnect(case, &composited, participant).await
                }
            };
            if index < last {
                let deadline = emulator.deadline(case, &composition);
                assert!(
                    deadline.is_none(),
                    "case {case:?}: with {} of {} participants still in the stage, the \
                     composition must be owed no stop, got {deadline:?}",
                    last - index,
                    last + 1
                );
            }
        }
        (composition, emptied)
    }
}

/// One participant leaving the composited stage, by the call that takes it out.
#[derive(Clone, Debug)]
enum Departure {
    /// The replication that put the replica there is stopped, named by its source stage and
    /// the publisher it replicates.
    StopReplication(String, String),
    /// The participant is disconnected by name.
    Disconnect(String),
}

// ---------------------------------------------------------------------------
// Case 1 — the departure that empties the stage puts its compositions on a deadline
// ---------------------------------------------------------------------------

/// However the composited stage comes to hold nobody, the composition is owed its stop one
/// published timeout after THAT instant — and not a moment earlier, so inside the timeout it
/// is still composing.
#[tokio::test]
async fn a_stage_a_replication_left_empty_owes_its_compositions_a_stop_one_timeout_later() {
    for row in EMPTIED_ROWS {
        let case = row.case();
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.create_encoder_configuration(case).await;
        let (composition, (before, after)) = row.arrange(&emulator, &encoder).await;

        let deadline = emulator.deadline(case, &composition);
        assert!(
            deadline.is_some_and(|due| due >= before + STOP_DUE_AFTER
                && due <= after + STOP_DUE_AFTER),
            "case {case:?}: the stop must be owed one {STOP_DUE_AFTER:?} after the stage was \
             left empty (between {:?} and {:?}), got {deadline:?}",
            before + STOP_DUE_AFTER,
            after + STOP_DUE_AFTER
        );

        // Nothing is owed YET: inside the published timeout the composition is still
        // composing, with no end time, and the configuration its destination names is still
        // held back.
        tokio::time::sleep(state::COMPOSITION_ACTIVATION_SPAN + SLACK).await;
        let (status, response) = emulator.read(&composition).await;
        assert_eq!(
            status, 200,
            "case {case:?}: inside the timeout the composition must still be reported, \
             got {status} {response}"
        );
        let read = response["composition"].clone();
        assert_eq!(
            member(&read, "state"),
            STATE_ACTIVE,
            "case {case:?}: inside the timeout the composition must still be composing, \
             got {read}"
        );
        assert!(
            read.get("endTime").is_none(),
            "case {case:?}: a composition that has not stopped carries no end time, got {read}"
        );
        let (status, response) = emulator
            .call("DeleteEncoderConfiguration", &json!({ "arn": encoder }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: while the composition composes, deleting {encoder} must be the \
             modelled conflict, got {status} {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Case 2 — when the timeout runs out
// ---------------------------------------------------------------------------

/// Once the published timeout has run out, the composition ends the way a caller's stop ends
/// it, the ephemeral resource is deleted, and what it was holding back is released.
#[tokio::test]
async fn a_composition_of_a_stage_a_replication_emptied_stops_and_is_deleted() {
    for row in EMPTIED_ROWS {
        let case = row.case();
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.create_encoder_configuration(case).await;
        let (composition, _) = row.arrange(&emulator, &encoder).await;

        let ran_out = emulator.run_out_the_timeout(case, &composition);

        // Straight after the timeout the broadcast is ending: a stopping composition with an
        // end time, or — for a read the span boundary overtook — one that has stopped. The
        // ending state is asserted of every read that found the composition at all; only a
        // host slow enough to spend the whole stop and the retention of the stopped record
        // getting here may find the ephemeral resource already reclaimed, and that is the one
        // reading admitted without it.
        let (status, response) = emulator.read(&composition).await;
        if status == 200 {
            let stopping = response["composition"].clone();
            let state = member(&stopping, "state");
            assert!(
                [STATE_STOPPING, STATE_STOPPED].contains(&state.as_str()),
                "case {case:?}: the composition whose stage stayed empty must be ending, \
                 got {stopping}"
            );
            assert!(
                stopping.get("endTime").is_some(),
                "case {case:?}: a composition the clock stopped must report when its \
                 broadcast ended, got {stopping}"
            );
        } else {
            let elapsed = ran_out.elapsed();
            assert!(
                elapsed >= STOP_SPAN + RETENTION,
                "case {case:?}: {elapsed:?} after the timeout ran out is less than the stop \
                 and the retention of the stopped record together, so the composition must \
                 still be reported, got {status} {response}"
            );
        }

        // Once the retention span is over, the ephemeral resource is gone: the read is the
        // modelled 404 and the listing no longer carries it.
        emulator.run_out_the_retention(case, &composition);
        let (status, response) = emulator.read(&composition).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a composition the timeout deleted must be read as HTTP 404, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected the modelled not-found code, got {response}"
        );
        let listed = emulator.listed(case).await;
        assert!(
            !listed.contains(&composition),
            "case {case:?}: the deleted composition must be gone from the listing, \
             got {listed:?}"
        );

        // And what it was holding back is released.
        let (status, response) = emulator
            .call("DeleteEncoderConfiguration", &json!({ "arn": encoder }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: once the composition is deleted, deleting {encoder} must answer \
             HTTP 200, got {status} {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Case 3 — the source stage is not emptied by a replication ending
// ---------------------------------------------------------------------------

/// A publisher is in the stage it publishes in whether or not anything replicates it, so
/// ending a replication out of that stage leaves a composition of it composing — while the
/// composition of the stage the replica left IS owed its stop.
#[tokio::test]
async fn a_stop_of_a_replication_does_not_empty_the_stage_the_publisher_is_in() {
    // The rows differ in how many publishers of the one source stage are replicated into the
    // destination stage, every replication being stopped: however many leave the destination
    // stage, each publisher stays in the stage it publishes in.
    for publishers in [1usize, 2] {
        let case = &format!("{publishers} publisher(s) replicated out of one source stage");
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.create_encoder_configuration(case).await;
        let source = emulator.create_stage(case, "source").await;
        let destination = emulator.create_stage(case, "destination").await;
        let mut replicated = Vec::new();
        for _ in 0..publishers {
            let publisher = emulator.create_token(case, &source).await;
            emulator
                .replicate(case, &source, &destination, &publisher)
                .await;
            replicated.push(publisher);
        }

        // One composition of each end of the replication.
        let of_source = emulator.start(case, &source, &encoder).await;
        let of_destination = emulator.start(case, &destination, &encoder).await;

        for publisher in &replicated {
            emulator
                .stop_replication(case, &source, &destination, publisher)
                .await;
        }

        assert!(
            emulator.deadline(case, &of_source).is_none(),
            "case {case:?}: the publisher is still in the stage it publishes in, so its \
             composition must be owed no stop, got {:?}",
            emulator.deadline(case, &of_source)
        );
        assert!(
            emulator.deadline(case, &of_destination).is_some(),
            "case {case:?}: the stage the replica left holds nobody, so its composition must \
             be owed its stop, got {:?}",
            emulator.deadline(case, &of_destination)
        );

        // And the source stage's composition goes on composing, which is what a client sees.
        tokio::time::sleep(state::COMPOSITION_ACTIVATION_SPAN + SLACK).await;
        let (status, response) = emulator.read(&of_source).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the composition of the publisher's stage must still be reported, \
             got {status} {response}"
        );
        assert_eq!(
            member(&response["composition"], "state"),
            STATE_ACTIVE,
            "case {case:?}: the composition of the publisher's stage must still be composing, \
             got {response}"
        );
    }
}
