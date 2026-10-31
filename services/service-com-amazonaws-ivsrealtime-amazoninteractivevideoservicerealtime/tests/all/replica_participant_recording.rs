// SPDX-License-Identifier: CC0-1.0

//! What a stage that has turned replica participant recording OFF reports about the
//! participants of its sessions, end to end.
//!
//! Every case goes through the router's own `dispatch` with restJson1 requests, exactly as
//! signed `aws ivs-realtime` calls arrive, so what is asserted is the emulator's HTTP
//! behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs: a
//! participant's individual recording is reported only where the stage's
//! `autoParticipantRecordingConfiguration` enables it for THAT participant — every
//! condition the configuration states, each on one of its members. Besides naming a
//! storage configuration the media can be written to, the configuration decides whether a
//! participant that is a replica is recorded at all:
//! `AutoParticipantRecordingConfiguration$recordParticipantReplicas` is an "optional field
//! to disable replica participant recording. If this is set to `false` when a participant
//! is a replica, replica participants are not recorded. Default: `true`"
//! (`smithy_query(query="shape",shape="AutoParticipantRecordingConfiguration")`). So a
//! participant the session reports as a `REPLICA` of another stage carries the documented
//! empty `recordingS3BucketName` / `recordingS3Prefix` on a stage whose configuration sets
//! that member to `false`, and nothing counts it as a running recording — which is what
//! `UpdateStage` refuses an update of that configuration over.
//!
//! The rows differ in what the rule generalises over: the value of
//! `recordParticipantReplicas` (absent, `true`, `false`), how the participant takes part in
//! a replication of the session being read (a replica of another stage, the source of a
//! replication into another stage, or a participant no replication names), and the
//! `ParticipantRecordingState` the recorder reports for it.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage or starts its
//! recording: a participant joins over the stage's data plane with the token it was issued,
//! which is `state::join_stage_session` — the one place a participant record is created. So
//! these cases bring recorded participants into a session through it, reaching the store
//! through this suite's own engine handle, exactly as the `GetParticipant` and
//! `UpdateStage` suites beside it do.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The three values the modelled `ReplicationType` carries, from
/// `smithy_query(query="shape",shape="ReplicationType")`: what a participant reports when
/// it IS a replica of another stage, when it is replicated into one, and when no
/// replication names it.
const REPLICATION_TYPE_REPLICA: &str = "REPLICA";
const REPLICATION_TYPE_SOURCE: &str = "SOURCE";
const REPLICATION_TYPE_NONE: &str = "NONE";

/// The `ParticipantRecordingState` values of a recording media is still being written in,
/// from `smithy_query(query="shape",shape="ParticipantRecordingState")` (`STARTING` |
/// `ACTIVE` | `STOPPING` | `STOPPED` | `FAILED` | `DISABLED`): the states on which where a
/// participant is recorded is a question at all.
const RUNNING_RECORDING_STATES: [&str; 3] = ["STARTING", "ACTIVE", "STOPPING"];

/// The bucket the stage under test records into.
const RECORDING_BUCKET: &str = "replica-recordings";

/// What `recordingS3BucketName` and `recordingS3Prefix` report for a participant that is
/// not being recorded: both members document `""` as the value carried "if recording is not
/// enabled".
const NOT_RECORDED: &str = "";

/// One emulator holding only this service, driven through the router that serves production
/// traffic, plus the engine handle the data-plane seeding writes through.
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

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs`.
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
        text(&response["storageConfiguration"], "arn")
    }

    /// Create a stage from `request` and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, request: &Value) -> String {
        let (status, response) = self.call("CreateStage", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {request} must answer HTTP 200, got {status} {response}"
        );
        text(&response["stage"], "arn")
    }

    /// Issue a participant token for `stage_arn` — its default capabilities include
    /// `PUBLISH`, so its holder is a publisher a replication can name — and return the
    /// participant id it carries.
    async fn create_token(&self, case: &str, stage_arn: &str, user_id: &str) -> String {
        let request = json!({ "stageArn": stage_arn, "userId": user_id });
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        text(&response["participantToken"], "participantId")
    }

    /// Record a participant publishing into `stage_arn` with the recording state the
    /// recorder reports for it, the way the stage's data plane does.
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

    /// Start one replication, asserting it was accepted.
    async fn replicate(&self, case: &str, source: &str, destination: &str, participant_id: &str) {
        let request = json!({
            "sourceStageArn": source,
            "destinationStageArn": destination,
            "participantId": participant_id,
        });
        let (status, response) = self.call("StartParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
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
        let session_id = text(&self.stage(case, arn).await, "activeSessionId");
        assert!(
            !session_id.is_empty(),
            "case {case:?}: a participant in {arn} means the stage has an active session"
        );
        session_id
    }

    /// The `participant` `GetParticipant` reports for one triple.
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

    /// The `ParticipantSummary` `ListParticipants` reports for one participant, so a read
    /// of one participant can be held against the page it appears in.
    async fn summary(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Value {
        let request = json!({ "stageArn": stage_arn, "sessionId": session_id });
        let (status, response) = self.call("ListParticipants", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipants {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["participants"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .find(|summary| summary["participantId"].as_str() == Some(participant_id))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the session must list {participant_id:?}, got {response}")
            })
    }

    /// `UpdateStage` with `request`'s members: the status and the body a client sees.
    async fn update(&self, arn: &str, mut request: Value) -> (u16, Value) {
        request["arn"] = json!(arn);
        self.call("UpdateStage", &request).await
    }
}

/// One string member of a JSON object, as a `String`.
fn text(value: &Value, member: &str) -> String {
    value[member].as_str().unwrap_or_default().to_string()
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// How the participant being read takes part in a replication of the session it is read
/// in — the three values of the modelled `ReplicationType`, which is what the rule's "when
/// a participant is a replica" selects on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// A replica of a participant publishing in ANOTHER stage: the record the destination
    /// stage's session holds.
    Replica,
    /// The publisher a replication carries into another stage, read in its OWN session.
    Source,
    /// A publisher no replication names.
    Unreplicated,
}

impl Role {
    /// The `replicationType` a participant in this role reports.
    fn replication_type(self) -> &'static str {
        match self {
            Self::Replica => REPLICATION_TYPE_REPLICA,
            Self::Source => REPLICATION_TYPE_SOURCE,
            Self::Unreplicated => REPLICATION_TYPE_NONE,
        }
    }
}

/// The recording configuration of the stage under test: it records into `storage`, and
/// `record_replicas` is what the request says about `recordParticipantReplicas` — `None`
/// leaves the member out, so the stage takes the default the model documents for it.
fn recording_configuration(storage: &str, record_replicas: Option<bool>) -> Value {
    let mut configuration = json!({ "storageConfigurationArn": storage });
    if let Some(record_replicas) = record_replicas {
        configuration["recordParticipantReplicas"] = json!(record_replicas);
    }
    configuration
}

/// Bring one participant into the session of the stage under test in `role`, with the
/// recorder reporting `recording_state` for it, and answer that stage, its active session
/// and the participant id.
///
/// The stage under test is the one whose configuration decides the recording: for a replica
/// that is the DESTINATION of the replication, and for the publisher it carries that is the
/// source stage the publisher publishes in.
async fn seed(
    emulator: &Emulator,
    case: &str,
    configuration: &Value,
    role: Role,
    recording_state: &str,
) -> (String, String, String) {
    let configured = json!({
        "name": "under-test",
        "autoParticipantRecordingConfiguration": configuration.clone(),
    });
    match role {
        Role::Replica => {
            let source = emulator
                .create_stage(case, &json!({ "name": "publishes-elsewhere" }))
                .await;
            let destination = emulator.create_stage(case, &configured).await;
            let participant = emulator.create_token(case, &source, "replicated").await;
            // The publisher is in BOTH stages' data planes: it publishes in its own stage,
            // and the destination stage's recorder reports its replica's recording.
            emulator.join(case, &source, &participant, recording_state);
            emulator.join(case, &destination, &participant, recording_state);
            emulator
                .replicate(case, &source, &destination, &participant)
                .await;
            let session = emulator.active_session_id(case, &destination).await;
            (destination, session, participant)
        }
        Role::Source => {
            let source = emulator.create_stage(case, &configured).await;
            let destination = emulator
                .create_stage(case, &json!({ "name": "replicated-into" }))
                .await;
            let participant = emulator.create_token(case, &source, "publisher").await;
            emulator.join(case, &source, &participant, recording_state);
            emulator
                .replicate(case, &source, &destination, &participant)
                .await;
            let session = emulator.active_session_id(case, &source).await;
            (source, session, participant)
        }
        Role::Unreplicated => {
            let stage = emulator.create_stage(case, &configured).await;
            let participant = emulator.create_token(case, &stage, "publisher").await;
            emulator.join(case, &stage, &participant, recording_state);
            let session = emulator.active_session_id(case, &stage).await;
            (stage, session, participant)
        }
    }
}

// ---------------------------------------------------------------------------
// Where a replica's media is reported to go
// ---------------------------------------------------------------------------

/// A participant a session reports as a `REPLICA` is recorded only while the stage's
/// configuration has not turned replica recording off; every other participant of a stage
/// that records is recorded whatever that member says.
///
/// Rows span the three forms the member takes in a request (absent, `true`, `false`), the
/// three roles a participant of a session takes in a replication, and the recording states
/// media is written in.
#[tokio::test]
async fn a_stage_that_records_no_replicas_reports_no_destination_for_one() {
    for record_replicas in [None, Some(true), Some(false)] {
        for role in [Role::Replica, Role::Source, Role::Unreplicated] {
            for recording_state in RUNNING_RECORDING_STATES {
                let case = format!(
                    "recordParticipantReplicas {record_replicas:?}, {role:?}, \
                     recordingState {recording_state}"
                );
                let emulator = Emulator::new();
                let storage = emulator
                    .create_storage_configuration(&case, RECORDING_BUCKET)
                    .await;
                let configuration = recording_configuration(&storage, record_replicas);
                let (stage, session, participant) =
                    seed(&emulator, &case, &configuration, role, recording_state).await;

                // Only a participant that IS a replica is excluded, and only by a
                // configuration that says so.
                let excluded = role == Role::Replica && record_replicas == Some(false);
                let expected_bucket = if excluded { NOT_RECORDED } else { RECORDING_BUCKET };

                let reported = emulator
                    .participant(&case, &stage, &session, &participant)
                    .await;
                let replication_type = reported["replicationType"].as_str();
                assert_eq!(
                    replication_type,
                    Some(role.replication_type()),
                    "case {case:?}: expected replicationType {:?}, got {replication_type:?} \
                     in {reported}",
                    role.replication_type()
                );
                let bucket = reported["recordingS3BucketName"].as_str();
                assert_eq!(
                    bucket,
                    Some(expected_bucket),
                    "case {case:?}: expected recordingS3BucketName {expected_bucket:?}, \
                     got {bucket:?} in {reported}"
                );
                let prefix = reported["recordingS3Prefix"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: expected a recordingS3Prefix, got {reported}")
                });
                if excluded {
                    assert_eq!(
                        prefix, NOT_RECORDED,
                        "case {case:?}: a replica the stage does not record carries the \
                         documented empty prefix, got {prefix:?}"
                    );
                } else {
                    // The member's modelled `@pattern ^[a-zA-Z0-9-]*$` and `@length 0..256`,
                    // from `smithy_query(query="shape",shape="Participant")`.
                    assert!(
                        !prefix.is_empty()
                            && prefix.len() <= 256
                            && prefix
                                .chars()
                                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
                        "case {case:?}: expected a prefix the modelled pattern admits, \
                         got {prefix:?}"
                    );
                }

                // The recorder's own report is untouched by where the media goes, and the
                // page the participant appears in agrees with the read of it.
                let state = reported["recordingState"].as_str();
                assert_eq!(
                    state,
                    Some(recording_state),
                    "case {case:?}: expected recordingState {recording_state:?}, \
                     got {state:?} in {reported}"
                );
                let summary = emulator
                    .summary(&case, &stage, &session, &participant)
                    .await;
                for member in ["recordingState", "replicationType"] {
                    assert_eq!(
                        summary[member], reported[member],
                        "case {case:?}: a read of {member} and the page it appears in must \
                         agree, got {summary} and {reported}"
                    );
                }

                // Being a read, it leaves every answer exactly as it was.
                let again = emulator
                    .participant(&case, &stage, &session, &participant)
                    .await;
                assert_eq!(
                    again, reported,
                    "case {case:?}: reading the participant twice must answer identically, \
                     got {reported} then {again}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// What an update is refused over
// ---------------------------------------------------------------------------

/// A replica the stage's configuration excludes from recording is no running recording of
/// that stage, so `UpdateStage` takes the update of the recording configuration that a
/// recording it DOES make would refuse with `ConflictException` (HTTP 409).
///
/// Rows span the three forms `recordParticipantReplicas` takes in a request; the only
/// participant of the stage is the replica.
#[tokio::test]
async fn an_excluded_replica_is_no_running_recording_of_the_stage() {
    for record_replicas in [None, Some(true), Some(false)] {
        let case = format!("update with recordParticipantReplicas {record_replicas:?}");
        let emulator = Emulator::new();
        let storage = emulator
            .create_storage_configuration(&case, RECORDING_BUCKET)
            .await;
        let elsewhere = emulator
            .create_storage_configuration(&case, "other-recordings")
            .await;
        let configuration = recording_configuration(&storage, record_replicas);
        let (stage, _session, participant) =
            seed(&emulator, &case, &configuration, Role::Replica, "ACTIVE").await;

        let (status, response) = emulator
            .update(
                &stage,
                json!({
                    "autoParticipantRecordingConfiguration": {
                        "storageConfigurationArn": elsewhere.as_str(),
                    },
                }),
            )
            .await;
        let recorded_to = text(
            &emulator.stage(&case, &stage).await["autoParticipantRecordingConfiguration"],
            "storageConfigurationArn",
        );
        if record_replicas == Some(false) {
            assert_eq!(
                status, 200,
                "case {case:?}: the stage records no replica, so the update of \
                 {participant:?}'s stage must be taken, got {status} {response}"
            );
            assert_eq!(
                recorded_to, elsewhere,
                "case {case:?}: expected a read to record to {elsewhere:?}, got {recorded_to:?}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {case:?}: the replica's recording runs, so the update must be \
                 refused, got {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {response}"
            );
            assert_eq!(
                recorded_to, storage,
                "case {case:?}: a refused update must leave the stage recording to \
                 {storage:?}, got {recorded_to:?}"
            );
        }
    }
}
