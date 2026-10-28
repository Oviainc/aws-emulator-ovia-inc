// SPDX-License-Identifier: CC0-1.0

//! Deleting a stage that is an END of a running participant replication, end to end.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body — never
//! an internal call. The only internal write is the data-plane seeding of a participant
//! joining a stage, which no control-plane operation performs.
//!
//! The RULE under test, stated once and then exercised over a SET of rows: a replication
//! cannot outlive either stage it names, and a replication that has ended leaves no replica
//! behind — the participant it put into the DESTINATION stage's session is reported
//! `DISCONNECTED` there, its departure and the end of its media are occurrences of that
//! session, and that session "ends after the last participant leaves the stage" when it
//! held nobody else. So deleting a stage, which "shuts down and deletes the specified stage
//! (disconnecting all participants)", leaves the stage at the other end of each of its
//! replications exactly as stopping those replications would have, while every participant
//! the deleted stage did not put there stays where it is.
//!
//! The rows differ in what the rule generalises over: how many destination stages the
//! deleted stage replicates into, whether a destination stage also holds a participant of
//! its own, whether another source stage replicates into the same destination stage, and
//! whether a replication had already been stopped before the delete.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;
use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The two `ParticipantState` values of
/// `smithy_query(query="shape",shape="ParticipantState")`.
const CONNECTED: &str = "CONNECTED";
const DISCONNECTED: &str = "DISCONNECTED";

/// The `ReplicationType` values of `smithy_query(query="shape",shape="ReplicationType")`
/// this suite reads: what a replica reports while its replication is held, and the
/// documented default ("Default: `NONE`") a participant no replication names reports.
const REPLICATION_TYPE_REPLICA: &str = "REPLICA";
const REPLICATION_TYPE_NONE: &str = "NONE";

/// The state a started replication is in, from
/// `smithy_query(query="shape",shape="ReplicationState")` (`ACTIVE` | `STOPPED`).
const ACTIVE: &str = "ACTIVE";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it,
/// plus the engine handle the data-plane seeding writes through.
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

    /// A participant of `stage_arn`, from a token issued for it: the identity the model
    /// says a `participantId` comes from ("assigned by IVS and returned by
    /// `CreateParticipantToken`").
    async fn participant_id(&self, case: &str, stage_arn: &str) -> String {
        let request = json!({ "stageArn": stage_arn });
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Start one replication, asserting it was accepted.
    async fn replicate(&self, case: &str, source: &str, destination: &str, participant: &str) {
        let request = replication_request(source, destination, participant);
        let (status, response) = self.call("StartParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// Stop one replication, answering the status and body unchanged.
    async fn stop(&self, source: &str, destination: &str, participant: &str) -> (u16, Value) {
        self.call(
            "StopParticipantReplication",
            &replication_request(source, destination, participant),
        )
        .await
    }

    /// Delete one stage, answering the status and body unchanged.
    async fn delete_stage(&self, stage_arn: &str) -> (u16, Value) {
        self.call("DeleteStage", &json!({ "arn": stage_arn })).await
    }

    /// The `activeSessionId` `GetStage` reports for `stage_arn`, empty when the stage holds
    /// no session running.
    async fn active_session(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage on {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
    }

    /// The `Participant` `GetParticipant` reports for one participant of one session.
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

    /// The `StageSession` `GetStageSession` reports for one session of one stage.
    async fn session(&self, case: &str, stage_arn: &str, session_id: &str) -> Value {
        let request = json!({ "stageArn": stage_arn, "sessionId": session_id });
        let (status, response) = self.call("GetStageSession", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStageSession {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response["stageSession"].clone()
    }

    /// The occurrence names `ListParticipantEvents` reports for one participant of one
    /// session, in occurrence order, each with the `replica` flag it carries.
    async fn occurrences(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Vec<(String, bool)> {
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
            .iter()
            .map(|event| {
                (
                    member(event, "name"),
                    event["replica"].as_bool().unwrap_or(false),
                )
            })
            .collect()
    }

    /// The replicas this store reports for `participant_id` of `source_stage_arn`.
    async fn replicas(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
    ) -> Vec<Value> {
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
        response["replicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: replicas is @required, got {response}"))
            .clone()
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does —
    /// nothing in this control plane makes a participant join.
    fn join(&self, case: &str, stage_arn: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(
                &mut store,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.to_string(),
                    attributes: HashMap::new(),
                    ..Default::default()
                },
            )
            .is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
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

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> String {
    member(response, "__type")
}

/// The three members every request that names a replication carries.
fn replication_request(source: &str, destination: &str, participant: &str) -> Value {
    json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": participant,
    })
}

/// The occurrences a replica's record in a destination session carries once the
/// replication that put it there has ended: it joined publishing, and its media and then
/// the participant itself went — every one of them happening to a replica
/// (`Event$replica`).
fn ended_replica_occurrences() -> Vec<(String, bool)> {
    ["JOINED", "PUBLISH_STARTED", "PUBLISH_STOPPED", "LEFT"]
        .into_iter()
        .map(|name| (name.to_string(), true))
        .collect()
}

/// One row of the class: what this store holds when the SOURCE stage of a replication is
/// deleted.
struct DeleteCase {
    case: &'static str,
    /// How many destination stages the deleted stage's publisher is replicated into.
    destinations: usize,
    /// Whether each destination stage also holds a participant of its OWN, which no
    /// replication put there.
    local_participant: bool,
    /// Whether a SECOND source stage replicates its own publisher into the first
    /// destination stage.
    other_source: bool,
    /// Whether the replication into the first destination stage was already stopped before
    /// the delete.
    stopped_first: bool,
}

/// The rows this suite spans the class with: each differs along what the rule generalises
/// over — the number of replications the deleted stage sources, who else is in the
/// destination stage, and whether a replication had already ended.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            case: "one replication, nobody else in the destination stage",
            destinations: 1,
            local_participant: false,
            other_source: false,
            stopped_first: false,
        },
        DeleteCase {
            case: "one replication, the destination stage holds a participant of its own",
            destinations: 1,
            local_participant: true,
            other_source: false,
            stopped_first: false,
        },
        DeleteCase {
            case: "one publisher replicated into two stages",
            destinations: 2,
            local_participant: false,
            other_source: false,
            stopped_first: false,
        },
        DeleteCase {
            case: "one publisher replicated into four stages",
            destinations: 4,
            local_participant: false,
            other_source: false,
            stopped_first: false,
        },
        DeleteCase {
            case: "two replications and a participant of each destination stage",
            destinations: 2,
            local_participant: true,
            other_source: false,
            stopped_first: false,
        },
        DeleteCase {
            case: "another stage replicates its own publisher into the same destination",
            destinations: 1,
            local_participant: false,
            other_source: true,
            stopped_first: false,
        },
        DeleteCase {
            case: "the replication into the destination stage was already stopped",
            destinations: 1,
            local_participant: false,
            other_source: false,
            stopped_first: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// Deleting the source stage of a replication
// ---------------------------------------------------------------------------

/// THE RULE: deleting a stage ends every replication it sources, and each ended
/// replication's replica leaves the destination stage's session — reported `DISCONNECTED`
/// with its departure and the end of its media recorded as a replica's occurrences, and the
/// session ended when it held nobody else — while whoever else is in that stage stays.
#[tokio::test]
async fn deleting_a_source_stage_takes_its_replicas_out_of_the_destination_stages() {
    for row in delete_cases() {
        let case = row.case;
        let emulator = RealTime::new();
        let source = emulator.stage(case, "source").await;
        let publisher = emulator.participant_id(case, &source).await;

        // Every destination stage: its own participant first where the row asks for one, so
        // the replica joins a session the stage already had.
        let mut destinations = Vec::new();
        for index in 0..row.destinations {
            let destination = emulator.stage(case, &format!("destination-{index}")).await;
            let local = if row.local_participant {
                let local = emulator.participant_id(case, &destination).await;
                emulator.join(case, &destination, &local);
                Some(local)
            } else {
                None
            };
            emulator
                .replicate(case, &source, &destination, &publisher)
                .await;
            let session = emulator.active_session(case, &destination).await;
            assert!(
                !session.is_empty(),
                "case {case:?}: a replication into {destination} must have given it a \
                 session, got {session:?}"
            );
            destinations.push((destination, session, local));
        }

        // A second source stage replicating its own publisher into the first destination
        // stage: a replication the delete does not name, whose replica shares that session.
        let other = if row.other_source {
            let other_source = emulator.stage(case, "other-source").await;
            let other_publisher = emulator.participant_id(case, &other_source).await;
            emulator
                .replicate(case, &other_source, &destinations[0].0, &other_publisher)
                .await;
            Some((other_source, other_publisher))
        } else {
            None
        };

        // A replication already stopped before the delete: its replica has left already, so
        // the delete has nothing further to take out of that session.
        if row.stopped_first {
            let (status, response) = emulator.stop(&source, &destinations[0].0, &publisher).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StopParticipantReplication must answer HTTP 200, \
                 got {status} {response}"
            );
        }

        // Before the delete: the replica is IN each destination stage (except where the row
        // stopped its replication), which is the difference the delete has to make.
        for (index, (destination, session, _)) in destinations.iter().enumerate() {
            let expected = if row.stopped_first && index == 0 {
                DISCONNECTED
            } else {
                CONNECTED
            };
            let replica = emulator
                .participant(case, destination, session, &publisher)
                .await;
            assert_eq!(
                member(&replica, "state"),
                expected,
                "case {case:?}: before the delete the replica in {destination} must be \
                 {expected}, got {replica}"
            );
        }

        let (status, response) = emulator.delete_stage(&source).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStage {source} must answer HTTP 200, got {status} {response}"
        );

        for (index, (destination, session, local)) in destinations.iter().enumerate() {
            // The replica is OUT of the destination stage: its record stays, as a
            // participant of that session that has left.
            let replica = emulator
                .participant(case, destination, session, &publisher)
                .await;
            assert_eq!(
                member(&replica, "state"),
                DISCONNECTED,
                "case {case:?}: the replica in {destination} must have left the stage, \
                 got {replica}"
            );
            // A replication cannot outlive a stage it names, so the record that made this
            // participant a replica is gone and it reports the documented default.
            assert_eq!(
                member(&replica, "replicationType"),
                REPLICATION_TYPE_NONE,
                "case {case:?}: the replication of {destination} went with the deleted \
                 stage, got {replica}"
            );

            // The departure, and the end of the media the replication carried, are
            // occurrences of that session happening to a replica — recorded ONCE, whether
            // the replication ended at the delete or at a stop before it.
            let events = emulator
                .occurrences(case, destination, session, &publisher)
                .await;
            assert_eq!(
                events,
                ended_replica_occurrences(),
                "case {case:?}: the replica's occurrences in {destination} must end with \
                 its media stopping and its departure, got {events:?}"
            );

            // A session "ends after the last participant leaves the stage": it ends here
            // exactly when the replica was the last one in it.
            let still_held = local.is_some() || (index == 0 && other.is_some());
            let active = emulator.active_session(case, destination).await;
            let reported = emulator.session(case, destination, session).await;
            if still_held {
                assert_eq!(
                    &active, session,
                    "case {case:?}: {destination} still holds another participant, so its \
                     session must be the same one, got {active:?}"
                );
                assert!(
                    reported["endTime"].is_null(),
                    "case {case:?}: the session of {destination} is still active, so it \
                     carries no endTime, got {reported}"
                );
            } else {
                assert_eq!(
                    active, "",
                    "case {case:?}: the last participant of {destination} has left, so the \
                     stage must report no session, got {active:?}"
                );
                assert!(
                    !reported["endTime"].is_null(),
                    "case {case:?}: the session of {destination} ended with its last \
                     participant, so it carries an endTime, got {reported}"
                );
            }

            // Whoever the deleted stage did not put there is untouched.
            if let Some(local) = local {
                let held = emulator
                    .participant(case, destination, session, local)
                    .await;
                assert_eq!(
                    member(&held, "state"),
                    CONNECTED,
                    "case {case:?}: the participant of {destination}'s own must stay in it, \
                     got {held}"
                );
            }
        }

        // The replication another stage sources is still running, and its replica is still
        // in the destination stage the delete left standing.
        if let Some((other_source, other_publisher)) = &other {
            let replicas = emulator.replicas(case, other_source, other_publisher).await;
            assert_eq!(
                replicas.len(),
                1,
                "case {case:?}: the replication {other_source} sources must survive the \
                 delete, got {replicas:?}"
            );
            assert_eq!(
                member(&replicas[0], "replicationState"),
                ACTIVE,
                "case {case:?}: the replication {other_source} sources was not ended, \
                 got {replicas:?}"
            );
            let (destination, session, _) = &destinations[0];
            let held = emulator
                .participant(case, destination, session, other_publisher)
                .await;
            assert_eq!(
                member(&held, "state"),
                CONNECTED,
                "case {case:?}: the replica of {other_publisher} must stay in {destination}, \
                 got {held}"
            );
            assert_eq!(
                member(&held, "replicationType"),
                REPLICATION_TYPE_REPLICA,
                "case {case:?}: the replica of {other_publisher} is still a replica, \
                 got {held}"
            );
        }

        // Nothing of the deleted stage's replications is left to report or to stop: the
        // records went with the stage, and both ends of each of them are settled.
        let left_over = emulator.replicas(case, &source, &publisher).await;
        assert!(
            left_over.is_empty(),
            "case {case:?}: the deleted stage sources no replication any more, \
             got {left_over:?}"
        );
        let (status, response) = emulator.stop(&source, &destinations[0].0, &publisher).await;
        assert_eq!(
            status, 404,
            "case {case:?}: stopping a replication of a deleted stage must answer HTTP 404, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: got {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Deleting the destination stage of a replication
// ---------------------------------------------------------------------------

/// THE OTHER END: deleting the DESTINATION stage ends the replication too — the records go
/// with the stage — but the publisher being replicated is a participant of the stage it
/// publishes in, which this delete does not touch, so it stays in it and its session stays
/// open.
#[tokio::test]
async fn deleting_a_destination_stage_leaves_the_publisher_in_its_own_stage() {
    for (case, destinations) in [
        ("the only destination stage is deleted", 1usize),
        ("one destination stage of two is deleted", 2),
        ("one destination stage of three is deleted", 3),
    ] {
        let emulator = RealTime::new();
        let source = emulator.stage(case, "source").await;
        let publisher = emulator.participant_id(case, &source).await;
        let mut stages = Vec::new();
        for index in 0..destinations {
            let destination = emulator.stage(case, &format!("destination-{index}")).await;
            emulator
                .replicate(case, &source, &destination, &publisher)
                .await;
            stages.push(destination);
        }
        let session = emulator.active_session(case, &source).await;

        let (status, response) = emulator.delete_stage(&stages[0]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStage {} must answer HTTP 200, got {status} {response}",
            stages[0]
        );

        // The publisher publishes in its own stage whether or not anything replicates it.
        let held = emulator
            .participant(case, &source, &session, &publisher)
            .await;
        assert_eq!(
            member(&held, "state"),
            CONNECTED,
            "case {case:?}: deleting a destination stage must not disconnect the publisher, \
             got {held}"
        );
        assert_eq!(
            emulator.active_session(case, &source).await,
            session,
            "case {case:?}: the publisher's own session must still be the stage's active one"
        );

        // Exactly the replications naming the deleted stage are gone; every other one the
        // publisher has is still reported, and still running.
        let replicas = emulator.replicas(case, &source, &publisher).await;
        assert_eq!(
            replicas.len(),
            destinations - 1,
            "case {case:?}: only the replication into the deleted stage must be gone, \
             got {replicas:?}"
        );
        for replica in &replicas {
            assert_ne!(
                member(replica, "destinationStageArn"),
                stages[0],
                "case {case:?}: no replication may name the deleted stage, got {replica}"
            );
            assert_eq!(
                member(replica, "replicationState"),
                ACTIVE,
                "case {case:?}: the other replications were not ended, got {replica}"
            );
        }
    }
}
