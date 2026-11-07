// SPDX-License-Identifier: CC0-1.0

//! Which participants `DisconnectParticipant` can take out of a stage, end to end.
//!
//! Every case goes through the router's own `dispatch` with restJson1 requests, exactly as
//! signed `aws ivs-realtime` calls arrive, so what is asserted is the emulator's HTTP
//! behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs: a stage has
//! the participants its OWN records hold — every participant record in a session of that
//! stage, whichever call recorded it — together with the identities this service issued
//! against that stage, so every participant the stage's readers report as one of its
//! participants is one the operation whose `stageArn` is "the stage to which the
//! participant is attached" disconnects (HTTP 200, and the record reports `DISCONNECTED`
//! afterwards). A participant the stage holds neither a record nor an identity for has no
//! connection to end, so that call is the same empty 200 and changes nothing.
//!
//! The rows differ in what the rule generalises over: HOW the stage came to hold the
//! record (a replication putting a replica of another stage's publisher into it, with the
//! replicated publisher identified by a participant token or by an ingest configuration's
//! credential; a data-plane join under an identity issued against a different stage), what
//! else the stage holds beside it (nothing, a participant of its own), and how many stages
//! hold a record of the same participant at once.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the participant token or ingest key
//! it was issued, which is `state::join_stage_session` — the one place a participant record
//! is created. So the rows that need a plain join bring the participant in through it,
//! reaching the store through this suite's own engine handle, exactly as the
//! `DisconnectParticipant` and `ListParticipants` suites beside this one do.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The two values the modelled `ParticipantState` carries, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const CONNECTED: &str = "CONNECTED";
const DISCONNECTED: &str = "DISCONNECTED";

/// What a participant replicated INTO a stage reports for `replicationType`, from
/// `smithy_query(query="shape",shape="ReplicationType")`.
const REPLICATION_TYPE_REPLICA: &str = "REPLICA";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle the data-plane seeding writes through.
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

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs`.
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

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name:?} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Issue a participant token for `stage_arn` and return the participant id it carries
    /// — the id the holder of that token joins a stage under.
    async fn create_token(&self, case: &str, stage_arn: &str, user_id: &str) -> String {
        let request = json!({ "stageArn": stage_arn, "userId": user_id });
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Associate an ingest configuration with `stage_arn` and return its ARN and the
    /// participant its ingest credential publishes as.
    async fn create_ingest_participant(&self, case: &str, stage_arn: &str) -> (String, String) {
        let request = json!({ "ingestProtocol": "RTMPS", "stageArn": stage_arn });
        let (status, response) = self.call("CreateIngestConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration {request} must answer HTTP 200, \
             got {status} {response}"
        );
        (
            member(&response["ingestConfiguration"], "arn"),
            member(&response["ingestConfiguration"], "participantId"),
        )
    }

    /// The `stageArn` an ingest configuration reports, read back over the wire.
    async fn ingest_configuration_stage(&self, case: &str, arn: &str) -> String {
        let (status, response) = self
            .call("GetIngestConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetIngestConfiguration {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["ingestConfiguration"], "stageArn")
    }

    /// Start one replication of `participant_id` from `source` into `destination`.
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

    /// The stage's `activeSessionId`, read back over the wire (the empty string once the
    /// stage has no active session).
    async fn active_session_id(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
    }

    /// The record `ListParticipants` reports for one participant of one session of one
    /// stage, or `None` when that session holds no record for it.
    async fn participant_view(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Option<Value> {
        let request = json!({ "stageArn": stage_arn, "sessionId": session_id });
        let (status, response) = self.call("ListParticipants", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipants {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["participants"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|participant| member(participant, "participantId") == participant_id)
    }

    /// `DisconnectParticipant` for one stage and one participant.
    async fn disconnect(&self, stage_arn: &str, participant_id: &str) -> (u16, Value) {
        self.call(
            "DisconnectParticipant",
            &json!({
                "stageArn": stage_arn,
                "participantId": participant_id,
                "reason": "the suite disconnects it",
            }),
        )
        .await
    }
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Does this answer carry no members, as the modelled `DisconnectParticipantResponse`
/// (a structure with no members) requires?
fn carries_no_members(response: &Value) -> bool {
    response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty)
}

/// How the stage under test came to hold the record of the participant the case
/// disconnects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recorded {
    /// A replication put a replica of a token-issued publisher of another stage into it.
    ReplicaOfTokenPublisher,
    /// A replication put a replica of a publisher that reached its own stage with an
    /// ingest configuration's credential into it.
    ReplicaOfIngestPublisher,
    /// As `ReplicaOfTokenPublisher`, and the stage also holds a participant of its own.
    ReplicaBesideAParticipantOfItsOwn,
    /// One publisher replicated into TWO stages: the case disconnects it from one of them.
    ReplicaOfAPublisherReplicatedTwice,
    /// A data-plane join under an identity this service issued against a DIFFERENT stage.
    JoinUnderAnotherStagesIdentity,
}

/// One participant record that must be where it was once the case's disconnect ran.
struct StillConnected {
    stage_arn: String,
    session_id: String,
    participant_id: String,
}

/// What one case built: the stage and participant the disconnect names, and what else must
/// come out of it unchanged.
struct Scenario {
    /// The stage the disconnect names, and its session.
    stage_arn: String,
    session_id: String,
    /// The participant the disconnect names — one the stage reports as its own.
    participant_id: String,
    /// What the participant reports for `replicationType` before the disconnect.
    replication_type: Option<&'static str>,
    /// Records in other stages, or in this one, that the disconnect must not move.
    still_connected: Vec<StillConnected>,
    /// An ingest configuration the case created, and the `stageArn` it must still report.
    ingest_configuration: Option<(String, String)>,
}

/// Build the case's stages, identities and records, and say what the disconnect names.
async fn build(emulator: &RealTime, recorded: Recorded) -> Scenario {
    let case = format!("{recorded:?}");
    let case = case.as_str();
    match recorded {
        Recorded::ReplicaOfTokenPublisher | Recorded::ReplicaBesideAParticipantOfItsOwn => {
            let source = emulator.create_stage(case, "source").await;
            let destination = emulator.create_stage(case, "destination").await;
            let publisher = emulator.create_token(case, &source, "publisher").await;
            // The destination stage's own participant, joined over its data plane before
            // the replication, so the replica shares the session with it.
            let resident = if recorded == Recorded::ReplicaBesideAParticipantOfItsOwn {
                let resident = emulator.create_token(case, &destination, "resident").await;
                emulator.join(
                    case,
                    &destination,
                    state::JoinedParticipant {
                        participant_id: resident.clone(),
                        user_id: Some("resident".to_string()),
                        published: false,
                        ..Default::default()
                    },
                );
                Some(resident)
            } else {
                None
            };
            emulator
                .replicate(case, &source, &destination, &publisher)
                .await;
            let source_session = emulator.active_session_id(case, &source).await;
            let destination_session = emulator.active_session_id(case, &destination).await;
            let mut still_connected = vec![StillConnected {
                stage_arn: source.clone(),
                session_id: source_session,
                participant_id: publisher.clone(),
            }];
            if let Some(resident) = resident {
                still_connected.push(StillConnected {
                    stage_arn: destination.clone(),
                    session_id: destination_session.clone(),
                    participant_id: resident,
                });
            }
            Scenario {
                stage_arn: destination,
                session_id: destination_session,
                participant_id: publisher,
                replication_type: Some(REPLICATION_TYPE_REPLICA),
                still_connected,
                ingest_configuration: None,
            }
        }
        Recorded::ReplicaOfIngestPublisher => {
            let source = emulator.create_stage(case, "source").await;
            let destination = emulator.create_stage(case, "destination").await;
            let (configuration_arn, publisher) =
                emulator.create_ingest_participant(case, &source).await;
            emulator.join(
                case,
                &source,
                state::JoinedParticipant {
                    participant_id: publisher.clone(),
                    published: true,
                    ingest_configuration_arn: Some(configuration_arn.clone()),
                    ..Default::default()
                },
            );
            emulator
                .replicate(case, &source, &destination, &publisher)
                .await;
            let source_session = emulator.active_session_id(case, &source).await;
            let destination_session = emulator.active_session_id(case, &destination).await;
            Scenario {
                stage_arn: destination,
                session_id: destination_session,
                participant_id: publisher.clone(),
                replication_type: Some(REPLICATION_TYPE_REPLICA),
                still_connected: vec![StillConnected {
                    stage_arn: source.clone(),
                    session_id: source_session,
                    participant_id: publisher,
                }],
                // The configuration is associated with the SOURCE stage, which is not the
                // stage this case disconnects from, so it keeps that association.
                ingest_configuration: Some((configuration_arn, source)),
            }
        }
        Recorded::ReplicaOfAPublisherReplicatedTwice => {
            let source = emulator.create_stage(case, "source").await;
            let first = emulator.create_stage(case, "destination-one").await;
            let second = emulator.create_stage(case, "destination-two").await;
            let publisher = emulator.create_token(case, &source, "publisher").await;
            for destination in [&first, &second] {
                emulator
                    .replicate(case, &source, destination, &publisher)
                    .await;
            }
            let source_session = emulator.active_session_id(case, &source).await;
            let first_session = emulator.active_session_id(case, &first).await;
            let second_session = emulator.active_session_id(case, &second).await;
            Scenario {
                stage_arn: first,
                session_id: first_session,
                participant_id: publisher.clone(),
                replication_type: Some(REPLICATION_TYPE_REPLICA),
                still_connected: vec![
                    StillConnected {
                        stage_arn: source,
                        session_id: source_session,
                        participant_id: publisher.clone(),
                    },
                    StillConnected {
                        stage_arn: second,
                        session_id: second_session,
                        participant_id: publisher,
                    },
                ],
                ingest_configuration: None,
            }
        }
        Recorded::JoinUnderAnotherStagesIdentity => {
            let issuing = emulator.create_stage(case, "issuing").await;
            let joined_stage = emulator.create_stage(case, "joined").await;
            let participant_id = emulator.create_token(case, &issuing, "visitor").await;
            emulator.join(
                case,
                &joined_stage,
                state::JoinedParticipant {
                    participant_id: participant_id.clone(),
                    user_id: Some("visitor".to_string()),
                    published: true,
                    ..Default::default()
                },
            );
            let session_id = emulator.active_session_id(case, &joined_stage).await;
            Scenario {
                stage_arn: joined_stage,
                session_id,
                participant_id,
                // No replication names it, so it reports the documented default, which
                // this suite does not assert beyond its presence.
                replication_type: None,
                still_connected: Vec::new(),
                ingest_configuration: None,
            }
        }
    }
}

/// Every participant a stage reports as one of its own can be disconnected from that
/// stage, whichever call put the record there, and the disconnect moves nothing else.
#[tokio::test]
async fn a_participant_the_stage_reports_is_one_the_stage_can_disconnect() {
    let cases = [
        Recorded::ReplicaOfTokenPublisher,
        Recorded::ReplicaOfIngestPublisher,
        Recorded::ReplicaBesideAParticipantOfItsOwn,
        Recorded::ReplicaOfAPublisherReplicatedTwice,
        Recorded::JoinUnderAnotherStagesIdentity,
    ];

    for recorded in cases {
        let case = format!("{recorded:?}");
        let case = case.as_str();
        let emulator = RealTime::new();
        let scenario = build(&emulator, recorded).await;

        // The premise: the stage the disconnect names reports this participant as one of
        // its participants, present in its session.
        let before = emulator
            .participant_view(
                case,
                &scenario.stage_arn,
                &scenario.session_id,
                &scenario.participant_id,
            )
            .await;
        let listed = before.clone().unwrap_or(Value::Null);
        assert_eq!(
            before.as_ref().map(|participant| member(participant, "state")),
            Some(CONNECTED.to_string()),
            "case {case:?}: expected {} to report {:?} {CONNECTED}, got {listed}",
            scenario.stage_arn,
            scenario.participant_id,
        );
        if let Some(expected) = scenario.replication_type {
            let actual = member(&listed, "replicationType");
            assert_eq!(
                actual, expected,
                "case {case:?}: expected replicationType {expected} for {:?}, \
                 got {actual:?} in {listed}",
                scenario.participant_id,
            );
        }

        // The participant the stage just reported is one the stage can disconnect.
        let (status, response) = emulator
            .disconnect(&scenario.stage_arn, &scenario.participant_id)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 disconnecting {:?} from {}, \
             got {status} {response}",
            scenario.participant_id, scenario.stage_arn,
        );
        assert!(
            carries_no_members(&response),
            "case {case:?}: expected an answer carrying no member, got {response}"
        );

        // And it is out of the stage: the record stays in the session and reports
        // DISCONNECTED.
        let after = emulator
            .participant_view(
                case,
                &scenario.stage_arn,
                &scenario.session_id,
                &scenario.participant_id,
            )
            .await;
        let listed = after.clone().unwrap_or(Value::Null);
        assert_eq!(
            after.as_ref().map(|participant| member(participant, "state")),
            Some(DISCONNECTED.to_string()),
            "case {case:?}: expected {:?} to report {DISCONNECTED} in {}, got {listed}",
            scenario.participant_id,
            scenario.stage_arn,
        );

        // Nothing else moved: every other record of the same participant, and every other
        // participant of the same session, is where it was.
        for untouched in &scenario.still_connected {
            let actual = emulator
                .participant_view(
                    case,
                    &untouched.stage_arn,
                    &untouched.session_id,
                    &untouched.participant_id,
                )
                .await;
            let listed = actual.clone().unwrap_or(Value::Null);
            assert_eq!(
                actual
                    .as_ref()
                    .map(|participant| member(participant, "state")),
                Some(CONNECTED.to_string()),
                "case {case:?}: expected {:?} in {} to stay {CONNECTED}, got {listed}",
                untouched.participant_id,
                untouched.stage_arn,
            );
        }

        // A configuration associated with another stage than the one disconnected from is
        // not the one this operation dissociates.
        if let Some((arn, expected_stage_arn)) = &scenario.ingest_configuration {
            let actual = emulator.ingest_configuration_stage(case, arn).await;
            assert_eq!(
                &actual, expected_stage_arn,
                "case {case:?}: expected {arn} to keep stageArn {expected_stage_arn:?}, \
                 got {actual:?}"
            );
        }

        // A participant the stage holds neither a record nor an identity for has no
        // connection to end, so the call is the idempotent empty 200 and changes nothing;
        // a second disconnect of the departed participant is accepted the same way.
        let (status, response) = emulator
            .disconnect(&scenario.stage_arn, "nothing-minted-this-id")
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for an id {} holds nothing for, \
             got {status} {response}",
            scenario.stage_arn,
        );
        assert!(
            carries_no_members(&response),
            "case {case:?}: expected an answer carrying no member, got {response}"
        );
        let (status, response) = emulator
            .disconnect(&scenario.stage_arn, &scenario.participant_id)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated disconnect of {:?} to answer HTTP 200, \
             got {status} {response}",
            scenario.participant_id,
        );
        let after = emulator
            .participant_view(
                case,
                &scenario.stage_arn,
                &scenario.session_id,
                &scenario.participant_id,
            )
            .await;
        let listed = after.clone().unwrap_or(Value::Null);
        assert_eq!(
            after.as_ref().map(|participant| member(participant, "state")),
            Some(DISCONNECTED.to_string()),
            "case {case:?}: expected {:?} to stay {DISCONNECTED}, got {listed}",
            scenario.participant_id,
        );
    }
}
