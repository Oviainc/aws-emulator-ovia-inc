// SPDX-License-Identifier: CC0-1.0

//! Stopping a participant replication over the wire: `StopParticipantReplication`, and
//! what the operations that report a replication, its participants and its occurrences
//! answer once it has stopped.
//!
//! Every case drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body — never
//! an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A STOP ENDS EXACTLY THE REPLICATION IT NAMES. The replication the three members name
//!    reports the other value of the modelled `ReplicationState` list and keeps every other
//!    `@required` member of `ParticipantReplica`; every other replication this store holds
//!    is untouched.
//! 2. THE REPLICA LEAVES THE DESTINATION STAGE. The participant the replication put into
//!    the destination stage's session is reported `DISCONNECTED` there, that departure and
//!    the end of its media are occurrences of that session happening to a replica
//!    (`Event$replica`), and the session ends when it held nobody else — while the
//!    publisher stays in the stage it publishes in.
//! 3. THE STOP IS AN OCCURRENCE OF THE PUBLISHER'S SESSION, named `REPLICATION_STOPPED` and
//!    carrying the destination stage and session, the two members the model marks
//!    applicable to exactly the two replication event names.
//! 4. REFUSALS: a stage this store does not hold, and a replication it is not running
//!    (never started, already stopped), are the modelled `ResourceNotFoundException` (404);
//!    one stage named as both ends is the modelled `ValidationException` (400). A refused
//!    stop changes nothing.
//! 5. A STOPPED REPLICATION IS NOT A RUNNING ONE: the publisher can be replicated into that
//!    same stage again, which is a second replica in the state a start leaves it in.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The two values `ReplicationState` accepts, from
/// `smithy_query(query="shape",shape="ReplicationState")`: the state a start leaves a
/// replication in and the one a stop leaves it in.
const ACTIVE: &str = "ACTIVE";
const STOPPED: &str = "STOPPED";

/// The `ReplicationType` values of `smithy_query(query="shape",shape="ReplicationType")`
/// this suite reads: what the publisher of a replication reports, and what its replica in
/// the destination stage does.
const REPLICATION_TYPE_SOURCE: &str = "SOURCE";
const REPLICATION_TYPE_REPLICA: &str = "REPLICA";

/// The two `ParticipantState` values of
/// `smithy_query(query="shape",shape="ParticipantState")`.
const CONNECTED: &str = "CONNECTED";
const DISCONNECTED: &str = "DISCONNECTED";

/// `ReconnectWindowSeconds`' modelled `@range`, from
/// `smithy_query(query="operation",operation="StartParticipantReplication")`: `0..300`.
const RECONNECT_WINDOW_MAX: i64 = 300;

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

    /// A publisher of `stage_arn`, from a token issued for it: the identity the model says a
    /// `participantId` comes from ("assigned by IVS and returned by
    /// `CreateParticipantToken`").
    async fn publisher(&self, case: &str, stage_arn: &str) -> String {
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
    async fn replicate(&self, case: &str, request: &Value) {
        let (status, response) = self
            .call("StartParticipantReplication", request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// Stop one replication, answering the status and body unchanged.
    async fn stop(&self, request: &Value) -> (u16, Value) {
        self.call("StopParticipantReplication", request).await
    }

    /// Stop one replication, asserting the stop was accepted.
    async fn stop_ok(&self, case: &str, request: &Value) {
        let (status, response) = self.stop(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// The replicas this store holds for `participant_id` of `source_stage_arn`.
    async fn replicas(&self, case: &str, source_stage_arn: &str, participant_id: &str) -> Vec<Value> {
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

    /// The one replica of `participant_id` replicated into `destination_stage_arn`.
    async fn replica(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
        destination_stage_arn: &str,
    ) -> Value {
        let replicas = self.replicas(case, source_stage_arn, participant_id).await;
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
        matching[0].clone()
    }

    /// The `activeSessionId` `GetStage` reports for `stage_arn`, empty when the stage has no
    /// session running.
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
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// The stated message of a failed call.
fn error_message(response: &Value) -> &str {
    response["message"].as_str().unwrap_or("<no message>")
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

/// The occurrence names of `events`, in order, each with the `replica` flag it carries.
fn names(events: &[Value]) -> Vec<(String, bool)> {
    events
        .iter()
        .map(|event| {
            (
                member(event, "name"),
                event["replica"].as_bool().unwrap_or(false),
            )
        })
        .collect()
}

/// An IVS stage ARN in `us-east-1` naming the stage id `id`, in the form the model's
/// `StageArn` pattern requires.
fn stage_arn(id: &str) -> String {
    format!("arn:aws:ivs:us-east-1:{ACCOUNT}:stage/{id}")
}

/// The three members every `StopParticipantReplication` request carries.
fn stop_request(source: &str, destination: &str, participant: &str) -> Value {
    json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": participant,
    })
}

/// How this service names the replication a failed stop could not find: the one not-found
/// sentence of this service, over the publisher — named the way every non-ARN identifier
/// of this service is, by its kind with the id quoted — and the two stages a request names
/// a replication by.
fn replication_not_found(participant: &str, source: &str, destination: &str) -> String {
    format!(
        "Resource: participant \"{participant}\" of {source} replicated to \
         {destination} not found"
    )
}

/// One row of the stop class: how the replication was started, and what else this store
/// holds beside it.
struct StopCase {
    case: &'static str,
    /// The members the start request carries beyond the three required ones, so a stop is
    /// shown to be independent of what the start asked for.
    start_extra: Value,
    /// Further destination stages the same publisher is replicated into, none of which the
    /// stop names.
    other_destinations: usize,
    /// Whether a SECOND publisher is replicated into the destination stage the stop names.
    other_publisher: bool,
}

/// The rows this suite spans the stop class with: each differs in what the rule
/// generalises over — what the start request asked for, how many replications of the same
/// publisher exist, and whether another publisher's replica shares the destination stage.
fn stop_cases() -> Vec<StopCase> {
    vec![
        StopCase {
            case: "the smallest start request the model allows",
            start_extra: json!({}),
            other_destinations: 0,
            other_publisher: false,
        },
        StopCase {
            case: "a start that asked for the maximum reconnect window",
            start_extra: json!({ "reconnectWindowSeconds": RECONNECT_WINDOW_MAX }),
            other_destinations: 0,
            other_publisher: false,
        },
        StopCase {
            case: "a start that set attributes on the replica",
            start_extra: json!({ "attributes": { "seat": "left", "role": "guest" } }),
            other_destinations: 0,
            other_publisher: false,
        },
        StopCase {
            case: "one publisher replicated into two stages",
            start_extra: json!({}),
            other_destinations: 1,
            other_publisher: false,
        },
        StopCase {
            case: "one publisher replicated into four stages",
            start_extra: json!({ "reconnectWindowSeconds": 1 }),
            other_destinations: 3,
            other_publisher: false,
        },
        StopCase {
            case: "two publishers replicated into one destination stage",
            start_extra: json!({}),
            other_destinations: 0,
            other_publisher: true,
        },
    ]
}

// ---------------------------------------------------------------------------
// The stop, and what every reader answers after it
// ---------------------------------------------------------------------------

/// RULES 1, 2 and 3: the named replication reports `STOPPED` and keeps everything else it
/// reported, the replica leaves the destination stage while the publisher stays in its own,
/// and the stop is the publisher's `REPLICATION_STOPPED` occurrence naming that destination.
#[tokio::test]
async fn a_stop_ends_exactly_the_replication_it_names() {
    for row in stop_cases() {
        let case = row.case;
        let emulator = RealTime::new();
        let source = emulator.stage(case, "source").await;
        let destination = emulator.stage(case, "destination").await;
        let publisher = emulator.publisher(case, &source).await;

        let mut request = stop_request(&source, &destination, &publisher);
        if let Some(extra) = row.start_extra.as_object() {
            for (key, value) in extra {
                request[key] = value.clone();
            }
        }
        emulator.replicate(case, &request).await;

        // Every further replication of the same publisher, each into its own stage.
        let mut others = Vec::new();
        for index in 0..row.other_destinations {
            let other = emulator.stage(case, &format!("other-{index}")).await;
            emulator
                .replicate(case, &stop_request(&source, &other, &publisher))
                .await;
            others.push(other);
        }
        // A second publisher of the same source stage, replicated into the SAME destination
        // stage the stop names.
        let bystander = if row.other_publisher {
            let bystander = emulator.publisher(case, &source).await;
            emulator
                .replicate(case, &stop_request(&source, &destination, &bystander))
                .await;
            Some(bystander)
        } else {
            None
        };

        // Read before the stop: the sessions the replication names, and what the listing
        // reports for it, so what the stop changed is the difference between two reads.
        let source_session = emulator.active_session(case, &source).await;
        let destination_session = emulator.active_session(case, &destination).await;
        let before = emulator
            .replica(case, &source, &publisher, &destination)
            .await;
        assert_eq!(
            member(&before, "replicationState"),
            ACTIVE,
            "case {case:?}: a started replication must report {ACTIVE}, got {before}"
        );

        emulator
            .stop_ok(case, &stop_request(&source, &destination, &publisher))
            .await;

        // RULE 1: the one replication the request named reports the other state of the
        // modelled list, and every other member of it is the one it reported before.
        let after = emulator
            .replica(case, &source, &publisher, &destination)
            .await;
        assert_eq!(
            member(&after, "replicationState"),
            STOPPED,
            "case {case:?}: the stopped replication must report {STOPPED}, got {after}"
        );
        for unchanged in [
            "sourceStageArn",
            "sourceSessionId",
            "destinationStageArn",
            "destinationSessionId",
            "participantId",
        ] {
            assert_eq!(
                member(&after, unchanged),
                member(&before, unchanged),
                "case {case:?}: a stop must not change {unchanged}, got {after} \
                 (was {before})"
            );
        }

        // RULE 1, second reading: no other replication this store holds was touched.
        for other in &others {
            let untouched = emulator.replica(case, &source, &publisher, other).await;
            assert_eq!(
                member(&untouched, "replicationState"),
                ACTIVE,
                "case {case:?}: the replication into {other} was not stopped, got {untouched}"
            );
        }
        if let Some(bystander) = &bystander {
            let untouched = emulator
                .replica(case, &source, bystander, &destination)
                .await;
            assert_eq!(
                member(&untouched, "replicationState"),
                ACTIVE,
                "case {case:?}: the replication of {bystander} was not stopped, \
                 got {untouched}"
            );
        }

        // RULE 2: the replica is out of the destination stage's session, still reported as
        // one of that session's participants and still naming where it was replicated from.
        let replica = emulator
            .participant(case, &destination, &destination_session, &publisher)
            .await;
        assert_eq!(
            member(&replica, "state"),
            DISCONNECTED,
            "case {case:?}: the replica must have left the destination stage, got {replica}"
        );
        assert_eq!(
            member(&replica, "replicationType"),
            REPLICATION_TYPE_REPLICA,
            "case {case:?}: the record in the destination stage is a replica, got {replica}"
        );
        assert_eq!(
            member(&replica, "replicationState"),
            STOPPED,
            "case {case:?}: the replica must report its replication {STOPPED}, got {replica}"
        );
        assert_eq!(
            member(&replica, "sourceStageArn"),
            source,
            "case {case:?}: a replica names the stage it is replicated from, got {replica}"
        );

        // RULE 2, second reading: the publisher is in the stage it publishes in, whether or
        // not anything replicates it any more.
        let published = emulator
            .participant(case, &source, &source_session, &publisher)
            .await;
        assert_eq!(
            member(&published, "state"),
            CONNECTED,
            "case {case:?}: stopping a replication must not disconnect the publisher, \
             got {published}"
        );
        assert_eq!(
            member(&published, "replicationType"),
            REPLICATION_TYPE_SOURCE,
            "case {case:?}: the publisher is still what the replication was of, \
             got {published}"
        );
        // A publisher is replicating while ANY of its replications is: with another one
        // still running it reports that one, and with none it reports the stopped state.
        let expected = if others.is_empty() { STOPPED } else { ACTIVE };
        assert_eq!(
            member(&published, "replicationState"),
            expected,
            "case {case:?}: the publisher must report {expected} with \
             {} replication(s) still running, got {published}",
            others.len()
        );

        // RULE 2, third reading: a session "ends after the last participant leaves", so the
        // destination stage holds a session exactly while a replica is still in it.
        let still_active = emulator.active_session(case, &destination).await;
        if bystander.is_some() {
            assert_eq!(
                still_active, destination_session,
                "case {case:?}: the destination stage still holds another replica, so its \
                 session must be the same one, got {still_active:?}"
            );
        } else {
            assert_eq!(
                still_active, "",
                "case {case:?}: the last replica of the destination stage has left, so the \
                 stage must report no session, got {still_active:?}"
            );
        }

        // RULE 2, fourth reading: the replica's departure and the end of its media are
        // occurrences of the destination session, and both happened to a replica.
        let replica_events = emulator
            .events(case, &destination, &destination_session, &publisher)
            .await;
        assert_eq!(
            names(&replica_events),
            vec![
                ("JOINED".to_string(), true),
                ("PUBLISH_STARTED".to_string(), true),
                ("PUBLISH_STOPPED".to_string(), true),
                ("LEFT".to_string(), true),
            ],
            "case {case:?}: the replica's occurrences must end with its media stopping and \
             its departure, got {replica_events:?}"
        );

        // RULE 3: the stop is the publisher's own occurrence in its own session, and it
        // names the destination the stopped replication named.
        let publisher_events = emulator
            .events(case, &source, &source_session, &publisher)
            .await;
        let stopped: Vec<&Value> = publisher_events
            .iter()
            .filter(|event| member(event, "name") == "REPLICATION_STOPPED")
            .collect();
        assert_eq!(
            stopped.len(),
            1,
            "case {case:?}: one stop is one occurrence, got {publisher_events:?}"
        );
        let stop_event = stopped[0];
        assert_eq!(
            member(stop_event, "destinationStageArn"),
            destination,
            "case {case:?}: the stop must name the stage the replication ended into, \
             got {stop_event}"
        );
        assert_eq!(
            member(stop_event, "destinationSessionId"),
            member(&before, "destinationSessionId"),
            "case {case:?}: the stop must name the destination session the replication \
             named, got {stop_event}"
        );
        assert!(
            !stop_event["replica"].as_bool().unwrap_or(false),
            "case {case:?}: the stop happened to the publisher of its own stage, not to a \
             replica, got {stop_event}"
        );
        // It is the counterpart of the start, so it follows it: `ListParticipantEvents`
        // reports one session's occurrences in order.
        let ordered: Vec<String> = publisher_events
            .iter()
            .map(|event| member(event, "name"))
            .filter(|name| name.starts_with("REPLICATION_"))
            .collect();
        let mut expected_order: Vec<String> =
            vec!["REPLICATION_STARTED".to_string(); 1 + row.other_destinations];
        expected_order.push("REPLICATION_STOPPED".to_string());
        assert_eq!(
            ordered, expected_order,
            "case {case:?}: a stop is recorded after the starts it ends, \
             got {publisher_events:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the refusal class: the request, and the failure it must answer.
struct RefusalCase {
    case: String,
    request: Value,
    status: u16,
    code: &'static str,
    message: String,
}

/// RULE 4: a stop this store cannot serve is refused with the modelled failure that
/// condition is, and persists nothing — over every way a request can name no running
/// replication.
#[tokio::test]
async fn a_stop_that_names_no_running_replication_is_refused() {
    let case = "refusals";
    let emulator = RealTime::new();
    let source = emulator.stage(case, "source").await;
    let destination = emulator.stage(case, "destination").await;
    let elsewhere = emulator.stage(case, "elsewhere").await;
    let publisher = emulator.publisher(case, &source).await;
    let unreplicated = emulator.publisher(case, &source).await;
    let absent = stage_arn("neverCreated");
    emulator
        .replicate(case, &stop_request(&source, &destination, &publisher))
        .await;

    let cases = vec![
        RefusalCase {
            case: "a source stage this store does not hold".to_string(),
            request: stop_request(&absent, &destination, &publisher),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Resource: {absent} not found"),
        },
        RefusalCase {
            case: "a destination stage this store does not hold".to_string(),
            request: stop_request(&source, &absent, &publisher),
            status: 404,
            code: "ResourceNotFoundException",
            message: format!("Resource: {absent} not found"),
        },
        RefusalCase {
            case: "a publisher of the source stage that is not replicated".to_string(),
            request: stop_request(&source, &destination, &unreplicated),
            status: 404,
            code: "ResourceNotFoundException",
            message: replication_not_found(&unreplicated, &source, &destination),
        },
        RefusalCase {
            case: "a participant id no identity of this service carries".to_string(),
            request: stop_request(&source, &destination, "neverIssued"),
            status: 404,
            code: "ResourceNotFoundException",
            message: replication_not_found("neverIssued", &source, &destination),
        },
        RefusalCase {
            case: "a destination stage this publisher is not replicated into".to_string(),
            request: stop_request(&source, &elsewhere, &publisher),
            status: 404,
            code: "ResourceNotFoundException",
            message: replication_not_found(&publisher, &source, &elsewhere),
        },
        RefusalCase {
            case: "the two stages of the replication swapped".to_string(),
            request: stop_request(&destination, &source, &publisher),
            status: 404,
            code: "ResourceNotFoundException",
            message: replication_not_found(&publisher, &destination, &source),
        },
        RefusalCase {
            case: "one stage named as both ends of the replication".to_string(),
            request: stop_request(&source, &source, &publisher),
            status: 400,
            code: "ValidationException",
            message: format!(
                "StopParticipantReplication.destinationStageArn is required to name a stage \
                 other than sourceStageArn, found {source}"
            ),
        },
    ];

    for row in &cases {
        let (status, response) = emulator.stop(&row.request).await;
        assert_eq!(
            status, row.status,
            "case {:?}: {} must answer HTTP {}, got {status} {response}",
            row.case, row.request, row.status
        );
        assert_eq!(
            error_code(&response),
            row.code,
            "case {:?}: expected the modelled {}, got {response}",
            row.case,
            row.code
        );
        assert_eq!(
            error_message(&response),
            row.message,
            "case {:?}: expected the stated refusal, got {response}",
            row.case
        );
    }

    // Nothing above was persisted: the replication is still running and its replica is
    // still in the destination stage.
    let untouched = emulator
        .replica(case, &source, &publisher, &destination)
        .await;
    assert_eq!(
        member(&untouched, "replicationState"),
        ACTIVE,
        "a refused stop must leave the replication running, got {untouched}"
    );
    let destination_session = emulator.active_session(case, &destination).await;
    let replica = emulator
        .participant(case, &destination, &destination_session, &publisher)
        .await;
    assert_eq!(
        member(&replica, "state"),
        CONNECTED,
        "a refused stop must leave the replica in the destination stage, got {replica}"
    );

    // And the SAME request, once it has been served, is refused the second time: a stopped
    // replication is no replicated participant session to stop.
    let repeated = "a replication that has already been stopped";
    let request = stop_request(&source, &destination, &publisher);
    emulator.stop_ok(case, &request).await;
    let (status, response) = emulator.stop(&request).await;
    assert_eq!(
        status, 404,
        "case {repeated:?}: {request} must answer HTTP 404, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case {repeated:?}: expected the modelled ResourceNotFoundException, got {response}"
    );
    assert_eq!(
        error_message(&response),
        replication_not_found(&publisher, &source, &destination),
        "case {repeated:?}: expected the stated refusal, got {response}"
    );
    // The second stop changed nothing either: one stop, one stopped replication.
    let replicas = emulator.replicas(case, &source, &publisher).await;
    let states: Vec<String> = replicas
        .iter()
        .map(|replica| member(replica, "replicationState"))
        .collect();
    assert_eq!(
        states,
        vec![STOPPED.to_string()],
        "a repeated stop must leave exactly the one stopped replication, got {replicas:?}"
    );
}

// ---------------------------------------------------------------------------
// A stopped replication is not a running one
// ---------------------------------------------------------------------------

/// RULE 5: the conflict a start answers is about a replication that is RUNNING, so a
/// publisher whose replication into a stage has stopped can be replicated into it again —
/// a second replica, in the state a start leaves one in, with its own replica back in the
/// destination stage.
#[tokio::test]
async fn a_publisher_can_be_replicated_again_once_its_replication_stopped() {
    let case = "restarted after a stop";
    let emulator = RealTime::new();
    let source = emulator.stage(case, "source").await;
    let destination = emulator.stage(case, "destination").await;
    let publisher = emulator.publisher(case, &source).await;
    let request = stop_request(&source, &destination, &publisher);

    emulator.replicate(case, &request).await;
    let first_session = emulator.active_session(case, &destination).await;
    emulator.stop_ok(case, &request).await;
    emulator.replicate(case, &request).await;

    // Two replicas of one publisher into one stage: the stopped one and the running one,
    // in start order.
    let replicas = emulator.replicas(case, &source, &publisher).await;
    let states: Vec<String> = replicas
        .iter()
        .map(|replica| member(replica, "replicationState"))
        .collect();
    assert_eq!(
        states,
        vec![STOPPED.to_string(), ACTIVE.to_string()],
        "case {case:?}: a restarted replication is a second replica beside the stopped one, \
         got {replicas:?}"
    );

    // The destination stage's session ended when the replica left, so the new replication
    // names the session the new replica joined — not the one the stopped replication named.
    let second_session = emulator.active_session(case, &destination).await;
    assert_ne!(
        second_session, first_session,
        "case {case:?}: the destination stage opened a new session for the new replica, \
         got {second_session:?}"
    );
    assert_eq!(
        member(&replicas[1], "destinationSessionId"),
        second_session,
        "case {case:?}: the running replication must name the destination stage's session, \
         got {replicas:?}"
    );
    assert_eq!(
        member(&replicas[0], "destinationSessionId"),
        first_session,
        "case {case:?}: the stopped replication keeps the session it named, got {replicas:?}"
    );

    let replica = emulator
        .participant(case, &destination, &second_session, &publisher)
        .await;
    assert_eq!(
        member(&replica, "state"),
        CONNECTED,
        "case {case:?}: the new replica is in the destination stage, got {replica}"
    );
    assert_eq!(
        member(&replica, "replicationState"),
        ACTIVE,
        "case {case:?}: the new replica reports the running replication, got {replica}"
    );

    // One start, one stop, one start: the publisher's own session holds exactly those three
    // occurrences of replication, in that order.
    let source_session = emulator.active_session(case, &source).await;
    let events = emulator
        .events(case, &source, &source_session, &publisher)
        .await;
    let replication: Vec<String> = events
        .iter()
        .map(|event| member(event, "name"))
        .filter(|name| name.starts_with("REPLICATION_"))
        .collect();
    assert_eq!(
        replication,
        vec![
            "REPLICATION_STARTED".to_string(),
            "REPLICATION_STOPPED".to_string(),
            "REPLICATION_STARTED".to_string(),
        ],
        "case {case:?}: each start and stop is its own occurrence, in order, got {events:?}"
    );
}
