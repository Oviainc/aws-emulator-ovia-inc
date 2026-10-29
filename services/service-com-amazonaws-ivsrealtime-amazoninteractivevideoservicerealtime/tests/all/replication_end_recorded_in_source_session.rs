// SPDX-License-Identifier: CC0-1.0

//! How the END of a running participant replication is recorded in the session of the stage
//! whose publisher was being replicated, end to end.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status and the JSON body — never an internal
//! call.
//!
//! The RULE under test, stated once and then exercised over a SET of rows: a running
//! replication that ends — whichever way it ends, a caller stopping it or either stage it
//! names going away — records exactly one `REPLICATION_STOPPED` occurrence in the session of
//! the stage its publisher publishes in, carrying the destination stage and destination
//! session that replication named (`Event$destinationStageArn` and
//! `Event$destinationSessionId` are "applicable only if the event name is
//! `REPLICATION_STARTED` or `REPLICATION_STOPPED`"), after the `REPLICATION_STARTED` that
//! named the same destination, and happening to the publisher rather than to a replica
//! (`Event$replica`). A replication that had already ended records nothing a second time,
//! and the end of a replication is an occurrence of its SOURCE session only — so when the
//! source stage itself is the one going away there is no session left to report it.
//!
//! The rows differ in what the rule generalises over: the way the replication ends, how many
//! destination stages the publisher is replicated into, how many publishers of one stage are
//! replicated into the ending one, whether another stage replicates its own publisher into
//! it, and whether the replication had already been stopped before the stage went.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The two `EventName` values of `smithy_query(query="shape",shape="EventName")` this suite
/// reads: what a replication's start records in the publisher's session, and what its end
/// records there.
const REPLICATION_STARTED: &str = "REPLICATION_STARTED";
const REPLICATION_STOPPED: &str = "REPLICATION_STOPPED";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
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

    /// A participant of `stage_arn`, from a token issued for it: the identity the model says
    /// a `participantId` comes from ("assigned by IVS and returned by
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

    /// Stop one replication, asserting it was accepted.
    async fn stop(&self, case: &str, source: &str, destination: &str, participant: &str) {
        let request = replication_request(source, destination, participant);
        let (status, response) = self.call("StopParticipantReplication", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// Delete one stage, asserting it was accepted.
    async fn delete_stage(&self, case: &str, stage_arn: &str) {
        let (status, response) = self.call("DeleteStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStage {stage_arn} must answer HTTP 200, \
             got {status} {response}"
        );
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

    /// `ListParticipantEvents` for one participant of one session, status and body unchanged.
    async fn events_response(
        &self,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> (u16, Value) {
        self.call(
            "ListParticipantEvents",
            &json!({
                "stageArn": stage_arn,
                "sessionId": session_id,
                "participantId": participant_id,
            }),
        )
        .await
    }

    /// The occurrences `ListParticipantEvents` reports for one participant of one session, in
    /// occurrence order.
    async fn events(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Vec<Value> {
        let (status, response) = self
            .events_response(stage_arn, session_id, participant_id)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantEvents on {participant_id} of {stage_arn} must \
             answer HTTP 200, got {status} {response}"
        );
        response["events"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: events is @required, got {response}"))
            .clone()
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

/// The three members every request that names a replication carries.
fn replication_request(source: &str, destination: &str, participant: &str) -> Value {
    json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": participant,
    })
}

/// The destination each occurrence of `name` among `events` names, in occurrence order,
/// paired with its position among all the occurrences.
fn named(events: &[Value], name: &str) -> Vec<(usize, (String, String))> {
    events
        .iter()
        .enumerate()
        .filter(|(_, event)| member(event, "name") == name)
        .map(|(position, event)| {
            (
                position,
                (
                    member(event, "destinationStageArn"),
                    member(event, "destinationSessionId"),
                ),
            )
        })
        .collect()
}

/// The destinations the ends of replications recorded in one session name, in occurrence
/// order, asserting of each end that it happened to the publisher rather than to a replica
/// and that it follows the start that named the same destination.
fn ended_destinations(case: &str, events: &[Value]) -> Vec<(String, String)> {
    let started = named(events, REPLICATION_STARTED);
    let stopped = named(events, REPLICATION_STOPPED);
    for (position, destination) in &stopped {
        let start = started
            .iter()
            .find(|(_, started_destination)| started_destination == destination);
        let (start_position, _) = start.unwrap_or_else(|| {
            panic!(
                "case {case:?}: an end names the destination its start named, \
                 got {events:?}"
            )
        });
        assert!(
            start_position < position,
            "case {case:?}: the end of a replication follows the start it ends, \
             got {events:?}"
        );
    }
    for event in events
        .iter()
        .filter(|event| member(event, "name") == REPLICATION_STOPPED)
    {
        assert!(
            !event["replica"].as_bool().unwrap_or(false),
            "case {case:?}: a replication ends for the publisher of its own stage, not for \
             a replica, got {event}"
        );
    }
    stopped
        .into_iter()
        .map(|(_, destination)| destination)
        .collect()
}

/// How the replication under test ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// A caller stops it.
    Stopped,
    /// The stage it replicates INTO is deleted.
    DestinationDeleted,
    /// The stage its publisher publishes in is deleted.
    SourceDeleted,
}

/// One row of the class: what this store holds, and how the replication ends.
struct EndCase {
    case: &'static str,
    ending: Ending,
    /// How many destination stages the publishers are replicated into. The FIRST is the one
    /// the ending acts on.
    destinations: usize,
    /// How many publishers of the source stage are replicated into the first destination.
    publishers: usize,
    /// Whether a SECOND source stage replicates its own publisher into the first
    /// destination.
    other_source: bool,
    /// Whether the replication of the first publisher into the first destination is stopped
    /// by a caller BEFORE the ending.
    stopped_first: bool,
}

/// The rows this suite spans the class with.
fn end_cases() -> Vec<EndCase> {
    vec![
        EndCase {
            case: "a caller stops the only replication",
            ending: Ending::Stopped,
            destinations: 1,
            publishers: 1,
            other_source: false,
            stopped_first: false,
        },
        EndCase {
            case: "a caller stops one replication of three",
            ending: Ending::Stopped,
            destinations: 3,
            publishers: 1,
            other_source: false,
            stopped_first: false,
        },
        EndCase {
            case: "the only destination stage is deleted",
            ending: Ending::DestinationDeleted,
            destinations: 1,
            publishers: 1,
            other_source: false,
            stopped_first: false,
        },
        EndCase {
            case: "one destination stage of three is deleted",
            ending: Ending::DestinationDeleted,
            destinations: 3,
            publishers: 1,
            other_source: false,
            stopped_first: false,
        },
        EndCase {
            case: "a destination stage holding the replicas of two publishers is deleted",
            ending: Ending::DestinationDeleted,
            destinations: 1,
            publishers: 2,
            other_source: false,
            stopped_first: false,
        },
        EndCase {
            case: "a destination stage another stage also replicates into is deleted",
            ending: Ending::DestinationDeleted,
            destinations: 2,
            publishers: 1,
            other_source: true,
            stopped_first: false,
        },
        EndCase {
            case: "the destination stage of an already stopped replication is deleted",
            ending: Ending::DestinationDeleted,
            destinations: 1,
            publishers: 1,
            other_source: false,
            stopped_first: true,
        },
        EndCase {
            case: "the source stage of the replication is deleted",
            ending: Ending::SourceDeleted,
            destinations: 1,
            publishers: 1,
            other_source: false,
            stopped_first: false,
        },
    ]
}

/// THE RULE: a running replication that ends records exactly one end in the session of the
/// stage its publisher publishes in, naming the destination that replication named, whichever
/// way it ended — and a replication that had already ended records no second one.
#[tokio::test]
async fn ending_a_replication_records_it_in_the_publishers_own_session() {
    for row in end_cases() {
        let case = row.case;
        let emulator = RealTime::new();
        let source = emulator.stage(case, "source").await;

        // The publishers of the source stage, each replicated into every destination stage:
        // the first destination is the one the ending acts on, the others are replications
        // the ending does not name.
        let mut publishers = Vec::new();
        for _ in 0..row.publishers {
            publishers.push(emulator.participant_id(case, &source).await);
        }
        let mut destinations = Vec::new();
        for index in 0..row.destinations {
            let destination = emulator.stage(case, &format!("destination-{index}")).await;
            for publisher in &publishers {
                emulator
                    .replicate(case, &source, &destination, publisher)
                    .await;
            }
            let session = emulator.active_session(case, &destination).await;
            assert!(
                !session.is_empty(),
                "case {case:?}: a replication into {destination} must have given it a \
                 session, got {session:?}"
            );
            destinations.push((destination, session));
        }
        let source_session = emulator.active_session(case, &source).await;

        // A second source stage replicating its own publisher into the ending destination:
        // its own session is a second one the end of a replication is recorded in.
        let other = if row.other_source {
            let other_source = emulator.stage(case, "other-source").await;
            let other_publisher = emulator.participant_id(case, &other_source).await;
            emulator
                .replicate(case, &other_source, &destinations[0].0, &other_publisher)
                .await;
            let other_session = emulator.active_session(case, &other_source).await;
            Some((other_source, other_session, other_publisher))
        } else {
            None
        };

        // A replication a caller already stopped: its end was recorded then, and the ending
        // below must not record it twice.
        if row.stopped_first {
            emulator
                .stop(case, &source, &destinations[0].0, &publishers[0])
                .await;
        }

        match row.ending {
            Ending::Stopped => {
                emulator
                    .stop(case, &source, &destinations[0].0, &publishers[0])
                    .await;
            }
            Ending::DestinationDeleted => {
                emulator.delete_stage(case, &destinations[0].0).await;
            }
            Ending::SourceDeleted => {
                emulator.delete_stage(case, &source).await;
            }
        }

        if row.ending == Ending::SourceDeleted {
            // The end of a replication is an occurrence of its SOURCE session, and that
            // session went with the stage it belonged to: there is no session left for one
            // to be recorded in, and nothing was written into the surviving destination
            // session's replica instead.
            let (status, response) = emulator
                .events_response(&source, &source_session, &publishers[0])
                .await;
            let reported = response["events"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                ended_destinations(case, &reported),
                Vec::<(String, String)>::new(),
                "case {case:?}: the deleted stage holds no session for the end of a \
                 replication to be recorded in, got {status} {response}"
            );
            let replica_events = emulator
                .events(case, &destinations[0].0, &destinations[0].1, &publishers[0])
                .await;
            assert_eq!(
                ended_destinations(case, &replica_events),
                Vec::<(String, String)>::new(),
                "case {case:?}: a replication ends in the session its publisher publishes \
                 in, not in the one its replica joined, got {replica_events:?}"
            );
            continue;
        }

        // Exactly one end, naming the destination stage and the destination session the
        // ended replication named — one end whether the ending stopped it or deleted the
        // stage it replicated into, and one end even where a caller had already stopped it.
        let publisher_events = emulator
            .events(case, &source, &source_session, &publishers[0])
            .await;
        assert_eq!(
            ended_destinations(case, &publisher_events),
            vec![destinations[0].clone()],
            "case {case:?}: ending the replication into {} must record exactly that one \
             end in the publisher's own session, got {publisher_events:?}",
            destinations[0].0
        );

        // Every other publisher replicated into the same destination had its replication
        // ended by the same ending, so each of their sessions reports the same one end;
        // every replication into a destination the ending did not name is still running, so
        // nothing names it.
        for publisher in &publishers[1..] {
            let events = emulator
                .events(case, &source, &source_session, publisher)
                .await;
            let expected = match row.ending {
                Ending::DestinationDeleted => vec![destinations[0].clone()],
                _ => Vec::new(),
            };
            assert_eq!(
                ended_destinations(case, &events),
                expected,
                "case {case:?}: the other publisher's replications ended exactly with the \
                 destination stages that went, got {events:?}"
            );
        }
        if let Some((other_source, other_session, other_publisher)) = &other {
            let events = emulator
                .events(case, other_source, other_session, other_publisher)
                .await;
            let expected = match row.ending {
                Ending::DestinationDeleted => vec![destinations[0].clone()],
                _ => Vec::new(),
            };
            assert_eq!(
                ended_destinations(case, &events),
                expected,
                "case {case:?}: the other source stage's own session reports the end of its \
                 own replication, got {events:?}"
            );
        }
    }
}
