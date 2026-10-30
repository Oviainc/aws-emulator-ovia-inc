// SPDX-License-Identifier: CC0-1.0

//! How long a composition's TERMINAL state is reported before the ephemeral resource goes.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body — never
//! an internal call. The one thing written straight onto the store is the AGE of an end the
//! service itself recorded, which is how the sibling composition suites observe a span
//! without waiting it out.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A STOPPED COMPOSITION IS REPORTED UNTIL ITS RECORD EXPIRES. However its end began —
//!    a caller's `StopComposition` or the clock running out the deadline its empty stage
//!    earned — the composition answers HTTP 200 with state `STOPPED` and the `endTime` of
//!    that end for every read made inside one `state::COMPOSITION_RETENTION_SPAN` of the
//!    moment the stop finished, so a client that reads it back at a cadence of seconds
//!    observes the terminal state across several reads. Only past that span is the
//!    ephemeral resource deleted, and the read then the modelled
//!    `ResourceNotFoundException` (HTTP 404).
//! 2. EVERY READER AGREES. `GetComposition` and `ListCompositions` report the same state
//!    and the same end time while the record is retained, and neither reports the
//!    composition once the clock has reclaimed it.
//! 3. A CLOCK-DUE END FITS INSIDE THE PUBLISHED MINUTE. A composition whose stage holds
//!    nobody is owed its stop one `state::COMPOSITION_EMPTY_STAGE_STOP_SPAN` after the
//!    stage fell empty, which leaves the whole end — the stop and the retention of the
//!    stopped record — inside the published `state::COMPOSITION_EMPTY_STAGE_TIMEOUT`: the
//!    composition is `STOPPED` well before that minute is up and its record expires when
//!    the minute runs out.
//!
//! The ages the reads are made at span the retention of a stopped record: the instant the
//! end begins, the instant the stop finishes, the middle of the retention, its last moment,
//! and two ages past it.

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

/// The span the service gives the `STOPPING` -> `STOPPED` step of a stop, read from the
/// crate so this suite cannot drift from the span the service enforces.
const STOP_SPAN: Duration = state::COMPOSITION_STOP_SPAN;

/// How long a stopped composition goes on being reported before the ephemeral resource is
/// deleted, read from the crate for the same reason.
const RETENTION: Duration = state::COMPOSITION_RETENTION_SPAN;

/// How long after its stage was left empty a composition is owed its stop, read from the
/// crate for the same reason.
const STOP_DUE_AFTER: Duration = state::COMPOSITION_EMPTY_STAGE_STOP_SPAN;

/// The timeout the model publishes for a composition whose stage holds nobody, read from
/// the crate for the same reason.
const TIMEOUT: Duration = state::COMPOSITION_EMPTY_STAGE_TIMEOUT;

/// The margin every age in this suite keeps from a span boundary, so a read made a few
/// milliseconds after the age was written still lies on the side the case means.
const SLACK: Duration = Duration::from_millis(400);

/// The interval an ordinary client polls a composition it is waiting on at — the cadence
/// rule 1's "several reads" is counted over. Not a span the service enforces: it stands for
/// the caller.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

// The `CompositionState` values this suite asserts, from
// `smithy_query(query="shape",shape="CompositionState")`:
//   STARTING | ACTIVE | STOPPING | FAILED | STOPPED
const STATE_STOPPING: &str = "STOPPING";
const STATE_STOPPED: &str = "STOPPED";

/// The modelled error a read of a composition the clock has reclaimed is answered with.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it,
/// plus the engine handle the age of a recorded end is written through.
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

    /// Issue a participant token for `stage_arn` and answer the participant id it carries.
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

    /// Record a participant joining `stage_arn`, the way the stage's data plane does — the
    /// one seam of this suite that is not an operation, as in the sibling stage suites.
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

    /// `DisconnectParticipant`, asserting the modelled success.
    async fn disconnect(&self, case: &str, stage_arn: &str, participant_id: &str) {
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
    }

    /// `StartComposition` of `stage_arn` recording to `encoder`, asserting the modelled
    /// success, and answering the composition ARN.
    async fn start(&self, case: &str, stage_arn: &str, encoder: &str) -> String {
        let body = json!({
            "stageArn": stage_arn,
            "destinations": [{ "channel": {
                "channelArn": arn("channel", "live-channel"),
                "encoderConfigurationArn": encoder,
            } }],
        });
        let (status, response) = self.call("StartComposition", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartComposition {body} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["composition"], "arn")
    }

    /// `StopComposition`, asserting the modelled empty success.
    async fn stop(&self, case: &str, composition_arn: &str) {
        let (status, response) = self
            .call("StopComposition", &json!({ "arn": composition_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopComposition {composition_arn} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// `GetComposition` without asserting the answer: this suite reads a composition both
    /// while its record is retained and once it is gone.
    async fn read(&self, composition_arn: &str) -> (u16, Value) {
        self.call("GetComposition", &json!({ "arn": composition_arn }))
            .await
    }

    /// The summary `ListCompositions` reports for `composition_arn`, or `None` when the page
    /// does not carry it.
    async fn listed(&self, case: &str, composition_arn: &str) -> Option<Value> {
        let (status, response) = self.call("ListCompositions", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCompositions must answer HTTP 200, got {status} {response}"
        );
        response["compositions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|summary| member(summary, "arn") == composition_arn)
    }

    /// The deadline the service recorded for `composition_arn`: the instant the clock owes
    /// it the stop its empty stage earned, or `None` while somebody is in that stage.
    fn deadline(&self, case: &str, composition_arn: &str) -> Option<SystemTime> {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let store = handle.lock();
        store
            .compositions
            .get(composition_arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {composition_arn}"))
            .empty_stage_stop_at
    }

    /// Age the end the service recorded for `composition_arn` to `age`, so the service is
    /// read as a caller reading `age` after that end would read it.
    ///
    /// Only the AGE is written: which instants the record carries, and what the service
    /// makes of them, are the service's own. The end has to be there already — a stop the
    /// caller made, or the deadline the start recorded — so nothing here invents one.
    fn age_the_end(&self, case: &str, composition_arn: &str, age: Duration) {
        let handle = state::service_state(&*self.engine).expect("this service's store");
        let mut store = handle.lock();
        let composition = store
            .compositions
            .get_mut(composition_arn)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold {composition_arn}"));
        let began = SystemTime::now() - age;
        assert!(
            composition.ended_at.is_some() || composition.empty_stage_stop_at.is_some(),
            "case {case:?}: {composition_arn} must already carry the end to age, \
             got ended_at {:?} and deadline {:?}",
            composition.ended_at,
            composition.empty_stage_stop_at
        );
        composition.ended_at = composition.ended_at.map(|_| began);
        composition.empty_stage_stop_at = composition.empty_stage_stop_at.map(|_| began);
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

/// How a composition's end begins. The rows differ in WHICH of the two ways the model states
/// ended it, and — for the clock — in what left the stage empty, which is what the rule
/// generalises over: the record of a finished stop is kept for the same span either way.
#[derive(Clone, Copy, Debug)]
enum EndedBy {
    /// A caller's `StopComposition`.
    ACaller,
    /// The clock running out the deadline a stage that never held anybody earned.
    TheClockOverAStageNobodyJoined,
    /// The clock running out the deadline the departure of the stage's last participant
    /// earned.
    TheClockAfterTheLastParticipantLeft,
}

/// The rows every rule of this suite is exercised over.
const ENDED_ROWS: [EndedBy; 3] = [
    EndedBy::ACaller,
    EndedBy::TheClockOverAStageNobodyJoined,
    EndedBy::TheClockAfterTheLastParticipantLeft,
];

impl EndedBy {
    /// This row's name, as every assertion of this suite reports it.
    fn case(self) -> String {
        format!("{self:?}")
    }

    /// Stand a composition up and bring its end about as this row describes, answering its
    /// ARN. Whatever ended it, the composition's end has begun when this returns.
    async fn arrange(self, emulator: &RealTime, encoder: &str) -> String {
        let case = self.case();
        let case = case.as_str();
        let stage = emulator.create_stage(case).await;
        match self {
            Self::ACaller => {
                let composition = emulator.start(case, &stage, encoder).await;
                emulator.stop(case, &composition).await;
                composition
            }
            Self::TheClockOverAStageNobodyJoined => {
                let composition = emulator.start(case, &stage, encoder).await;
                assert!(
                    emulator.deadline(case, &composition).is_some(),
                    "case {case:?}: a start against a stage nobody is in must record the \
                     deadline the clock owes its stop at"
                );
                composition
            }
            Self::TheClockAfterTheLastParticipantLeft => {
                let participant = emulator.create_token(case, &stage).await;
                emulator.join(case, &stage, &participant);
                let composition = emulator.start(case, &stage, encoder).await;
                assert!(
                    emulator.deadline(case, &composition).is_none(),
                    "case {case:?}: a composition whose stage holds somebody is owed no stop"
                );
                emulator.disconnect(case, &stage, &participant).await;
                assert!(
                    emulator.deadline(case, &composition).is_some(),
                    "case {case:?}: the departure that emptied the stage must record the \
                     deadline the clock owes the stop at"
                );
                composition
            }
        }
    }
}

/// What a read of the composition must answer at one age of its end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// Reported, in this `CompositionState`.
    Reported(&'static str),
    /// Gone: the ephemeral resource has been deleted, so the read is the modelled 404.
    Reclaimed,
}

/// One age of an end, and what every reader must answer at it.
///
/// The ages span the retention of a stopped record: the instant the end begins, the instant
/// the stop finishes, the middle of the retention, its last moment, the moment it is over
/// and one well past it.
fn age_rows() -> Vec<(String, Duration, Expected)> {
    vec![
        (
            "the instant the end began".to_string(),
            Duration::ZERO,
            Expected::Reported(STATE_STOPPING),
        ),
        (
            "halfway through the stop".to_string(),
            STOP_SPAN / 2,
            Expected::Reported(STATE_STOPPING),
        ),
        (
            "the instant the stop finished".to_string(),
            STOP_SPAN + SLACK,
            Expected::Reported(STATE_STOPPED),
        ),
        (
            "the middle of the retention of the stopped record".to_string(),
            STOP_SPAN + RETENTION / 2,
            Expected::Reported(STATE_STOPPED),
        ),
        (
            "the last moment of the retention of the stopped record".to_string(),
            STOP_SPAN + RETENTION - SLACK,
            Expected::Reported(STATE_STOPPED),
        ),
        (
            "the moment the retention of the stopped record is over".to_string(),
            STOP_SPAN + RETENTION + SLACK,
            Expected::Reclaimed,
        ),
        (
            "well past the retention of the stopped record".to_string(),
            STOP_SPAN + RETENTION * 3,
            Expected::Reclaimed,
        ),
    ]
}

/// RULE 1 and RULE 2: at every age inside the retention of a stopped record both readers
/// report the composition — `STOPPED` with its end time once the stop has finished — and
/// past that span neither reports it at all.
#[tokio::test]
async fn a_stopped_composition_is_reported_until_its_record_expires() {
    for row in ENDED_ROWS {
        let case = row.case();
        for (age_case, age, expected) in age_rows() {
            let case = format!("{case}, read at {age_case}");
            let case = case.as_str();
            // One emulator per case, so each decides against a store holding only its own
            // composition.
            let emulator = RealTime::new();
            let encoder = emulator.create_encoder_configuration(case).await;
            let composition = row.arrange(&emulator, &encoder).await;

            emulator.age_the_end(case, &composition, age);
            let (status, response) = emulator.read(&composition).await;
            let listed = emulator.listed(case, &composition).await;

            match expected {
                Expected::Reported(state) => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: an end {age:?} old is inside the retention of the \
                         stopped record, so GetComposition must answer HTTP 200, \
                         got {status} {response}"
                    );
                    let read = response["composition"].clone();
                    assert_eq!(
                        member(&read, "state"),
                        state,
                        "case {case:?}: an end {age:?} old must be reported as {state}, \
                         got {read}"
                    );
                    let end_time = member(&read, "endTime");
                    assert!(
                        !end_time.is_empty(),
                        "case {case:?}: a composition whose end has begun must report when \
                         its broadcast ended, got {read}"
                    );
                    let summary = listed.unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: a composition GetComposition reports must be on \
                             the ListCompositions page too"
                        )
                    });
                    assert_eq!(
                        (member(&summary, "state"), member(&summary, "endTime")),
                        (state.to_string(), end_time.clone()),
                        "case {case:?}: the listing must report the same state and end time \
                         as the read ({state}, {end_time}), got {summary}"
                    );
                    // Reading it again changes nothing: the end time a reader is given is
                    // the instant the end began, not the instant it was asked.
                    let (again_status, again) = emulator.read(&composition).await;
                    assert_eq!(
                        (again_status, member(&again["composition"], "endTime")),
                        (200, end_time.clone()),
                        "case {case:?}: a second read must answer with the same end time \
                         {end_time}, got {again_status} {again}"
                    );
                }
                Expected::Reclaimed => {
                    assert_eq!(
                        status, 404,
                        "case {case:?}: an end {age:?} old is past the retention of the \
                         stopped record, so the ephemeral resource is deleted and the read \
                         must answer HTTP 404, got {status} {response}"
                    );
                    assert_eq!(
                        error_code(&response),
                        NOT_FOUND,
                        "case {case:?}: the refusal must be the modelled {NOT_FOUND}, \
                         got {response}"
                    );
                    assert_eq!(
                        member(&response, "message"),
                        format!("Resource: {composition} not found"),
                        "case {case:?}: the refusal must name the composition the request \
                         asked for, got {response}"
                    );
                    assert!(
                        listed.is_none(),
                        "case {case:?}: a reclaimed composition must be off the \
                         ListCompositions page, got {listed:?}"
                    );
                }
            }
        }
    }
}

/// RULE 1, as the client that waits for a stop sees it: a caller reading the composition
/// back every [`POLL_INTERVAL`] observes the terminal state on more than one read, whichever
/// way the end began.
#[tokio::test]
async fn a_client_polling_at_a_cadence_of_seconds_observes_the_terminal_state() {
    for row in ENDED_ROWS {
        let case = row.case();
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.create_encoder_configuration(case).await;
        let composition = row.arrange(&emulator, &encoder).await;

        // The polls a client makes: one straight after the end began, then one every
        // interval for as long as the retention of a stopped record can last.
        let mut age = Duration::ZERO;
        let mut terminal_reads = 0_u32;
        let mut states = Vec::new();
        while age <= STOP_SPAN + RETENTION {
            emulator.age_the_end(case, &composition, age);
            let (status, response) = emulator.read(&composition).await;
            let state = if status == 200 {
                member(&response["composition"], "state")
            } else {
                format!("HTTP {status} {}", error_code(&response))
            };
            if state == STATE_STOPPED {
                terminal_reads += 1;
                assert!(
                    !member(&response["composition"], "endTime").is_empty(),
                    "case {case:?}: the terminal read at {age:?} must carry the end time, \
                     got {response}"
                );
            }
            states.push((age, state));
            age += POLL_INTERVAL;
        }

        assert!(
            terminal_reads > 1,
            "case {case:?}: a client polling every {POLL_INTERVAL:?} must observe {STATE_STOPPED} \
             on more than one read, got {states:?}"
        );
    }
}

/// RULE 3: the deadline the clock records for a composition whose stage holds nobody leaves
/// the whole end — the stop and the retention of the stopped record — inside the published
/// timeout, so the composition is reported `STOPPED` before that minute is up and its record
/// expires when it runs out.
#[tokio::test]
async fn a_clock_due_end_fits_inside_the_published_timeout() {
    for row in [
        EndedBy::TheClockOverAStageNobodyJoined,
        EndedBy::TheClockAfterTheLastParticipantLeft,
    ] {
        let case = row.case();
        let case = case.as_str();
        let emulator = RealTime::new();
        let encoder = emulator.create_encoder_configuration(case).await;

        let emptied = SystemTime::now();
        let composition = row.arrange(&emulator, &encoder).await;
        let arranged = SystemTime::now();

        let deadline = emulator
            .deadline(case, &composition)
            .unwrap_or_else(|| panic!("case {case:?}: the stop must be owed at a deadline"));
        assert!(
            deadline >= emptied + STOP_DUE_AFTER && deadline <= arranged + STOP_DUE_AFTER,
            "case {case:?}: the stop must be owed one {STOP_DUE_AFTER:?} after the stage was \
             left empty (between {:?} and {:?}), got {deadline:?}",
            emptied + STOP_DUE_AFTER,
            arranged + STOP_DUE_AFTER
        );
        // Which is what makes the whole end fit: the stop finishes, and the record is kept
        // for its retention, inside the minute the model publishes.
        assert!(
            deadline + STOP_SPAN + RETENTION <= arranged + TIMEOUT,
            "case {case:?}: the stop ({STOP_SPAN:?}) and the retention of the stopped record \
             ({RETENTION:?}) after the deadline {deadline:?} must still be inside the \
             published {TIMEOUT:?}, which runs out by {:?}",
            arranged + TIMEOUT
        );
        assert!(
            deadline + STOP_SPAN < arranged + TIMEOUT,
            "case {case:?}: the composition must be {STATE_STOPPED} before the published \
             {TIMEOUT:?} is up, got a stop finishing at {:?}",
            deadline + STOP_SPAN
        );

        // And that is what a caller sees: at an age one published timeout old the resource
        // is gone, and at every age the stop has finished inside it the state is terminal.
        for (age_case, age, expected) in [
            (
                "one stop span into the timeout",
                STOP_SPAN + SLACK,
                Expected::Reported(STATE_STOPPED),
            ),
            (
                "the last moment of the timeout",
                TIMEOUT - STOP_DUE_AFTER - SLACK,
                Expected::Reported(STATE_STOPPED),
            ),
            (
                "one whole published timeout after the stage fell empty",
                TIMEOUT - STOP_DUE_AFTER + SLACK,
                Expected::Reclaimed,
            ),
        ] {
            emulator.age_the_end(case, &composition, age);
            let (status, response) = emulator.read(&composition).await;
            match expected {
                Expected::Reported(state) => {
                    assert_eq!(
                        (status, member(&response["composition"], "state")),
                        (200, state.to_string()),
                        "case {case:?} at {age_case:?}: the composition must be reported as \
                         {state}, got {status} {response}"
                    );
                }
                Expected::Reclaimed => {
                    assert_eq!(
                        (status, error_code(&response)),
                        (404, NOT_FOUND),
                        "case {case:?} at {age_case:?}: the ephemeral resource must be gone, \
                         got {status} {response}"
                    );
                }
            }
        }
    }
}
