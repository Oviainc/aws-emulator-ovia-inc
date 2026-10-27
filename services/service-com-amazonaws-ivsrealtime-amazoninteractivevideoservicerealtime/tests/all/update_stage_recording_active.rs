// SPDX-License-Identifier: CC0-1.0

//! `UpdateStage` against a stage whose individual participant recording is running, over
//! the wire.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs: a member the
//! model documents as not updatable while a recording is active is refused with the
//! operation's modelled `ConflictException` (HTTP 409), which persists nothing, for as
//! long as a recording is running on the resource the request names — and the update is
//! taken as soon as it is not. For `UpdateStage` that member is
//! `autoParticipantRecordingConfiguration` ("Note that this cannot be updated while
//! recording is active",
//! `smithy_query(query="doc",shape="UpdateStageRequest",member="autoParticipantRecordingConfiguration")`),
//! and a recording is running when a participant still in the stage carries a
//! `ParticipantRecordingState` that is neither "nothing records it" (`DISABLED`) nor a
//! recording that has come to rest (`STOPPED`, `FAILED`) AND the stage's configuration
//! names a storage configuration the recording can be written to ("Individual participant
//! recording cannot be started unless a storage configuration is specified",
//! `smithy_query(query="shape",shape="AutoParticipantRecordingConfiguration")`). A request
//! that does not supply that member is never refused over a recording, because it does
//! not update it.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request, exactly
//! as a signed `aws ivs-realtime update-stage` call arrives, so what is asserted is the
//! emulator's HTTP behaviour and never a handler called directly.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage or starts
//! its recording: a participant joins over the stage's data plane with the token it was
//! issued, which is `state::join_stage_session` — the one place a participant record is
//! created. So these cases bring recorded participants into a session through it, reaching
//! the store through this suite's own engine handle, exactly as the `GetParticipant` and
//! `UpdateIngestConfiguration` suites beside it do.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// Every value the modelled `ParticipantRecordingState` carries, from
/// `smithy_query(query="shape",shape="ParticipantRecordingState")`, each paired with
/// whether it is a recording that is RUNNING: `DISABLED` is nothing recording the
/// participant, `STOPPED` and `FAILED` are recordings that have come to rest, and the
/// other three are states media is still being written in. Every one of them is a row of
/// the table below.
const RECORDING_STATES: [(&str, bool); 6] = [
    ("STARTING", true),
    ("ACTIVE", true),
    ("STOPPING", true),
    ("STOPPED", false),
    ("FAILED", false),
    ("DISABLED", false),
];

/// The `storageConfigurationArn` of a stage that records nowhere: the empty string the
/// model documents as "no storage configuration is specified", and the value that "resets
/// other fields in this object to their defaults".
const NO_STORAGE_CONFIGURATION: &str = "";

/// One emulator holding only this service, driven through the router that serves
/// production traffic, plus the engine handle the data-plane seeding writes through.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00";
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("/{operation}"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a well-formed request");
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

    /// Create a storage configuration over `bucket` and return the ARN the service minted
    /// for it.
    async fn create_storage_configuration(&self, case: &str, bucket: &str) -> String {
        let request = json!({ "s3": { "bucketName": bucket }, "name": bucket });
        let (status, response) = self.call("CreateStorageConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["storageConfiguration"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Create a stage named `name` whose recording configuration points at
    /// `storage_configuration_arn`, and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str, storage_configuration_arn: &str) -> String {
        let request = json!({
            "name": name,
            "autoParticipantRecordingConfiguration": {
                "storageConfigurationArn": storage_configuration_arn,
            },
        });
        let (status, response) = self.call("CreateStage", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {request} must answer HTTP 200, got {status} {response}"
        );
        response["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Record a participant joining `stage_arn` with the recording state the recorder
    /// reports for it, the way the stage's data plane does.
    fn join(&self, case: &str, stage_arn: &str, participant_id: &str, recording_state: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(
                &mut store,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.to_string(),
                    published: true,
                    recording_state: Some(recording_state.to_string()),
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
            "case {case:?}: the departure of {participant_id:?} must be recorded"
        );
    }

    /// The stage `arn`, as `GetStage` reports it.
    async fn stage(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage on {arn} must answer HTTP 200, got {status} {response}"
        );
        response["stage"].clone()
    }

    /// The stage's `activeSessionId`, read back over the wire.
    async fn active_session_id(&self, case: &str, arn: &str) -> String {
        self.stage(case, arn).await["activeSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// `UpdateStage` with `request`'s members: the status and the body a client sees.
    async fn update(&self, arn: &str, mut request: Value) -> (u16, Value) {
        request["arn"] = json!(arn);
        self.call("UpdateStage", &request).await
    }

    /// The `storageConfigurationArn` the stage's recording configuration reports, which is
    /// what each case holds its expectation against.
    async fn recorded_to(&self, case: &str, arn: &str) -> String {
        self.stage(case, arn).await["autoParticipantRecordingConfiguration"]
            ["storageConfigurationArn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The `message` of an error body.
fn error_message(response: &Value) -> &str {
    response["message"].as_str().unwrap_or("<no message>")
}

/// How a case asks for the recording configuration to be changed.
#[derive(Debug, Clone, Copy)]
enum Requested {
    /// Point the stage at another storage configuration.
    OtherStorage,
    /// Reset the whole object with the documented empty string.
    Reset,
}

/// The configuration member a request supplies, and what a taken update leaves the stage
/// recording to.
impl Requested {
    fn body(&self, other_storage: &str) -> Value {
        let arn = match self {
            Self::OtherStorage => other_storage,
            Self::Reset => NO_STORAGE_CONFIGURATION,
        };
        json!({ "autoParticipantRecordingConfiguration": { "storageConfigurationArn": arn } })
    }

    fn stored(&self, other_storage: &str) -> String {
        match self {
            Self::OtherStorage => other_storage.to_string(),
            Self::Reset => NO_STORAGE_CONFIGURATION.to_string(),
        }
    }
}

/// Whether the recording configuration can be updated is decided by the recording state
/// of the stage's own participants: while one of them is in a running recording the update
/// is the modelled `ConflictException` (HTTP 409) and writes nothing, and in every other
/// state it is taken and round-trips through `GetStage`.
///
/// Rows span every value the modelled `ParticipantRecordingState` carries, and both forms
/// a request can ask for the member in.
#[tokio::test]
async fn a_running_recording_refuses_an_update_of_the_recording_configuration() {
    for (recording_state, running) in RECORDING_STATES {
        for requested in [Requested::OtherStorage, Requested::Reset] {
            let case = format!("recordingState {recording_state}, update to {requested:?}");
            let emulator = Emulator::new();
            let storage = emulator.create_storage_configuration(&case, "recorded-here").await;
            let other_storage = emulator
                .create_storage_configuration(&case, "recorded-there")
                .await;
            let stage = emulator.create_stage(&case, "recorded", &storage).await;
            let participant = "publisher-1";
            emulator.join(&case, &stage, participant, recording_state);

            let (status, response) = emulator.update(&stage, requested.body(&other_storage)).await;
            let recorded_to = emulator.recorded_to(&case, &stage).await;
            if running {
                assert_eq!(
                    status, 409,
                    "case {case:?}: expected HTTP 409 while the recording runs, \
                     got {status} {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "ConflictException",
                    "case {case:?}: expected ConflictException, got {response}"
                );
                let expected = format!(
                    "Resource: {stage} is in use: the recording of participant {participant} \
                     is {recording_state}"
                );
                assert_eq!(
                    error_message(&response),
                    expected,
                    "case {case:?}: expected {expected:?}, got {response}"
                );
                assert_eq!(
                    recorded_to, storage,
                    "case {case:?}: a refused update must leave the stage recording to \
                     {storage:?}, got {recorded_to:?}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200 with no running recording, \
                     got {status} {response}"
                );
                let expected = requested.stored(&other_storage);
                let answered = response["stage"]["autoParticipantRecordingConfiguration"]
                    ["storageConfigurationArn"]
                    .as_str()
                    .unwrap_or_default();
                assert_eq!(
                    answered, expected,
                    "case {case:?}: expected the answer to record to {expected:?}, \
                     got {response}"
                );
                assert_eq!(
                    recorded_to, expected,
                    "case {case:?}: expected a read to record to {expected:?}, \
                     got {recorded_to:?}"
                );
            }
        }
    }
}

/// A request that does not supply the recording configuration is not refused over a
/// running recording: the member it cannot update is the one it does not send.
///
/// Rows differ in which other members the update supplies.
#[tokio::test]
async fn an_update_that_leaves_the_recording_configuration_alone_is_taken() {
    let cases: Vec<(&str, Value, &str)> = vec![
        ("name only", json!({ "name": "renamed" }), "renamed"),
        (
            "the empty name the model admits",
            json!({ "name": "" }),
            "",
        ),
        ("no member at all", json!({}), "recorded"),
    ];
    for (case, requested, expected_name) in cases {
        let emulator = Emulator::new();
        let storage = emulator.create_storage_configuration(case, "recorded-here").await;
        let stage = emulator.create_stage(case, "recorded", &storage).await;
        emulator.join(case, &stage, "publisher-1", "ACTIVE");

        let (status, response) = emulator.update(&stage, requested.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {requested} must answer HTTP 200 while a recording runs, \
             got {status} {response}"
        );
        let name = emulator.stage(case, &stage).await["name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            name, expected_name,
            "case {case:?}: expected the stage to be named {expected_name:?}, got {name:?}"
        );
        let recorded_to = emulator.recorded_to(case, &stage).await;
        assert_eq!(
            recorded_to, storage,
            "case {case:?}: expected the recording configuration left at {storage:?}, \
             got {recorded_to:?}"
        );
    }
}

/// What a case does to the stage before the update, so that no recording of ITS own is
/// running when the update arrives.
#[derive(Debug, Clone, Copy)]
enum NotRunning {
    /// The recorded participant has left the stage, so nothing is being recorded in it.
    ParticipantLeft,
    /// The stage names no storage configuration, so no recording of it can have started.
    NoStorageConfiguration,
    /// The running recording is a participant of ANOTHER stage.
    AnotherStage,
}

/// A recording that is not running in the stage the request names does not refuse the
/// update: a participant that has left, a stage that records nowhere, and a recording in
/// another stage each leave the update to be taken and to round-trip.
#[tokio::test]
async fn a_recording_elsewhere_or_ended_does_not_refuse_the_update() {
    for not_running in [
        NotRunning::ParticipantLeft,
        NotRunning::NoStorageConfiguration,
        NotRunning::AnotherStage,
    ] {
        let case = format!("{not_running:?}");
        let emulator = Emulator::new();
        let storage = emulator.create_storage_configuration(&case, "recorded-here").await;
        let other_storage = emulator
            .create_storage_configuration(&case, "recorded-there")
            .await;
        let attached = match not_running {
            NotRunning::NoStorageConfiguration => NO_STORAGE_CONFIGURATION,
            _ => storage.as_str(),
        };
        let stage = emulator.create_stage(&case, "recorded", attached).await;
        let participant = "publisher-1";
        match not_running {
            NotRunning::ParticipantLeft => {
                emulator.join(&case, &stage, participant, "ACTIVE");
                let session_id = emulator.active_session_id(&case, &stage).await;
                assert!(
                    !session_id.is_empty(),
                    "case {case:?}: the join must have opened a session of {stage}"
                );
                emulator.leave(&case, &stage, &session_id, participant);
            }
            NotRunning::NoStorageConfiguration => {
                emulator.join(&case, &stage, participant, "ACTIVE");
            }
            NotRunning::AnotherStage => {
                let elsewhere = emulator
                    .create_stage(&case, "recorded-elsewhere", &storage)
                    .await;
                emulator.join(&case, &elsewhere, participant, "ACTIVE");
            }
        }

        let request = json!({
            "autoParticipantRecordingConfiguration": {
                "storageConfigurationArn": other_storage.as_str(),
            },
        });
        let (status, response) = emulator.update(&stage, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 with no recording running in {stage}, \
             got {status} {response}"
        );
        let recorded_to = emulator.recorded_to(&case, &stage).await;
        assert_eq!(
            recorded_to, other_storage,
            "case {case:?}: expected a read to record to {other_storage:?}, got {recorded_to:?}"
        );
    }
}

/// A refused update is taken as soon as the recording it was refused over has finished:
/// the same request, on the same stage, answers HTTP 409 while the recording runs and
/// HTTP 200 once the recorder reports it stopped.
#[tokio::test]
async fn the_refused_update_is_taken_once_the_recording_has_stopped() {
    let case = "a recording that stops";
    let emulator = Emulator::new();
    let storage = emulator.create_storage_configuration(case, "recorded-here").await;
    let other_storage = emulator
        .create_storage_configuration(case, "recorded-there")
        .await;
    let stage = emulator.create_stage(case, "recorded", &storage).await;
    let participant = "publisher-1";
    emulator.join(case, &stage, participant, "ACTIVE");

    let request = json!({
        "autoParticipantRecordingConfiguration": {
            "storageConfigurationArn": other_storage.as_str(),
        },
    });
    let (status, response) = emulator.update(&stage, request.clone()).await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected HTTP 409 while the recording runs, got {status} {response}"
    );

    // The recorder reports the recording stopped — the same data-plane seam the join came
    // through, which a rejoin of the participant carries the new state on.
    emulator.join(case, &stage, participant, "STOPPED");
    let (status, response) = emulator.update(&stage, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 once the recording stopped, got {status} {response}"
    );
    let recorded_to = emulator.recorded_to(case, &stage).await;
    assert_eq!(
        recorded_to, other_storage,
        "case {case:?}: expected a read to record to {other_storage:?}, got {recorded_to:?}"
    );
}
