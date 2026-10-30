// SPDX-License-Identifier: CC0-1.0

//! What the clock owes a composition whose stage holds no participant.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body — never
//! an internal call. The one exception is bringing participants INTO a stage, which no
//! operation of this control plane does: a participant joins over the stage's data plane
//! with the token or ingest key it was issued, which is `state::join_stage_session`, so
//! these cases join through it exactly as the `DisconnectParticipant` and `ListParticipants`
//! suites beside this one do.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A STAGE LEFT WITHOUT PARTICIPANTS PUTS ITS COMPOSITIONS ON A DEADLINE. Whenever the
//!    stage a composition composites comes to hold no participant — the start finding it
//!    empty, the last participant disconnecting, the publisher of an ingest configuration
//!    removed with it — every composition of that stage is owed its stop one
//!    `state::COMPOSITION_EMPTY_STAGE_STOP_SPAN` after THAT instant, which is one whole end
//!    inside the published `state::COMPOSITION_EMPTY_STAGE_TIMEOUT` measured from it.
//! 2. A PARTICIPANT IN THE STAGE OWES NOTHING. A composition whose stage still holds one
//!    goes on composing, and a participant joining before the timeout has run out takes
//!    the deadline away again.
//! 3. WHEN THE DEADLINE RUNS OUT the composition ends exactly as a caller's
//!    `StopComposition` ends it: `STOPPING` with an end time and every destination with
//!    it, `STOPPED` one `state::COMPOSITION_STOP_SPAN` later, and the ephemeral resource
//!    deleted one `state::COMPOSITION_RETENTION_SPAN` after that — the model says the
//!    "Composition stops and the resource is deleted", so the read that follows is the
//!    modelled 404.
//! 4. WHAT THAT RELEASES. While the composition is composing, the encoder configuration its
//!    destination names cannot be deleted (the modelled `ConflictException`, HTTP 409);
//!    once the clock has reclaimed the composition the delete succeeds and the composition
//!    is gone from `ListCompositions`.
//!
//! The timeout AWS publishes is a MINUTE, which no test waits out: the cases that observe
//! what happens when it runs out move the stored deadline — the one value the service's
//! materialisation compares the clock against — up to the present, after asserting the
//! service recorded it, and then watch the service walk every step itself.

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
/// crate so this suite cannot drift from the span the service enforces.
const STOP_DUE_AFTER: Duration = state::COMPOSITION_EMPTY_STAGE_STOP_SPAN;

/// The timeout the model publishes for a composition whose stage holds nobody: the whole
/// end — [`STOP_DUE_AFTER`], the stop and the retention of the stopped record — fits inside
/// it, which is what [`STOP_DUE_AFTER`] is derived from.
const TIMEOUT: Duration = state::COMPOSITION_EMPTY_STAGE_TIMEOUT;

/// The span the service gives the `STOPPING` -> `STOPPED` step of a stop, read from the
/// crate for the same reason.
const STOP_SPAN: Duration = state::COMPOSITION_STOP_SPAN;

/// How long the service goes on reporting a stopped composition before the ephemeral
/// resource is deleted, read from the crate for the same reason.
const RETENTION: Duration = state::COMPOSITION_RETENTION_SPAN;

/// Added to every sleep, so a span this suite waits out has certainly elapsed whatever
/// scheduling delay the test host adds.
const SLACK: Duration = Duration::from_millis(400);

// The `CompositionState` and `DestinationState` values this suite asserts, from
// `smithy_query(query="shape",shape="CompositionState,DestinationState")`:
//   CompositionState  STARTING | ACTIVE | STOPPING | FAILED | STOPPED
//   DestinationState  STARTING | ACTIVE | STOPPING | RECONNECTING | FAILED | STOPPED
const STATE_STARTING: &str = "STARTING";
const STATE_ACTIVE: &str = "ACTIVE";
const STATE_STOPPING: &str = "STOPPING";
const STATE_STOPPED: &str = "STOPPED";

/// The states a composition the clock stopped passes through, in order: `STOPPING` until
/// one [`STOP_SPAN`] has elapsed, `STOPPED` for the whole [`RETENTION`] after that, and then
/// no state at all — the resource is deleted, so the read is a 404.
const STOP_PHASES: [Option<&str>; 3] = [Some(STATE_STOPPING), Some(STATE_STOPPED), None];

/// Which of [`STOP_PHASES`] a composition whose stop began `elapsed` ago is in.
fn stop_phase(elapsed: Duration) -> usize {
    if elapsed < STOP_SPAN {
        0
    } else if elapsed < STOP_SPAN + RETENTION {
        1
    } else {
        2
    }
}

/// Assert a read issued between `before` and `after` of the instant the stop began found
/// the composition in one of the states that window admits — one state where the read
/// cannot have straddled a span boundary, and either side of it where it can.
fn assert_stop_phase(case: &str, what: &str, window: (Duration, Duration), read: (u16, &Value)) {
    let (before, after) = window;
    let (status, response) = read;
    let observed: Option<String> = (status == 200)
        .then(|| member(&response["composition"], "state"))
        .filter(|state| !state.is_empty());
    let admissible = &STOP_PHASES[stop_phase(before)..=stop_phase(after)];
    assert!(
        admissible
            .iter()
            .any(|state| state.map(str::to_string) == observed),
        "case {case:?}: {what}, read between {before:?} and {after:?} after the timeout ran \
         out, must be one of {admissible:?} (and reported exactly when that state is Some), \
         got HTTP {status} {response}"
    );
}

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
    async fn create_stage(&self, case: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage must answer HTTP 200, got {status} {response}"
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
    /// the id the holder of that token joins the stage under.
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

    /// Create an ingest configuration for `stage_arn` and answer what the service reported.
    async fn create_ingest_configuration(&self, case: &str, stage_arn: &str) -> Value {
        let request = json!({
            "ingestProtocol": "RTMP",
            "insecureIngest": true,
            "stageArn": stage_arn,
        });
        let (status, response) = self.call("CreateIngestConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does.
    fn join(
        &self,
        case: &str,
        stage_arn: &str,
        participant_id: &str,
        ingest_configuration_arn: Option<&str>,
    ) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let joined = state::JoinedParticipant {
            participant_id: participant_id.to_string(),
            published: ingest_configuration_arn.is_some(),
            ingest_configuration_arn: ingest_configuration_arn.map(str::to_string),
            ..Default::default()
        };
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// `DisconnectParticipant`, asserting the modelled success, and answering the window
    /// the call was made in.
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

    /// `StartComposition` against `stage_arn`, with one destination naming `encoder`.
    /// Answers the composition's ARN beside the window the call was made in.
    async fn start(
        &self,
        case: &str,
        stage_arn: &str,
        encoder: &str,
    ) -> (String, (SystemTime, SystemTime)) {
        let request = json!({
            "stageArn": stage_arn,
            "destinations": [{
                "channel": {
                    "channelArn": arn("channel", "live"),
                    "encoderConfigurationArn": encoder,
                },
            }],
        });
        let before = SystemTime::now();
        let (status, response) = self.call("StartComposition", &request).await;
        let after = SystemTime::now();
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
        (member(&composition, "arn"), (before, after))
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
    /// Refuses to invent one: the deadline has to be there already, which is rule 1.
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
    /// Refuses to invent one: the composition's end has to have begun already, by a caller's
    /// stop or by the deadline the clock ran out.
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

/// The destinations of a composition, as the wire reports them.
fn destinations(composition: &Value) -> Vec<Value> {
    composition["destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One way the stage a composition composites comes to hold no participant at all. The rows
/// differ in WHICH call empties the stage and in how many participants it had to take out,
/// which is what the rule generalises over: the deadline is measured from the instant the
/// stage was left empty, whichever call left it that way.
#[derive(Clone, Copy, Debug)]
enum Emptied {
    /// No participant ever joined, so the start itself finds the stage empty — the model's
    /// "if there are no participants in the stage when StartComposition is called".
    NobodyEverJoined,
    /// The stage had a participant, who left before the composition started.
    AlreadyEmptyAtTheStart,
    /// `n` participants joined and were disconnected one by one while the composition was
    /// composing — the model's "when all participants are disconnected from the stage".
    DisconnectedWhileComposing(usize),
    /// The stage's only participant was a publisher, removed together with the ingest
    /// configuration it was publishing with (`force: true`).
    PublisherRemovedWithItsIngestConfiguration,
}

/// The rows rule 1 and rule 3 are exercised over.
const EMPTIED_ROWS: [Emptied; 5] = [
    Emptied::NobodyEverJoined,
    Emptied::AlreadyEmptyAtTheStart,
    Emptied::DisconnectedWhileComposing(1),
    Emptied::DisconnectedWhileComposing(3),
    Emptied::PublisherRemovedWithItsIngestConfiguration,
];

impl Emptied {
    /// This row's name, as every assertion of this suite reports it.
    fn case(self) -> String {
        format!("{self:?}")
    }

    /// Stand a composition of one stage up as this row describes, and answer its ARN beside
    /// the window of the call that left its stage empty — the instant the published timeout
    /// is measured from.
    async fn arrange(
        self,
        emulator: &RealTime,
        encoder: &str,
    ) -> (String, (SystemTime, SystemTime)) {
        let case = self.case();
        let case = case.as_str();
        let stage = emulator.create_stage(case).await;
        match self {
            Self::NobodyEverJoined => emulator.start(case, &stage, encoder).await,
            Self::AlreadyEmptyAtTheStart => {
                let participant = emulator.create_token(case, &stage).await;
                emulator.join(case, &stage, &participant, None);
                emulator.disconnect(case, &stage, &participant).await;
                emulator.start(case, &stage, encoder).await
            }
            Self::DisconnectedWhileComposing(joining) => {
                let mut participants = Vec::new();
                for _ in 0..joining {
                    let participant = emulator.create_token(case, &stage).await;
                    emulator.join(case, &stage, &participant, None);
                    participants.push(participant);
                }
                let (composition, _) = emulator.start(case, &stage, encoder).await;
                let mut emptied = (SystemTime::now(), SystemTime::now());
                for (index, participant) in participants.iter().enumerate() {
                    emptied = emulator.disconnect(case, &stage, participant).await;
                    // Until the LAST one goes the stage still holds somebody, so nothing is
                    // owed yet: the timeout runs from the departure that empties the stage.
                    if index + 1 < participants.len() {
                        let deadline = emulator.deadline(case, &composition);
                        assert!(
                            deadline.is_none(),
                            "case {case:?}: with {} of {} participants still in the stage, \
                             the composition must be owed no stop, got {deadline:?}",
                            participants.len() - index - 1,
                            participants.len()
                        );
                    }
                }
                (composition, emptied)
            }
            Self::PublisherRemovedWithItsIngestConfiguration => {
                let configuration = emulator.create_ingest_configuration(case, &stage).await;
                let configuration_arn = member(&configuration, "arn");
                let publisher = member(&configuration, "participantId");
                emulator.join(case, &stage, &publisher, Some(&configuration_arn));
                let (composition, _) = emulator.start(case, &stage, encoder).await;
                let before = SystemTime::now();
                let (status, response) = emulator
                    .call(
                        "DeleteIngestConfiguration",
                        &json!({ "arn": configuration_arn, "force": true }),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: DeleteIngestConfiguration {configuration_arn} with force \
                     must answer HTTP 200, got {status} {response}"
                );
                (composition, (before, SystemTime::now()))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 1 — a stage left without participants puts its compositions on a deadline
// ---------------------------------------------------------------------------

/// However the stage a composition composites comes to hold nobody, the composition is owed
/// its stop one published timeout after THAT instant.
#[tokio::test]
async fn a_stage_left_without_participants_owes_its_compositions_a_stop_one_timeout_later() {
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
             left empty (between {:?} and {:?}), so that the whole end is over one {TIMEOUT:?} \
             after it, got {deadline:?}",
            before + STOP_DUE_AFTER,
            after + STOP_DUE_AFTER
        );

        // Nothing is owed YET: the composition is still composing, with no end time, and
        // the configuration its destination names is still held back.
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
    }
}

// ---------------------------------------------------------------------------
// Rule 2 — a participant in the stage owes nothing
// ---------------------------------------------------------------------------

/// A composition whose stage holds somebody is owed no stop, and a join inside the timeout
/// takes a deadline away again.
#[tokio::test]
async fn a_composition_whose_stage_holds_a_participant_is_owed_no_stop() {
    // The rows differ in how the stage comes to hold a participant while the composition
    // composites it: one that was there all along, one of several that stayed behind, and
    // one that arrived after the start had already put the composition on a deadline.
    let rows = [
        "a participant that joined before the start and stayed",
        "one of three participants left behind by the departure of the other two",
        "a participant that joined after the start had found the stage empty",
    ];

    for case in rows {
        let emulator = RealTime::new();
        let encoder = emulator.create_encoder_configuration(case).await;
        let stage = emulator.create_stage(case).await;

        let joining = if case.contains("three") { 3 } else { 1 };
        let joined_before_the_start = !case.contains("after the start");
        let mut participants = Vec::new();
        if joined_before_the_start {
            for _ in 0..joining {
                let participant = emulator.create_token(case, &stage).await;
                emulator.join(case, &stage, &participant, None);
                participants.push(participant);
            }
        }

        let (composition, _) = emulator.start(case, &stage, &encoder).await;

        if joined_before_the_start {
            // Everyone but the last of them leaves, so the stage never empties.
            for participant in participants.iter().take(joining - 1) {
                emulator.disconnect(case, &stage, participant).await;
            }
        } else {
            // The start found the stage empty, so the composition IS on a deadline here —
            // which the join must take away.
            assert!(
                emulator.deadline(case, &composition).is_some(),
                "case {case:?}: a start against an empty stage must record a deadline"
            );
            let participant = emulator.create_token(case, &stage).await;
            emulator.join(case, &stage, &participant, None);
        }

        let deadline = emulator.deadline(case, &composition);
        assert!(
            deadline.is_none(),
            "case {case:?}: a composition whose stage holds a participant must be owed no \
             stop, got {deadline:?}"
        );

        // So it goes on composing, and the encoder configuration its destination names
        // stays undeletable: the model tells a caller to stop the composition first.
        tokio::time::sleep(state::COMPOSITION_ACTIVATION_SPAN + SLACK).await;
        let (status, response) = emulator.read(&composition).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the composition must still be reported, got {status} {response}"
        );
        assert_eq!(
            member(&response["composition"], "state"),
            STATE_ACTIVE,
            "case {case:?}: the composition must still be composing, got {response}"
        );
        let (status, response) = emulator
            .call("DeleteEncoderConfiguration", &json!({ "arn": encoder }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: an encoder configuration a composing composition names cannot be \
             deleted, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected the modelled conflict code, got {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rules 3 and 4 — when the timeout runs out
// ---------------------------------------------------------------------------

/// Once the published timeout has run out, the composition walks the same steps a caller's
/// stop walks it through, the ephemeral resource is deleted, and what it was holding back is
/// released.
#[tokio::test]
async fn a_composition_of_an_empty_stage_stops_and_is_deleted_when_the_timeout_runs_out() {
    for row in EMPTIED_ROWS {
        let case = row.case();
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.create_encoder_configuration(case).await;
        let (composition, _) = row.arrange(&emulator, &encoder).await;

        // Before the timeout runs out the encoder configuration cannot be deleted.
        let (status, response) = emulator
            .call("DeleteEncoderConfiguration", &json!({ "arn": encoder }))
            .await;
        assert_eq!(
            status, 409,
            "case {case:?}: while the composition composes, deleting {encoder} must be the \
             modelled conflict, got {status} {response}"
        );

        let ran_out = emulator.run_out_the_timeout(case, &composition);

        // Straight after the timeout: stopping, with an end time, and every destination
        // with it.
        let before = ran_out.elapsed();
        let (status, response) = emulator.read(&composition).await;
        let window = (before, ran_out.elapsed());
        assert_stop_phase(
            case,
            "the composition whose stage stayed empty",
            window,
            (status, &response),
        );
        if status == 200 {
            let stopping = response["composition"].clone();
            assert!(
                stopping.get("endTime").is_some(),
                "case {case:?}: a composition the clock stopped must report when its \
                 broadcast ended, got {stopping}"
            );
            for (index, destination) in destinations(&stopping).iter().enumerate() {
                let state = member(destination, "state");
                assert!(
                    [STATE_STOPPING, STATE_STOPPED].contains(&state.as_str()),
                    "case {case:?}: destination {index} must follow the composition it \
                     belongs to, got {destination}"
                );
                assert!(
                    destination.get("endTime").is_some(),
                    "case {case:?}: destination {index} must report when its broadcast \
                     ended, got {destination}"
                );
            }
        }

        // One span later the stop has finished.
        tokio::time::sleep(STOP_SPAN + SLACK).await;
        let before = ran_out.elapsed();
        let (status, response) = emulator.read(&composition).await;
        let window = (before, ran_out.elapsed());
        assert_stop_phase(
            case,
            "the composition one span after the timeout",
            window,
            (status, &response),
        );

        // Once the retention span is over the ephemeral resource is gone, so the read is the
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

        // And what it was holding back is released: the encoder configuration its
        // destination named can now be deleted.
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

/// A participant arriving after the timeout has already run out does not take the stop
/// back: what the clock owed at the instant it fell due is what every later caller sees.
#[tokio::test]
async fn a_join_after_the_timeout_ran_out_does_not_revive_the_composition() {
    let case = "a participant joining after the timeout ran out";
    let emulator = RealTime::new();
    let encoder = emulator.create_encoder_configuration(case).await;
    let stage = emulator.create_stage(case).await;
    let (composition, _) = emulator.start(case, &stage, &encoder).await;

    let ran_out = emulator.run_out_the_timeout(case, &composition);
    let participant = emulator.create_token(case, &stage).await;
    emulator.join(case, &stage, &participant, None);

    let before = ran_out.elapsed();
    let (status, response) = emulator.read(&composition).await;
    let window = (before, ran_out.elapsed());
    assert_stop_phase(
        case,
        "the composition a late join reached",
        window,
        (status, &response),
    );

    // And it goes the rest of the way: the ephemeral resource is deleted all the same.
    tokio::time::sleep(STOP_SPAN + SLACK).await;
    emulator.run_out_the_retention(case, &composition);
    let (status, response) = emulator.read(&composition).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the composition the timeout stopped must still be deleted, \
         got {status} {response}"
    );
}
