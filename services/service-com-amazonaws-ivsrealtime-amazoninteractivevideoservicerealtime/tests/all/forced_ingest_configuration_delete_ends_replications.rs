// SPDX-License-Identifier: CC0-1.0

//! What a forced `DeleteIngestConfiguration` does to the replications its publishers source.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body — never
//! an internal call. There are three exceptions. Bringing a publisher INTO a stage is the
//! stage's DATA plane, which no operation of this control plane serves: it is
//! `state::join_stage_session`, exactly as the `DisconnectParticipant` and
//! `ListParticipants` suites beside this one do it. Running a reconnect window out is moving
//! the deadline the service recorded up to the present — the one value its materialisation
//! compares the clock against — so a window of up to five minutes is observed without
//! waiting one out. And the deadline a composition of the destination stage is owed is read
//! off the store, the way the `composition_empty_stage_timeout` suite reads it, because it
//! is what the CALL recorded rather than what a later read happens to notice.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A FORCED DELETE TAKES EVERY PUBLISHER OF THE CONFIGURATION OUT OF THE STAGE IT
//!    PUBLISHES IN — "the participant is disconnected from the stage, followed by deletion
//!    of the IngestConfiguration" — so every replication those publishers source is put on
//!    a deadline one `reconnectWindowSeconds` after that instant, the model's own rule for a
//!    disconnected publisher ("if the participant disconnects and then reconnects within the
//!    specified interval, replication will continue to be `ACTIVE`").
//! 2. THE CALL WRITES DOWN EVERY END THE CLOCK ALREADY OWES. With the window the model
//!    documents as the default the end is owed at the very instant the publisher went, so
//!    by the time the delete answers the replication is `STOPPED`, its replica has left the
//!    destination stage, that stage reports no session once the replica was the last one in
//!    it — so a composition of it is owed its stop one
//!    `state::COMPOSITION_EMPTY_STAGE_STOP_SPAN` after THIS call — and the end is the
//!    publisher's `REPLICATION_STOPPED` occurrence naming that destination, exactly as
//!    `DisconnectParticipant` and `StopParticipantReplication` leave them.
//! 3. A WINDOW THE CALLER ASKED FOR LEAVES THE END OWED until it runs out: the replication
//!    is still `ACTIVE` with its replica still in the destination stage and that stage's
//!    composition owed nothing, and when the window has run out everything rule 2 states
//!    holds.
//! 4. ONLY THE REPLICATIONS OF THOSE PUBLISHERS END. Another publisher of the same source
//!    stage replicated into the same destination keeps running, a participant of the
//!    destination stage's own stays, and the deleted configuration is gone (the modelled
//!    404).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

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

/// The two values `ReplicationState` accepts, from
/// `smithy_query(query="shape",shape="ReplicationState")`: the state a start leaves a
/// replication in and the one its end leaves it in.
const ACTIVE: &str = "ACTIVE";
const STOPPED: &str = "STOPPED";

/// The two `ParticipantState` values of
/// `smithy_query(query="shape",shape="ParticipantState")`.
const CONNECTED: &str = "CONNECTED";
const DISCONNECTED: &str = "DISCONNECTED";

/// The `ReplicationType` a publisher replicated into another stage reports, from
/// `smithy_query(query="shape",shape="ReplicationType")`.
const REPLICATION_TYPE_SOURCE: &str = "SOURCE";

/// The occurrence name the end of a replication is recorded under, from
/// `smithy_query(query="shape",shape="EventName")`.
const REPLICATION_STOPPED: &str = "REPLICATION_STOPPED";

/// `ReconnectWindowSeconds`' modelled `@range`, from
/// `smithy_query(query="operation",operation="StartParticipantReplication")`: `0..300`.
const RECONNECT_WINDOW_MIN: i64 = 0;
const RECONNECT_WINDOW_MAX: i64 = 300;

/// How long after its stage was left empty a composition is owed its stop, read from the
/// crate so this suite cannot drift from the span the service enforces.
const STOP_DUE_AFTER: Duration = state::COMPOSITION_EMPTY_STAGE_STOP_SPAN;

/// The error code a read of a deleted configuration answers with, from
/// `smithy_query(query="operation",operation="GetIngestConfiguration")`.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

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

    /// A stage, created the way a client creates one, reported by its ARN.
    async fn stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name:?} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// An encoder configuration, for the one destination every composition this suite starts
    /// broadcasts to.
    async fn encoder_configuration(&self, case: &str) -> String {
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

    /// An ingest configuration of `stage_arn`, with the backup credentials beside its own
    /// when `redundant` asks for them, as the service reported it.
    async fn ingest_configuration(&self, case: &str, stage_arn: &str, redundant: bool) -> Value {
        let request = json!({
            "ingestProtocol": "RTMPS",
            "stageArn": stage_arn,
            "redundantIngest": redundant,
        });
        let (status, response) = self.call("CreateIngestConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// Issue a participant token for `stage_arn` and answer the participant id it carries —
    /// the other identity the model says a `participantId` comes from ("assigned by IVS and
    /// returned by `CreateParticipantToken`").
    async fn token_participant(&self, case: &str, stage_arn: &str) -> String {
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

    /// Record a participant joining `stage_arn` over the stage's data plane: publishing with
    /// `configuration_arn` when it reached the stage with an ingest configuration's
    /// credentials, and publishing on its own account when it did not.
    fn join(
        &self,
        case: &str,
        stage_arn: &str,
        participant_id: &str,
        configuration_arn: Option<&str>,
    ) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let joined = state::JoinedParticipant {
            participant_id: participant_id.to_string(),
            published: true,
            ingest_configuration_arn: configuration_arn.map(str::to_string),
            ..Default::default()
        };
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// Replicate `participant_id` of `source` into `destination`, with the window the row
    /// asked for, asserting the start was accepted.
    async fn replicate(
        &self,
        case: &str,
        source: &str,
        destination: &str,
        participant_id: &str,
        window: Option<i64>,
    ) {
        let request = start_request(source, destination, participant_id, window);
        let (status, response) = self.call("StartParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// Start a composition of `stage_arn` with one destination naming `encoder`, and answer
    /// its ARN.
    async fn compose(&self, case: &str, stage_arn: &str, encoder: &str) -> String {
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
        member(&response["composition"], "arn")
    }

    /// `DeleteIngestConfiguration` with `force`, asserting the modelled success, and
    /// answering the window the call was made in.
    async fn force_delete(&self, case: &str, configuration_arn: &str) -> (SystemTime, SystemTime) {
        let request = json!({ "arn": configuration_arn, "force": true });
        let before = SystemTime::now();
        let (status, response) = self.call("DeleteIngestConfiguration", &request).await;
        let after = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {case:?}: a forced DeleteIngestConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        (before, after)
    }

    /// What `GetIngestConfiguration` answers for `configuration_arn`.
    async fn read_configuration(&self, configuration_arn: &str) -> (u16, Value) {
        self.call(
            "GetIngestConfiguration",
            &json!({ "arn": configuration_arn }),
        )
        .await
    }

    /// The `replicationState` `ListParticipantReplicas` reports for the one replica of
    /// `participant_id` in `destination_stage_arn`.
    async fn replication_state(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
        destination_stage_arn: &str,
    ) -> String {
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
        member(matching[0], "replicationState")
    }

    /// The `participant` `GetParticipant` reports for one participant of one session.
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

    /// The deadline the service recorded for one replication: the instant the clock ends it
    /// at because the publisher it replicates has gone, or `None` while that publisher is
    /// connected — and `None` once the replication has ended, which is owed no further end.
    fn replication_deadline(
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

    /// The deadline the service recorded for the composition `arn` names: the instant the
    /// clock owes it the stop its empty stage earned, or `None` while somebody is in that
    /// stage.
    fn composition_deadline(&self, case: &str, arn: &str) -> Option<SystemTime> {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let store = handle.lock();
        store
            .compositions
            .get(arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {arn}"))
            .empty_stage_stop_at
    }

    /// Bring every deadline the service recorded for `participant_id`'s replications up to
    /// the present, so what it does when a reconnect window runs out is observable without
    /// waiting one out, and answer how many it moved.
    ///
    /// Refuses to invent one: the deadline has to be there already, which is rule 1.
    fn run_out_the_window(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
    ) -> usize {
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
                 carry the deadline its publisher's removal earned it",
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

/// An IVS ARN of `resource_type` in [`REGION`], naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
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

/// One row of the class "an ingest configuration whose publishers source replications is
/// deleted with `force`". The rows differ in what the rule generalises over: the window the
/// starts asked to reconnect in, how many destination stages the publishers are replicated
/// into, how many publishers the configuration has (its own credential, and the backup ones
/// redundant ingest adds), whether another publisher of the source stage shares the first
/// destination stage, and whether that stage holds a participant of its own.
#[derive(Clone, Copy, Debug)]
struct Case {
    case: &'static str,
    /// `reconnectWindowSeconds`, or `None` to omit the member — which is the default the
    /// model documents ("Default: 0").
    window: Option<i64>,
    /// How many destination stages every publisher of the configuration is replicated into.
    destinations: usize,
    /// Whether the configuration carries the backup credentials redundant ingest adds, each
    /// of which is a publisher of it in its own right.
    redundant: bool,
    /// Whether a publisher of the source stage that is NOT one of the configuration's is
    /// replicated into the first destination stage.
    other_publisher: bool,
    /// Whether the first destination stage holds a participant of its own.
    local_participant: bool,
}

impl Case {
    /// The window this row's replications are given, as the span the clock measures: the
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
            redundant: false,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "starts that asked for the minimum window the model allows",
            window: Some(RECONNECT_WINDOW_MIN),
            destinations: 2,
            redundant: false,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "the default window, one publisher replicated into three stages",
            window: None,
            destinations: 3,
            redundant: false,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "the default window, with another publisher sharing the destination stage",
            window: None,
            destinations: 1,
            redundant: false,
            other_publisher: true,
            local_participant: false,
        },
        Case {
            case: "the default window, with the destination stage holding a participant of \
                   its own",
            window: None,
            destinations: 1,
            redundant: false,
            other_publisher: false,
            local_participant: true,
        },
        Case {
            case: "the default window, with the configuration's redundant publishers \
                   broadcasting too",
            window: None,
            destinations: 2,
            redundant: true,
            other_publisher: false,
            local_participant: false,
        },
        Case {
            case: "starts that asked for a window halfway up the modelled range",
            window: Some(RECONNECT_WINDOW_MAX / 2),
            destinations: 2,
            redundant: false,
            other_publisher: false,
            local_participant: true,
        },
        Case {
            case: "starts that asked for the maximum window the model allows",
            window: Some(RECONNECT_WINDOW_MAX),
            destinations: 1,
            redundant: false,
            other_publisher: true,
            local_participant: false,
        },
        Case {
            case: "a window to run out, with every publisher of a redundant configuration \
                   replicated",
            window: Some(30),
            destinations: 2,
            redundant: true,
            other_publisher: true,
            local_participant: true,
        },
    ]
}

/// Everything one row set up.
struct Arranged {
    source: String,
    destinations: Vec<String>,
    configuration: String,
    /// Every publisher of the configuration: its own credential's participant, and the
    /// backup credentials' when the row asked for redundant ingest.
    publishers: Vec<String>,
    /// The publisher of the source stage that is not one of the configuration's, replicated
    /// into the first destination stage, when the row has one.
    bystander: Option<String>,
    /// The participant of the first destination stage's own, when the row has one.
    local: Option<String>,
    source_session: String,
    /// The session of each destination stage, in the same order as [`Self::destinations`].
    destination_sessions: Vec<String>,
    /// The composition of [`Self::composed`], started while that stage held the replicas.
    composition: String,
    /// Which destination stage the composition composites: the LAST one, which only the
    /// first can be shared with a bystander or a participant of its own.
    composed: usize,
}

impl Arranged {
    /// Whether the forced delete leaves the composited destination stage with nobody in it:
    /// true unless somebody that delete does not touch is in that stage.
    fn composed_stage_emptied(&self) -> bool {
        self.composed > 0 || (self.bystander.is_none() && self.local.is_none())
    }
}

/// Build one row's stages, publishers and replications, and assert the starts left every
/// replication running with its replica in the destination stage — the state the forced
/// delete acts on.
async fn arrange(emulator: &RealTime, row: Case) -> Arranged {
    let case = row.case;
    let source = emulator.stage(case, "source").await;
    let configuration = emulator
        .ingest_configuration(case, &source, row.redundant)
        .await;
    let configuration_arn = member(&configuration, "arn");
    // Every credential of the configuration is a publisher of it: its own, and each backup
    // one redundant ingest minted beside it.
    let mut publishers = vec![member(&configuration, "participantId")];
    for credential in configuration["redundantIngestCredentials"]
        .as_array()
        .unwrap_or(&Vec::new())
    {
        publishers.push(member(credential, "participantId"));
    }
    assert_eq!(
        publishers.len() > 1,
        row.redundant,
        "case {case:?}: a configuration with redundantIngest must carry backup credentials \
         beside its own, got {configuration}"
    );
    for publisher in &publishers {
        emulator.join(case, &source, publisher, Some(&configuration_arn));
    }

    let mut destinations = Vec::new();
    for index in 0..row.destinations {
        let destination = emulator.stage(case, &format!("destination-{index}")).await;
        for publisher in &publishers {
            emulator
                .replicate(case, &source, &destination, publisher, row.window)
                .await;
        }
        destinations.push(destination);
    }
    let bystander = if row.other_publisher {
        let bystander = emulator.token_participant(case, &source).await;
        emulator.join(case, &source, &bystander, None);
        emulator
            .replicate(case, &source, &destinations[0], &bystander, row.window)
            .await;
        Some(bystander)
    } else {
        None
    };
    let local = if row.local_participant {
        let local = emulator.token_participant(case, &destinations[0]).await;
        emulator.join(case, &destinations[0], &local, None);
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
        for publisher in &publishers {
            let state = emulator
                .replication_state(case, &source, publisher, destination)
                .await;
            assert_eq!(
                state, ACTIVE,
                "case {case:?}: the started replication of {publisher} into {destination} must \
                 report {ACTIVE}, got {state:?}"
            );
            let replica = emulator.participant(case, destination, session, publisher).await;
            assert_eq!(
                member(&replica, "state"),
                CONNECTED,
                "case {case:?}: the replica of {publisher} in {destination} must be \
                 {CONNECTED}, got {replica}"
            );
        }
    }

    // The composition is of the LAST destination stage — the one no bystander and no
    // participant of its own is in unless the row has a single destination — and it is
    // started while that stage holds the replicas, so nothing is owed yet.
    let composed = destinations.len() - 1;
    let encoder = emulator.encoder_configuration(case).await;
    let composition = emulator
        .compose(case, &destinations[composed], &encoder)
        .await;
    let deadline = emulator.composition_deadline(case, &composition);
    assert!(
        deadline.is_none(),
        "case {case:?}: the composited stage holds the replicas, so its composition must be \
         owed no stop, got {deadline:?}"
    );

    Arranged {
        source,
        destinations,
        configuration: configuration_arn,
        publishers,
        bystander,
        local,
        source_session,
        destination_sessions,
        composition,
        composed,
    }
}

/// Assert every replication of the configuration's publishers has ended the way a stop ends
/// one, that the stages it named are left as a stop would leave them, and that nothing else
/// in them moved.
async fn assert_ended(emulator: &RealTime, row: Case, arranged: &Arranged) {
    let case = row.case;
    for (index, (destination, session)) in arranged
        .destinations
        .iter()
        .zip(&arranged.destination_sessions)
        .enumerate()
    {
        for publisher in &arranged.publishers {
            // RULE 2: the replication reports the other value of the modelled list.
            let state = emulator
                .replication_state(case, &arranged.source, publisher, destination)
                .await;
            assert_eq!(
                state, STOPPED,
                "case {case:?}: the replication of {publisher} into {destination} must report \
                 {STOPPED} once its publisher is out of the stage it published in, got {state:?}"
            );
            // RULE 2: the replica the replication put into that stage has left it.
            let replica = emulator
                .participant(case, destination, session, publisher)
                .await;
            assert_eq!(
                member(&replica, "state"),
                DISCONNECTED,
                "case {case:?}: the replica of {publisher} in {destination} must be \
                 {DISCONNECTED}, got {replica}"
            );
            // RULE 2: the end is an occurrence of the publisher's own session, naming the
            // destination the ended replication named.
            let events = emulator
                .events(case, &arranged.source, &arranged.source_session, publisher)
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
                "case {case:?}: the session {publisher} published in must record exactly one \
                 {REPLICATION_STOPPED} naming {destination}, got {events:?}"
            );
            assert_eq!(
                member(ends[0], "destinationSessionId"),
                *session,
                "case {case:?}: that occurrence must name the destination session, got {:?}",
                ends[0]
            );
            // RULE 2: the publisher itself is out of the stage it published in, and what it
            // reports about its replication is that replication's own state.
            let published_in = emulator
                .participant(case, &arranged.source, &arranged.source_session, publisher)
                .await;
            assert_eq!(
                member(&published_in, "state"),
                DISCONNECTED,
                "case {case:?}: the publisher {publisher} must be {DISCONNECTED} in the stage \
                 it published in, got {published_in}"
            );
            assert_eq!(
                (
                    member(&published_in, "replicationType"),
                    member(&published_in, "replicationState"),
                ),
                (
                    REPLICATION_TYPE_SOURCE.to_string(),
                    STOPPED.to_string()
                ),
                "case {case:?}: the publisher {publisher} must report the state of the \
                 replication it sourced, got {published_in}"
            );
        }

        // RULE 2: a destination stage the replicas leaving emptied reports no session; one
        // that still holds somebody keeps the session it was running.
        let held_by_others =
            index == 0 && (arranged.bystander.is_some() || arranged.local.is_some());
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
                "case {case:?}: {destination} was emptied by the replicas leaving, so it must \
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
            "case {case:?}: the replication of {bystander}, which the deleted configuration \
             never issued, must still report {ACTIVE}, got {state:?}"
        );
        let replica = emulator
            .participant(
                case,
                &arranged.destinations[0],
                &arranged.destination_sessions[0],
                bystander,
            )
            .await;
        assert_eq!(
            member(&replica, "state"),
            CONNECTED,
            "case {case:?}: the replica of {bystander} must still be {CONNECTED}, got {replica}"
        );
    }
    if let Some(local) = &arranged.local {
        let participant = emulator
            .participant(
                case,
                &arranged.destinations[0],
                &arranged.destination_sessions[0],
                local,
            )
            .await;
        assert_eq!(
            member(&participant, "state"),
            CONNECTED,
            "case {case:?}: the destination stage's own participant {local} must still be \
             {CONNECTED}, got {participant}"
        );
    }

    // RULE 4: the configuration itself is gone, whichever way its replications ended.
    let (status, response) = emulator.read_configuration(&arranged.configuration).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the deleted configuration must be gone, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        RESOURCE_NOT_FOUND,
        "case {case:?}: that read must answer the modelled {RESOURCE_NOT_FOUND}, got {response}"
    );
}

// ---------------------------------------------------------------------------
// Rules 1, 2 and 4 — the forced delete removes the publishers, and the window
// decides when the replications they source end
// ---------------------------------------------------------------------------

/// A forced delete puts every replication its publishers source on a deadline one window
/// later, and writes down every end that window has already made due before it answers.
#[tokio::test]
async fn a_forced_delete_ends_the_replications_its_publishers_source() {
    for row in cases() {
        let case = row.case;
        let emulator = RealTime::new();
        let arranged = arrange(&emulator, row).await;

        let (before, after) = emulator.force_delete(case, &arranged.configuration).await;

        if row.window().is_zero() {
            // RULE 2: with the window the model documents as the default, the end is owed at
            // the very instant the publishers went, so the delete has already written every
            // one of them down and no replication is left waiting for one. Read off the store
            // before any other call, because what is asserted is what THIS call left.
            for destination in &arranged.destinations {
                for publisher in &arranged.publishers {
                    let deadline = emulator.replication_deadline(
                        case,
                        &arranged.source,
                        publisher,
                        destination,
                    );
                    assert!(
                        deadline.is_none(),
                        "case {case:?}: the replication of {publisher} into {destination} has \
                         already ended, so it must wait for nothing, got {deadline:?}"
                    );
                }
            }
            // RULE 2: and because the replicas are out of the destination stage already, a
            // composition of a stage they emptied is owed its stop one span after THIS call.
            let deadline = emulator.composition_deadline(case, &arranged.composition);
            if arranged.composed_stage_emptied() {
                assert!(
                    deadline.is_some_and(|due| due >= before + STOP_DUE_AFTER
                        && due <= after + STOP_DUE_AFTER),
                    "case {case:?}: the composition of the stage the replicas left must be owed \
                     its stop one {STOP_DUE_AFTER:?} after the forced delete (between {:?} and \
                     {:?}), got {deadline:?}",
                    before + STOP_DUE_AFTER,
                    after + STOP_DUE_AFTER
                );
            } else {
                assert!(
                    deadline.is_none(),
                    "case {case:?}: the composited stage still holds a participant, so its \
                     composition must be owed no stop, got {deadline:?}"
                );
            }
            assert_ended(&emulator, row, &arranged).await;
            continue;
        }

        // RULE 1: a window the starts asked for leaves every replication of those publishers
        // owed its end one window after the instant the publisher went.
        for destination in &arranged.destinations {
            for publisher in &arranged.publishers {
                let deadline =
                    emulator.replication_deadline(case, &arranged.source, publisher, destination);
                assert!(
                    deadline.is_some_and(|due| due >= before + row.window()
                        && due <= after + row.window()),
                    "case {case:?}: the replication of {publisher} into {destination} must be \
                     owed its end one {:?} after its publisher was removed (between {:?} and \
                     {:?}), got {deadline:?}",
                    row.window(),
                    before + row.window(),
                    after + row.window()
                );
            }
        }

        // RULE 3: inside the window nothing has ended — the replication is still running,
        // its replica is still in the destination stage and a composition of that stage is
        // owed nothing.
        for (destination, session) in arranged
            .destinations
            .iter()
            .zip(&arranged.destination_sessions)
        {
            for publisher in &arranged.publishers {
                let state = emulator
                    .replication_state(case, &arranged.source, publisher, destination)
                    .await;
                assert_eq!(
                    state, ACTIVE,
                    "case {case:?}: inside the {:?} window the replication of {publisher} into \
                     {destination} must still report {ACTIVE}, got {state:?}",
                    row.window()
                );
                let replica = emulator
                    .participant(case, destination, session, publisher)
                    .await;
                assert_eq!(
                    member(&replica, "state"),
                    CONNECTED,
                    "case {case:?}: inside the window the replica of {publisher} in \
                     {destination} must still be {CONNECTED}, got {replica}"
                );
            }
            let active = emulator.active_session(case, destination).await;
            assert_eq!(
                active, *session,
                "case {case:?}: inside the window {destination} must keep the session its \
                 replicas are in, got {active:?}"
            );
        }
        let deadline = emulator.composition_deadline(case, &arranged.composition);
        assert!(
            deadline.is_none(),
            "case {case:?}: inside the window the composited stage still holds the replicas, \
             so its composition must be owed no stop, got {deadline:?}"
        );

        // The window runs out, and the service walks every step itself.
        for publisher in &arranged.publishers {
            let moved = emulator.run_out_the_window(case, &arranged.source, publisher);
            assert_eq!(
                moved,
                arranged.destinations.len(),
                "case {case:?}: every replication of {publisher} must carry a deadline to run \
                 out"
            );
        }
        assert_ended(&emulator, row, &arranged).await;
        let deadline = emulator.composition_deadline(case, &arranged.composition);
        assert_eq!(
            deadline.is_some(),
            arranged.composed_stage_emptied(),
            "case {case:?}: once the window has run out, the composition of the composited \
             stage must be owed a stop exactly when the replicas leaving emptied that stage, \
             got {deadline:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1 — the source stage is left as the departure of its publishers leaves it
// ---------------------------------------------------------------------------

/// "The participant is disconnected from the stage, followed by deletion of the
/// IngestConfiguration": the publishers are out of the session they published in by the time
/// the delete answers, so that session ends with the last of them — whatever the window the
/// replications they source were given.
#[tokio::test]
async fn a_forced_delete_takes_every_publisher_out_of_the_stage_it_published_in() {
    for row in cases() {
        let case = row.case;
        let emulator = RealTime::new();
        let arranged = arrange(&emulator, row).await;

        emulator.force_delete(case, &arranged.configuration).await;

        for publisher in &arranged.publishers {
            let participant = emulator
                .participant(case, &arranged.source, &arranged.source_session, publisher)
                .await;
            assert_eq!(
                member(&participant, "state"),
                DISCONNECTED,
                "case {case:?}: {publisher} must be {DISCONNECTED} in the stage it published \
                 in, got {participant}"
            );
        }
        // A session "ends after the last participant leaves the stage": only the row with a
        // publisher the delete does not touch keeps one.
        let active = emulator.active_session(case, &arranged.source).await;
        if arranged.bystander.is_some() {
            assert_eq!(
                active, arranged.source_session,
                "case {case:?}: the source stage still holds the publisher the configuration \
                 never issued, so it must keep the session it was running, got {active:?}"
            );
        } else {
            assert!(
                active.is_empty(),
                "case {case:?}: every participant of the source stage left with the \
                 configuration, so it must report no active session, got {active:?}"
            );
        }
    }
}
