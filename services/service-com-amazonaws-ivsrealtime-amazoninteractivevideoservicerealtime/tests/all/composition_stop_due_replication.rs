// SPDX-License-Identifier: CC0-1.0

//! What a `StopComposition` answers over a store holding a replication whose reconnect
//! window the clock has already run out, and which nothing has written down yet.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body. There
//! are two exceptions, both the same seams the sibling replication and composition suites
//! use: a publisher dropping off a stage's DATA plane, which no operation of this control
//! plane performs (`state::leave_stage_session`), and bringing a deadline the service
//! recorded up to the present instead of waiting a published minute out.
//!
//! The rule under test, in two halves:
//!
//! 1. EVERY OPERATION WHOSE ANSWER TURNS ON A COMPOSITION writes down what the clock owes
//!    the REPLICATIONS before what it owes the compositions: a replication whose publisher
//!    stayed away for its whole reconnect window ends with its replica leaving the
//!    destination stage, which is one of the ways the stage a composition composites comes
//!    to hold nobody and so earns that composition its own stop. So the stop, the read, the
//!    delete of a configuration a composition's destination names and a tag write all
//!    answer over the one state, and none of them depends on whether another call ran
//!    first.
//! 2. EACH END THE CLOCK OWED IS WRITTEN AS OF THE INSTANT IT FELL DUE, never as of the
//!    call that notices it — so the empty-stage deadline it earns a composition, and the
//!    end that deadline begins, run out of that instant and a store materialised late holds
//!    exactly what one materialised on time held.
//!
//! The rows span what the rule generalises over: the reconnect window the replication was
//! started with (the model's default and both ends of its published range), how the
//! publisher left the stage it publishes in, how many replicas the composited stage held
//! and whether it also held a participant of its own — crossed with how long ago the clock
//! owed the last of those ends, which places the composition at each step of the lifecycle
//! the published timeout walks it through.

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

/// How long after its stage was left empty a composition is owed its stop, read from the
/// crate so this suite cannot drift from the span the service enforces.
const STOP_DUE_AFTER: Duration = state::COMPOSITION_EMPTY_STAGE_STOP_SPAN;

/// The span the service gives the `STOPPING` -> `STOPPED` step of a stop, read from the
/// crate for the same reason.
const STOP_SPAN: Duration = state::COMPOSITION_STOP_SPAN;

/// How long the service goes on reporting a stopped composition before the ephemeral
/// resource is deleted, read from the crate for the same reason.
const RETENTION: Duration = state::COMPOSITION_RETENTION_SPAN;

/// How far apart the deadlines of several replications of one row are put, so the stage they
/// replicate into is left empty by the LAST of them and not by the first.
const STAGGER: Duration = Duration::from_secs(5);

/// The margin every row keeps from the boundary of a span, so the milliseconds a case spends
/// in the emulator cannot carry it into the next step of the lifecycle.
const MARGIN: Duration = Duration::from_secs(1);

// The `CompositionState` values this suite asserts, from
// `smithy_query(query="shape",shape="CompositionState")`:
//   STARTING | ACTIVE | STOPPING | FAILED | STOPPED
const STATE_STARTING: &str = "STARTING";
const STATE_ACTIVE: &str = "ACTIVE";
const STATE_STOPPING: &str = "STOPPING";
const STATE_STOPPED: &str = "STOPPED";

/// The modelled error a call naming a composition the clock has reclaimed is answered with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The error code the TAGGING surface answers the same reclaim with — its own, not the
/// modelled one the resource operations answer (see `tagging.rs`).
const TAGGING_NOT_FOUND: &str = "NotFoundException";

/// The modelled error a stop of a composition that is no longer composing is answered with.
const CONFLICT: &str = "ConflictException";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it,
/// plus the engine handle the data-plane departures and the clock are reached through.
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

    /// An operation bound to `POST /<Operation>`, which is every operation this suite calls
    /// except the tagging ones.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// `TagResource`, bound to `POST /tags/{resourceArn}`, carrying the ARN in the URI the
    /// way a client does: percent-encoded into one path segment.
    async fn tag(&self, arn: &str) -> (u16, Value) {
        let body = json!({ "tags": { "team": "video" } });
        self.send(
            "POST",
            &format!("/tags/{}", encode_path_label(arn)),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(&self, method: &str, path: &str, body: Option<bytes::Bytes>) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
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

    /// An encoder configuration, reported by its ARN — the resource a composition's channel
    /// destination names and goes on holding back until its stop has finished.
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

    /// A publisher of `stage_arn`, from a token issued for it: the identity the model says a
    /// `participantId` comes from ("assigned by IVS and returned by
    /// `CreateParticipantToken`").
    async fn publisher(&self, case: &str, stage_arn: &str) -> String {
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

    /// Replicate the publisher `participant_id` of `source` into `destination`, with the
    /// window this row asked to reconnect in, asserting the start was accepted — which is
    /// what puts a replica into `destination`.
    async fn replicate(
        &self,
        case: &str,
        source: &str,
        destination: &str,
        participant_id: &str,
        window: Option<i64>,
    ) {
        let mut request = json!({
            "sourceStageArn": source,
            "destinationStageArn": destination,
            "participantId": participant_id,
        });
        if let Some(window) = window {
            request["reconnectWindowSeconds"] = json!(window);
        }
        let (status, response) = self.call("StartParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// `StartComposition` against `stage_arn`, with one channel destination naming
    /// `encoder`, and answer the ARN the service minted for it.
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
        let (status, response) = self.call("StartComposition", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartComposition {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["composition"], "arn")
    }

    /// `DisconnectParticipant`, asserting the call was accepted.
    async fn disconnect(&self, case: &str, stage_arn: &str, participant_id: &str) {
        let request = json!({ "stageArn": stage_arn, "participantId": participant_id });
        let (status, response) = self.call("DisconnectParticipant", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DisconnectParticipant {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// The `activeSessionId` `GetStage` reports for `stage_arn`.
    async fn active_session(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage on {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
    }

    /// `StopComposition`, answering the status and body unchanged — a stop of a composition
    /// the clock has already ended is one of the answers under test.
    async fn stop(&self, composition_arn: &str) -> (u16, Value) {
        self.call("StopComposition", &json!({ "arn": composition_arn }))
            .await
    }

    /// `GetComposition`, answering the status and body unchanged for the same reason.
    async fn read(&self, composition_arn: &str) -> (u16, Value) {
        self.call("GetComposition", &json!({ "arn": composition_arn }))
            .await
    }

    /// Record a participant joining `stage_arn` as a publisher, the way the stage's data
    /// plane records one.
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

    /// Record the publisher `participant_id` dropping off the DATA plane of `stage_arn`:
    /// the departure no operation of this control plane performs, and the one that leaves
    /// what the clock then owes its replications unwritten.
    fn drop_off(&self, case: &str, stage_arn: &str, session_id: &str, participant_id: &str) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        assert!(
            state::leave_stage_session(&mut store, stage_arn, session_id, participant_id),
            "case {case:?}: the departure of {participant_id:?} from {stage_arn:?} must be \
             recorded"
        );
    }

    /// Put the deadline the service recorded for each of `publishers`' replications into the
    /// PAST: the last of them fell due `lapsed` ago and each one before it one [`STAGGER`]
    /// earlier, so the composited stage has held nobody since `lapsed` ago — the wait of a
    /// published minute, observable without waiting one out. Answers that instant.
    ///
    /// Refuses to invent one: each departure has to have earned its replication a deadline
    /// already, and none of them may have been written down yet.
    fn run_out_the_windows(
        &self,
        case: &str,
        source: &str,
        publishers: &[String],
        lapsed: Duration,
    ) -> SystemTime {
        let emptied = SystemTime::now() - lapsed;
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let replications = store
            .participant_replicas
            .get_mut(source)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {source} must hold the replications it sources")
            });
        for (index, publisher) in publishers.iter().rev().enumerate() {
            let earlier = STAGGER * u32::try_from(index).expect("a handful of publishers");
            let mut moved = 0usize;
            for replication in replications
                .iter_mut()
                .filter(|replication| replication.participant_id == *publisher)
            {
                assert!(
                    replication.reconnect_deadline.is_some(),
                    "case {case:?}: the replication of {publisher} must already carry the \
                     deadline its publisher's departure earned it"
                );
                assert!(
                    replication.is_active(),
                    "case {case:?}: the replication of {publisher} must still be running, so \
                     its end is one nothing has written down"
                );
                replication.reconnect_deadline = Some(emptied - earlier);
                moved += 1;
            }
            assert_eq!(
                moved, 1,
                "case {case:?}: exactly one replication of {publisher} must be held, \
                 found {moved}"
            );
        }
        emptied
    }

    /// Whether this store still holds the record of the composition `arn`.
    fn holds(&self, arn: &str) -> bool {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let store = handle.lock();
        store.compositions.contains_key(arn)
    }

    /// The empty-stage deadline the service recorded for the composition `arn`: the instant
    /// the clock owes it the stop its empty stage earned, or `None` while somebody is in
    /// that stage.
    fn deadline(&self, case: &str, arn: &str) -> Option<SystemTime> {
        self.record(case, arn).empty_stage_stop_at
    }

    /// The instant the service recorded the composition `arn`'s broadcast as having ended,
    /// or `None` while it is still composing.
    fn ended_at(&self, case: &str, arn: &str) -> Option<SystemTime> {
        self.record(case, arn).ended_at
    }

    fn record(&self, case: &str, arn: &str) -> state::CompositionRecord {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let store = handle.lock();
        store
            .compositions
            .get(arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {arn}"))
            .clone()
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> String {
    response["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .to_string()
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

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// How the publisher of a replication left the stage it publishes in.
#[derive(Clone, Copy, Debug)]
enum Departure {
    /// It dropped off the stage's data plane, which no control-plane call observes.
    DataPlaneDrop,
    /// A caller disconnected it with `DisconnectParticipant`.
    Disconnected,
}

/// How the composited stage came to be held only by replicas whose replications the clock
/// owes an end.
#[derive(Clone, Copy, Debug)]
struct Arrangement {
    case: &'static str,
    /// `reconnectWindowSeconds` the start asked for, or `None` to omit the member — the
    /// default the model documents ("Default: 0"). The published range is 0..=300
    /// (`smithy_query(query="operation",operation="StartParticipantReplication")`).
    window: Option<i64>,
    departure: Departure,
    /// How many publishers of the one source stage are replicated into the composited stage.
    replicas: usize,
    /// Whether the composited stage also held a participant of its own, disconnected while
    /// the replicas were still in it — so nothing was owed then either.
    local_participant: bool,
}

/// The arrangements every lapse below is exercised over.
const ARRANGEMENTS: [Arrangement; 4] = [
    Arrangement {
        case: "one replica, publisher dropped off the data plane, no window asked for",
        window: None,
        departure: Departure::DataPlaneDrop,
        replicas: 1,
        local_participant: false,
    },
    Arrangement {
        case: "one replica, publisher disconnected, the longest published window",
        window: Some(300),
        departure: Departure::Disconnected,
        replicas: 1,
        local_participant: false,
    },
    Arrangement {
        case: "two replicas of two publishers, ending a stagger apart",
        window: Some(30),
        departure: Departure::Disconnected,
        replicas: 2,
        local_participant: false,
    },
    Arrangement {
        case: "one replica beside a participant of the stage's own, which left first",
        window: Some(0),
        departure: Departure::DataPlaneDrop,
        replicas: 1,
        local_participant: true,
    },
];

/// Where the composition stands by the time the call under test is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// Still composing: the stop is the one this composition gets, the modelled 200.
    Composing,
    /// Its end has already begun, so a second stop would move a terminal state backwards —
    /// the modelled `ConflictException`.
    Ending,
    /// Stopped long enough ago that the ephemeral resource is gone — the modelled
    /// `ResourceNotFoundException`.
    Reclaimed,
}

impl Expected {
    /// The HTTP status `StopComposition` answers a composition in this state with.
    fn status(self) -> u16 {
        match self {
            Self::Composing => 200,
            Self::Ending => 409,
            Self::Reclaimed => 404,
        }
    }

    /// The modelled error code that answer carries, and `None` for the success.
    fn error(self) -> Option<&'static str> {
        match self {
            Self::Composing => None,
            Self::Ending => Some(CONFLICT),
            Self::Reclaimed => Some(NOT_FOUND),
        }
    }
}

/// One row: an arrangement, and how long ago the clock owed the end that left the composited
/// stage holding nobody.
#[derive(Clone, Copy, Debug)]
struct Case {
    arrangement: Arrangement,
    lapse: &'static str,
    lapsed: Duration,
    expected: Expected,
}

impl Case {
    fn case(&self) -> String {
        format!("{} / {}", self.arrangement.case, self.lapse)
    }

    /// The status a `DeleteEncoderConfiguration` of the configuration this row's composition
    /// names must answer with, or `None` where the row straddles a span boundary and either
    /// answer is right: the model tells a caller to "first use StopComposition and wait for
    /// it to complete", so a composition that is starting, composing or still stopping holds
    /// the configuration back and one whose stop has finished does not.
    fn encoder_delete(&self) -> Option<u16> {
        match self.expected {
            Expected::Composing => Some(409),
            Expected::Reclaimed => Some(200),
            // Inside the stop the configuration is still held back, after it it is not, and
            // this row is only unambiguous once the stop span is a margin behind it.
            Expected::Ending if self.lapsed >= STOP_DUE_AFTER + STOP_SPAN + MARGIN => Some(200),
            Expected::Ending => None,
        }
    }
}

/// Every arrangement at every step of the lifecycle the published timeout walks a
/// composition of an emptied stage through.
fn cases() -> Vec<Case> {
    // (the lapse's name, how long ago the stage fell empty, where that leaves the
    // composition): the stop is owed one STOP_DUE_AFTER after the stage was left empty, is
    // finished one STOP_SPAN after that, and the record is reclaimed one RETENTION after
    // THAT — one published minute from the instant the stage fell empty, all told.
    let lapses: [(&'static str, Duration, Expected); 5] = [
        ("the end fell due a moment ago", MARGIN, Expected::Composing),
        (
            "halfway to the stop the empty stage earns it",
            STOP_DUE_AFTER / 2,
            Expected::Composing,
        ),
        (
            "a margin inside the timeout",
            STOP_DUE_AFTER - MARGIN,
            Expected::Composing,
        ),
        (
            "past the stop the empty stage earned it",
            STOP_DUE_AFTER + STOP_SPAN + RETENTION / 2,
            Expected::Ending,
        ),
        (
            "past the whole published minute",
            STOP_DUE_AFTER + STOP_SPAN + RETENTION + MARGIN,
            Expected::Reclaimed,
        ),
    ];
    ARRANGEMENTS
        .iter()
        .flat_map(|arrangement| {
            lapses.iter().map(move |(lapse, lapsed, expected)| Case {
                arrangement: *arrangement,
                lapse,
                lapsed: *lapsed,
                expected: *expected,
            })
        })
        .collect()
}

/// What one row's arrangement left in the store.
struct Arranged {
    composition: String,
    /// The instant the composited stage was left holding nobody: the deadline of the LAST
    /// replication the clock owes an end.
    emptied: SystemTime,
}

/// Stand one row up: a composition of a stage held by replicas, started while that stage is
/// OCCUPIED, whose publishers then leave the stage they publish in — so the only thing that
/// can put the composition on a deadline is the end the clock owes those replications, and
/// nothing has written that end down.
async fn arrange(emulator: &RealTime, row: &Case, encoder: &str) -> Arranged {
    let case = row.case();
    let case = case.as_str();
    let source = emulator.stage(case, "source").await;
    let composited = emulator.stage(case, "composited").await;

    let mut publishers = Vec::new();
    for _ in 0..row.arrangement.replicas {
        let publisher = emulator.publisher(case, &source).await;
        emulator
            .replicate(
                case,
                &source,
                &composited,
                &publisher,
                row.arrangement.window,
            )
            .await;
        publishers.push(publisher);
    }
    let local = if row.arrangement.local_participant {
        let local = emulator.publisher(case, &composited).await;
        emulator.join(case, &composited, &local);
        Some(local)
    } else {
        None
    };

    let composition = emulator
        .start_composition(case, &composited, encoder)
        .await;
    assert!(
        emulator.deadline(case, &composition).is_none(),
        "case {case:?}: a composition of a stage that holds participants must be owed no \
         stop, got {:?}",
        emulator.deadline(case, &composition)
    );

    // The stage's own participant goes first, while the replicas are still in it: nothing is
    // owed then either.
    if let Some(local) = &local {
        emulator.disconnect(case, &composited, local).await;
        assert!(
            emulator.deadline(case, &composition).is_none(),
            "case {case:?}: the replicas still hold the composited stage, so the composition \
             must be owed no stop, got {:?}",
            emulator.deadline(case, &composition)
        );
    }

    // Each publisher leaves the stage it publishes in, which earns the replication it
    // sources its end one window later.
    let source_session = emulator.active_session(case, &source).await;
    for publisher in &publishers {
        match row.arrangement.departure {
            Departure::DataPlaneDrop => {
                emulator.drop_off(case, &source, &source_session, publisher);
            }
            Departure::Disconnected => emulator.disconnect(case, &source, publisher).await,
        }
    }
    assert!(
        emulator.deadline(case, &composition).is_none(),
        "case {case:?}: until the end the clock owes those replications is written down the \
         replicas are still in the composited stage, so no empty-stage deadline can have \
         been recorded, got {:?}",
        emulator.deadline(case, &composition)
    );

    let emptied = emulator.run_out_the_windows(case, &source, &publishers, row.lapsed);
    Arranged {
        composition,
        emptied,
    }
}

// ---------------------------------------------------------------------------
// Rule 1 — the stop answers over the state the clock had already reached
// ---------------------------------------------------------------------------

/// `StopComposition` answers the state the clock had already reached, with no other call in
/// front of it: it stops a composition that is still composing, refuses one whose end the
/// clock already began, and reports one whose ephemeral resource it has reclaimed as gone.
#[tokio::test]
async fn the_stop_answers_the_state_the_clock_had_already_reached() {
    for row in cases() {
        let case = row.case();
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.encoder_configuration(case).await;
        let arranged = arrange(&emulator, &row, &encoder).await;

        let (status, response) = emulator.stop(&arranged.composition).await;
        assert_eq!(
            status,
            row.expected.status(),
            "case {case:?}: a stop of a composition the clock has left {:?} must answer HTTP \
             {}, got {status} {response}",
            row.expected,
            row.expected.status()
        );
        if let Some(expected) = row.expected.error() {
            assert_eq!(
                error_code(&response),
                expected,
                "case {case:?}: expected the modelled {expected}, got {response}"
            );
        }

        // A stop that was accepted took effect: the read back reports the end it began, with
        // the instant of this call rather than one the clock had reached.
        if row.expected == Expected::Composing {
            let (status, response) = emulator.read(&arranged.composition).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the stopped composition must still be reported, \
                 got {status} {response}"
            );
            let composition = response["composition"].clone();
            let state = member(&composition, "state");
            assert!(
                [STATE_STOPPING, STATE_STOPPED].contains(&state.as_str()),
                "case {case:?}: a composition this stop accepted must be ending, \
                 got {composition}"
            );
            assert!(
                composition.get("endTime").is_some(),
                "case {case:?}: a stopped composition must report when its broadcast ended, \
                 got {composition}"
            );
            let ended_at = emulator.ended_at(case, &arranged.composition);
            assert!(
                ended_at.is_some_and(|ended| ended > arranged.emptied),
                "case {case:?}: a stop this caller asked for ends the broadcast at this call, \
                 which is after the stage fell empty at {:?}, got {ended_at:?}",
                arranged.emptied
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1 — a read at the same instant reports the state the stop acts on
// ---------------------------------------------------------------------------

/// The same arrangement READ instead of stopped reports the very state the stop above acted
/// on — and rule 2: the deadline the empty stage earned, and the end it begins, are dated at
/// the instant the clock ran the reconnect window out, not at the call that wrote it down.
#[tokio::test]
async fn a_read_at_the_same_instant_reports_the_state_the_stop_acts_on() {
    for row in cases() {
        let case = row.case();
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.encoder_configuration(case).await;
        let arranged = arrange(&emulator, &row, &encoder).await;

        let (status, response) = emulator.read(&arranged.composition).await;
        match row.expected {
            Expected::Composing => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: inside the published timeout the composition must still \
                     be reported, got {status} {response}"
                );
                let composition = response["composition"].clone();
                let state = member(&composition, "state");
                assert!(
                    [STATE_STARTING, STATE_ACTIVE].contains(&state.as_str()),
                    "case {case:?}: inside the published timeout the composition must still \
                     be composing, got {composition}"
                );
                assert!(
                    composition.get("endTime").is_none(),
                    "case {case:?}: a composition that has not stopped carries no end time, \
                     got {composition}"
                );
                let deadline = emulator.deadline(case, &arranged.composition);
                assert_eq!(
                    deadline,
                    Some(arranged.emptied + STOP_DUE_AFTER),
                    "case {case:?}: the stop must be owed one {STOP_DUE_AFTER:?} after the \
                     instant the clock emptied the stage ({:?}), not after this call, \
                     got {deadline:?}",
                    arranged.emptied
                );
            }
            Expected::Ending => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a stopped composition is reported until its record \
                     expires, got {status} {response}"
                );
                let composition = response["composition"].clone();
                let state = member(&composition, "state");
                assert!(
                    [STATE_STOPPING, STATE_STOPPED].contains(&state.as_str()),
                    "case {case:?}: the composition of a stage the clock emptied must be \
                     ending, got {composition}"
                );
                assert!(
                    composition.get("endTime").is_some(),
                    "case {case:?}: a composition the clock stopped must report when its \
                     broadcast ended, got {composition}"
                );
                let ended_at = emulator.ended_at(case, &arranged.composition);
                assert_eq!(
                    ended_at,
                    Some(arranged.emptied + STOP_DUE_AFTER),
                    "case {case:?}: the end the clock began must be dated at the deadline it \
                     ran out ({:?} plus {STOP_DUE_AFTER:?}), not at this call, \
                     got {ended_at:?}",
                    arranged.emptied
                );
            }
            Expected::Reclaimed => {
                assert_eq!(
                    status, 404,
                    "case {case:?}: a composition whose record the clock reclaimed must be \
                     read as HTTP 404, got {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    NOT_FOUND,
                    "case {case:?}: expected the modelled {NOT_FOUND}, got {response}"
                );
                assert!(
                    !emulator.holds(&arranged.composition),
                    "case {case:?}: the reclaimed record must be gone from the store"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1 and 2 together — an unrelated read changes nothing
// ---------------------------------------------------------------------------

/// The stop answers exactly the same with an unrelated read in front of it as with nothing
/// in front of it: the read only writes down what the clock already owed, and writes it down
/// as of the instant it fell due.
#[tokio::test]
async fn an_unrelated_read_does_not_change_what_the_stop_answers() {
    for row in cases() {
        let case = row.case();
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.encoder_configuration(case).await;
        let arranged = arrange(&emulator, &row, &encoder).await;

        // The unrelated calls: one read of this composition and one listing of them all.
        let (read_status, read_response) = emulator.read(&arranged.composition).await;
        let (listed_status, listed_response) = emulator.call("ListCompositions", &json!({})).await;
        assert_eq!(
            listed_status, 200,
            "case {case:?}: ListCompositions must answer HTTP 200, \
             got {listed_status} {listed_response}"
        );

        let (status, response) = emulator.stop(&arranged.composition).await;
        assert_eq!(
            status,
            row.expected.status(),
            "case {case:?}: the stop must answer HTTP {} whether or not a read ran first \
             (the read answered {read_status} {read_response}), got {status} {response}",
            row.expected.status()
        );
        if let Some(expected) = row.expected.error() {
            assert_eq!(
                error_code(&response),
                expected,
                "case {case:?}: expected the modelled {expected}, got {response}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1 — the delete and the tag write see the same state
// ---------------------------------------------------------------------------

/// The other two calls whose answer turns on a composition see the same state as the stop
/// does, each as the FIRST call made since the clock ran the reconnect window out: the
/// delete of the encoder configuration the composition's destination names, and a tag write
/// on the composition itself.
#[tokio::test]
async fn the_delete_and_the_tag_write_see_the_state_the_clock_reached() {
    for row in cases() {
        let case = row.case();
        let case = case.as_str();

        if let Some(expected) = row.encoder_delete() {
            let emulator = RealTime::new();
            let encoder = emulator.encoder_configuration(case).await;
            let _arranged = arrange(&emulator, &row, &encoder).await;
            let (status, response) = emulator
                .call("DeleteEncoderConfiguration", &json!({ "arn": encoder }))
                .await;
            assert_eq!(
                status, expected,
                "case {case:?}: with the composition left {:?} by the clock, deleting \
                 {encoder} must answer HTTP {expected}, got {status} {response}",
                row.expected
            );
            if expected == 409 {
                assert_eq!(
                    error_code(&response),
                    CONFLICT,
                    "case {case:?}: expected the modelled {CONFLICT}, got {response}"
                );
            }
        }

        let emulator = RealTime::new();
        let encoder = emulator.encoder_configuration(case).await;
        let arranged = arrange(&emulator, &row, &encoder).await;
        let (status, response) = emulator.tag(&arranged.composition).await;
        // `TagResourceResponse` models no member, so the success is the framed empty answer.
        let expected = if row.expected == Expected::Reclaimed {
            404
        } else {
            204
        };
        assert_eq!(
            status, expected,
            "case {case:?}: with the composition left {:?} by the clock, a tag write on it \
             must answer HTTP {expected}, got {status} {response}",
            row.expected
        );
        if expected == 404 {
            assert_eq!(
                error_code(&response),
                TAGGING_NOT_FOUND,
                "case {case:?}: expected the tagging surface's {TAGGING_NOT_FOUND}, \
                 got {response}"
            );
        }
    }
}
