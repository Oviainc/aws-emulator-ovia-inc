// SPDX-License-Identifier: CC0-1.0

//! The participants a started replication puts into the two sessions it names, end to
//! end: what `ListParticipants` reports for the publisher being replicated and for its
//! replica in the destination stage.
//!
//! Every case goes through the router's own `dispatch` with restJson1 requests, exactly as
//! signed `aws ivs-realtime` calls arrive, so what is asserted is the emulator's HTTP
//! behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs: a replication
//! makes the replicated publisher a participant of BOTH sessions it names — the session of
//! the destination stage holds a participant that is a replica, reporting the stage and
//! session it is replicated from and the replication's state, and the session the
//! publisher publishes in reports that publisher as a replication source in that same
//! state, whether read as a `ListParticipants` page or through `GetParticipant` — and each
//! of those four members is read off the replication the store holds, so a participant no
//! replication names reports the default (`NONE`, with the other three members absent) and
//! one whose replication is gone reports it again.
//!
//! The rows differ in what the rule generalises over: how the publisher's identity was
//! issued (a token of the stage, a token `CreateStage` minted, the credential of an ingest
//! configuration), whether it carries a `userId`, what its own session already holds for it
//! (nothing, a record that has published, one that has not, or one that has left), how many
//! destination stages it is replicated into, and what else the start request asks for.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;

/// The three values the modelled `ReplicationType` carries, from
/// `smithy_query(query="shape",shape="ReplicationType")`: what a participant reports when
/// it is replicated into another stage, when it IS such a replica, and when no replication
/// names it ("Default: `NONE`", as `ParticipantSummary$replicationType` documents).
const REPLICATION_TYPE_SOURCE: &str = "SOURCE";
const REPLICATION_TYPE_REPLICA: &str = "REPLICA";
const REPLICATION_TYPE_NONE: &str = "NONE";

/// The state of a replication that has been started and not stopped, from
/// `smithy_query(query="shape",shape="ReplicationState")` (`ACTIVE` | `STOPPED`) and the
/// `StartParticipantReplicationRequest$reconnectWindowSeconds` doc ("replication will
/// continue to be `ACTIVE`").
const STARTED_REPLICATION_STATE: &str = "ACTIVE";

/// `ParticipantState` of a participant that is in the stage, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const CONNECTED: &str = "CONNECTED";

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
        router.register_service(
            service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
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

    /// Create a stage and return the ARN the service minted for it. The name is the
    /// case's label with every character outside the modelled `StageName` pattern
    /// (`^[a-zA-Z0-9-_]*$`) replaced.
    async fn create_stage(&self, name: &str) -> String {
        let name = stage_name(name);
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(status, 200, "CreateStage {name}: got {status} {response}");
        text(&response["stage"], "arn")
    }

    /// Create a stage whose own `participantTokenConfigurations` mint a publisher, and
    /// return both the stage ARN and that participant's id.
    async fn create_stage_with_token(&self, name: &str, user_id: Option<&str>) -> (String, String) {
        let mut configuration = json!({});
        if let Some(user_id) = user_id {
            configuration["userId"] = json!(user_id);
        }
        let request = json!({
            "name": stage_name(name),
            "participantTokenConfigurations": [configuration],
        });
        let (status, response) = self.call("CreateStage", &request).await;
        assert_eq!(status, 200, "CreateStage {request}: got {status} {response}");
        (
            text(&response["stage"], "arn"),
            text(&response["participantTokens"][0], "participantId"),
        )
    }

    /// Issue a participant token for `stage_arn` and return the participant id it carries
    /// — the id the holder of that token joins the stage under.
    async fn create_token(&self, stage_arn: &str, user_id: Option<&str>) -> String {
        let mut request = json!({ "stageArn": stage_arn });
        if let Some(user_id) = user_id {
            request["userId"] = json!(user_id);
        }
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "CreateParticipantToken {request}: got {status} {response}"
        );
        text(&response["participantToken"], "participantId")
    }

    /// Associate an ingest configuration with `stage_arn` and return the participant its
    /// ingest credential publishes as.
    async fn create_ingest_participant(&self, stage_arn: &str, user_id: Option<&str>) -> String {
        let mut request = json!({ "ingestProtocol": "RTMPS", "stageArn": stage_arn });
        if let Some(user_id) = user_id {
            request["userId"] = json!(user_id);
        }
        let (status, response) = self.call("CreateIngestConfiguration", &request).await;
        assert_eq!(
            status, 200,
            "CreateIngestConfiguration {request}: got {status} {response}"
        );
        text(&response["ingestConfiguration"], "participantId")
    }

    /// Start one replication, asserting it was accepted.
    async fn replicate(&self, case: &str, request: &Value) {
        let (status, response) = self.call("StartParticipantReplication", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// The stage's `activeSessionId`, read back over the wire.
    async fn active_session_id(&self, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(status, 200, "GetStage {stage_arn}: got {status} {response}");
        text(&response["stage"], "activeSessionId")
    }

    /// `ListParticipants` for one session, with `extra` merged into the request.
    async fn participants(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        extra: &Value,
    ) -> Value {
        let mut request = json!({ "stageArn": stage_arn, "sessionId": session_id });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        let (status, response) = self.call("ListParticipants", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipants {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response
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

    /// The occurrence names `ListParticipantEvents` reports for one participant of one
    /// session, with the `replica` flag each carries.
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
                    text(event, "name"),
                    event["replica"].as_bool().unwrap_or(false),
                )
            })
            .collect()
    }

    /// Delete a stage, asserting the delete was accepted.
    async fn delete_stage(&self, case: &str, stage_arn: &str) {
        let (status, response) = self.call("DeleteStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStage {stage_arn} must answer HTTP 200, got {status} {response}"
        );
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does —
    /// nothing in this control plane makes a participant join.
    fn join(
        &self,
        case: &str,
        stage_arn: &str,
        participant_id: &str,
        user_id: Option<&str>,
        published: bool,
    ) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(
                &mut store,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.to_string(),
                    user_id: user_id.map(str::to_string),
                    published,
                    attributes: HashMap::new(),
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
            "case {case:?}: the departure of {participant_id:?} from {session_id:?} must be \
             recorded"
        );
    }
}

/// A string member, read as the empty string when the wire omits it.
fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A case's label as a stage name the model accepts: every character outside the modelled
/// `StageName` pattern (`^[a-zA-Z0-9-_]*$`) replaced, so a case may name itself freely.
fn stage_name(label: &str) -> String {
    label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// The participant of `listed` whose `participantId` is `participant_id`.
fn reported<'a>(case: &str, listed: &'a Value, participant_id: &str) -> &'a Value {
    listed["participants"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: participants must be a list, got {listed}"))
        .iter()
        .find(|summary| text(summary, "participantId") == participant_id)
        .unwrap_or_else(|| {
            panic!("case {case:?}: {participant_id} must be a participant of the session, got {listed}")
        })
}

/// The participant ids `listed` reported, in the order it reported them.
fn reported_ids(case: &str, listed: &Value) -> Vec<String> {
    listed["participants"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: participants must be a list, got {listed}"))
        .iter()
        .map(|summary| text(summary, "participantId"))
        .collect()
}

/// Assert the four replication members of `summary`, and that it is a connected publisher:
/// `replication_type` with the replication state, and the source stage and session a
/// replica is replicated from (both absent for anything that is not a `REPLICA`, which is
/// what their docs say: "if `replicationType` is `REPLICA`").
fn assert_replication(
    case: &str,
    what: &str,
    summary: &Value,
    replication_type: &str,
    replication_state: Option<&str>,
    source: Option<(&str, &str)>,
) {
    assert_eq!(
        text(summary, "replicationType"),
        replication_type,
        "case {case:?}: {what} must report replicationType {replication_type}, got {summary}"
    );
    assert_eq!(
        summary["replicationState"],
        replication_state.map_or(Value::Null, |state| json!(state)),
        "case {case:?}: {what} must report replicationState {replication_state:?}, \
         got {summary}"
    );
    assert_eq!(
        summary["sourceStageArn"],
        source.map_or(Value::Null, |(stage_arn, _)| json!(stage_arn)),
        "case {case:?}: {what} must report the stage it is replicated from \
         ({source:?}), got {summary}"
    );
    assert_eq!(
        summary["sourceSessionId"],
        source.map_or(Value::Null, |(_, session_id)| json!(session_id)),
        "case {case:?}: {what} must report the session it is replicated from \
         ({source:?}), got {summary}"
    );
}

/// How the publisher's identity is issued — the three ways this service hands out a
/// participant id for a stage.
#[derive(Debug, Clone, Copy)]
enum Identity {
    /// `CreateParticipantToken` against an existing stage.
    Token,
    /// A `participantTokenConfigurations` entry of `CreateStage`.
    StageToken,
    /// The ingest credential of an ingest configuration associated with the stage.
    IngestCredential,
}

/// One row of the replication class: how the publisher got its id, whether it carries a
/// `userId`, whether it was already in its session, how many stages it is replicated into,
/// and what else the start request asks for.
struct Row {
    case: &'static str,
    identity: Identity,
    user_id: Option<&'static str>,
    presence: Presence,
    destinations: usize,
    extra: Value,
}

/// What the publisher's own session holds for it before the replication starts.
#[derive(Debug, Clone, Copy)]
enum Presence {
    /// Nothing: the replication is the first thing that puts it in a session.
    Absent,
    /// A record from a join over the stage's data plane, publishing or not yet.
    Joined { published: bool },
    /// A record that joined and left again — it stays in the session, disconnected.
    Left,
}

/// Bring the publisher's own session to `presence` before the replication starts, the way
/// the stage's data plane would.
async fn seed_presence(
    emulator: &Emulator,
    case: &str,
    source: &str,
    participant: &str,
    user_id: Option<&str>,
    presence: Presence,
) {
    match presence {
        Presence::Absent => {}
        Presence::Joined { published } => {
            emulator.join(case, source, participant, user_id, published);
        }
        Presence::Left => {
            // One other participant keeps the session open after this one leaves: a session
            // "ends after the last participant leaves the stage".
            let keeper = emulator.create_token(source, Some("keeper")).await;
            emulator.join(case, source, &keeper, Some("keeper"), false);
            emulator.join(case, source, participant, user_id, false);
            let session = emulator.active_session_id(source).await;
            emulator.leave(case, source, &session, participant);
        }
    }
}

/// The rows this suite spans the class with.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a token of the stage, one destination",
            identity: Identity::Token,
            user_id: Some("host"),
            presence: Presence::Absent,
            destinations: 1,
            extra: json!({}),
        },
        Row {
            case: "a token carrying no userId, with attributes for the replica",
            identity: Identity::Token,
            user_id: None,
            presence: Presence::Absent,
            destinations: 1,
            extra: json!({ "attributes": { "seat": "left" } }),
        },
        Row {
            case: "a publisher already recorded in its own session",
            identity: Identity::Token,
            user_id: Some("live"),
            presence: Presence::Joined { published: true },
            destinations: 1,
            extra: json!({ "reconnectWindowSeconds": 30 }),
        },
        Row {
            case: "a participant in its own session that has not published yet",
            identity: Identity::Token,
            user_id: Some("quiet"),
            presence: Presence::Joined { published: false },
            destinations: 1,
            extra: json!({}),
        },
        Row {
            case: "a participant whose record has left its own session",
            identity: Identity::Token,
            user_id: Some("returning"),
            presence: Presence::Left,
            destinations: 1,
            extra: json!({}),
        },
        Row {
            case: "a token minted by CreateStage",
            identity: Identity::StageToken,
            user_id: Some("guest"),
            presence: Presence::Absent,
            destinations: 1,
            extra: json!({}),
        },
        Row {
            case: "the credential of an ingest configuration",
            identity: Identity::IngestCredential,
            user_id: Some("encoder"),
            presence: Presence::Absent,
            destinations: 1,
            extra: json!({}),
        },
        Row {
            case: "one publisher replicated into three destinations",
            identity: Identity::Token,
            user_id: Some("multi"),
            presence: Presence::Absent,
            destinations: 3,
            extra: json!({ "reconnectWindowSeconds": 300 }),
        },
    ]
}

/// The source stage and the publisher of one row.
async fn publisher(emulator: &Emulator, row: &Row) -> (String, String) {
    match row.identity {
        Identity::Token => {
            let stage_arn = emulator.create_stage(row.case).await;
            let participant_id = emulator.create_token(&stage_arn, row.user_id).await;
            (stage_arn, participant_id)
        }
        Identity::StageToken => emulator.create_stage_with_token(row.case, row.user_id).await,
        Identity::IngestCredential => {
            let stage_arn = emulator.create_stage(row.case).await;
            let participant_id = emulator
                .create_ingest_participant(&stage_arn, row.user_id)
                .await;
            (stage_arn, participant_id)
        }
    }
}

// ---------------------------------------------------------------------------
// What the two sessions of a replication report
// ---------------------------------------------------------------------------

/// For every row of the class: the destination session holds the publisher as a `REPLICA`
/// naming the source stage and session, and the source session holds it as a `SOURCE`,
/// both in the started replication's state.
#[tokio::test]
async fn a_started_replication_is_a_participant_of_both_sessions_it_names() {
    let emulator = Emulator::new();

    for row in rows() {
        let case = row.case;
        let (source, participant) = publisher(&emulator, &row).await;
        seed_presence(
            &emulator,
            case,
            &source,
            &participant,
            row.user_id,
            row.presence,
        )
        .await;

        let mut destinations = Vec::new();
        for index in 0..row.destinations {
            let destination = emulator
                .create_stage(&format!("{case}-destination-{index}"))
                .await;
            let mut request = json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": participant,
            });
            if let Some(members) = row.extra.as_object() {
                for (member, value) in members {
                    request[member] = value.clone();
                }
            }
            emulator.replicate(case, &request).await;
            destinations.push(destination);
        }

        // The session the publisher publishes in: the stage now reports one, and it holds
        // the publisher as the source of the replication.
        let source_session = emulator.active_session_id(&source).await;
        assert!(
            !source_session.is_empty(),
            "case {case:?}: a started replication puts a participant in the source stage, \
             so it has an active session"
        );
        let listed = emulator
            .participants(case, &source, &source_session, &json!({}))
            .await;
        let publisher_summary = reported(case, &listed, &participant);
        assert_replication(
            case,
            "the replicated publisher",
            publisher_summary,
            REPLICATION_TYPE_SOURCE,
            Some(STARTED_REPLICATION_STATE),
            None,
        );
        assert_eq!(
            text(publisher_summary, "state"),
            CONNECTED,
            "case {case:?}: the replicated publisher is in the stage, got {publisher_summary}"
        );
        assert_eq!(
            publisher_summary["published"],
            json!(true),
            "case {case:?}: only a publishing participant is replicated, so the publisher \
             has published, got {publisher_summary}"
        );
        assert_eq!(
            publisher_summary["userId"],
            row.user_id.map_or(Value::Null, |user_id| json!(user_id)),
            "case {case:?}: the publisher reports the name its identity carries, \
             got {publisher_summary}"
        );

        // A publisher is selectable by the filter that keeps the participants that
        // published.
        let published_only = emulator
            .participants(
                case,
                &source,
                &source_session,
                &json!({ "filterByPublished": true }),
            )
            .await;
        assert!(
            reported_ids(case, &published_only).contains(&participant),
            "case {case:?}: filterByPublished must select the replicated publisher, \
             got {published_only}"
        );

        // Read back through `GetParticipant`: a page and a read of one participant report
        // the same replication.
        assert_replication(
            case,
            "the replicated publisher, read back",
            &emulator
                .participant(case, &source, &source_session, &participant)
                .await,
            REPLICATION_TYPE_SOURCE,
            Some(STARTED_REPLICATION_STATE),
            None,
        );

        // Each destination session: one participant, and it is the replica.
        for destination in &destinations {
            let destination_session = emulator.active_session_id(destination).await;
            let listed = emulator
                .participants(case, destination, &destination_session, &json!({}))
                .await;
            assert_eq!(
                reported_ids(case, &listed),
                vec![participant.clone()],
                "case {case:?}: the destination session holds exactly the replica, \
                 got {listed}"
            );
            let replica = reported(case, &listed, &participant);
            assert_replication(
                case,
                "the replica in the destination stage",
                replica,
                REPLICATION_TYPE_REPLICA,
                Some(STARTED_REPLICATION_STATE),
                Some((&source, &source_session)),
            );
            assert_replication(
                case,
                "the replica in the destination stage, read back",
                &emulator
                    .participant(case, destination, &destination_session, &participant)
                    .await,
                REPLICATION_TYPE_REPLICA,
                Some(STARTED_REPLICATION_STATE),
                Some((&source, &source_session)),
            );
            assert_eq!(
                text(replica, "state"),
                CONNECTED,
                "case {case:?}: a replica is in the destination stage, got {listed}"
            );
            assert_eq!(
                replica["published"],
                json!(true),
                "case {case:?}: a replica carries the publisher's media into the \
                 destination stage, got {listed}"
            );
            assert_eq!(
                replica["userId"],
                row.user_id.map_or(Value::Null, |user_id| json!(user_id)),
                "case {case:?}: the replica reports the name the publisher's identity \
                 carries, got {listed}"
            );
        }

        // Reading is a read: the second answer is the first.
        let again = emulator
            .participants(case, &source, &source_session, &json!({}))
            .await;
        assert_eq!(
            again, listed,
            "case {case:?}: reading the participants twice must answer identically, \
             got {listed} then {again}"
        );
    }
}

/// A participant no replication names reports the model's default, beside one that is
/// replicated: the replication members describe a replication, not the session.
#[tokio::test]
async fn a_participant_no_replication_names_reports_the_default() {
    let emulator = Emulator::new();
    let case = "a replicated publisher beside participants no replication names";
    let source = emulator.create_stage(case).await;
    let destination = emulator.create_stage("destination").await;
    let replicated = emulator.create_token(&source, Some("replicated")).await;
    let other_publisher = emulator.create_token(&source, Some("publisher")).await;
    let subscriber = emulator.create_token(&source, Some("subscriber")).await;

    emulator.join(case, &source, &other_publisher, Some("publisher"), true);
    emulator.join(case, &source, &subscriber, Some("subscriber"), false);
    emulator
        .replicate(
            case,
            &json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": replicated,
            }),
        )
        .await;

    let session = emulator.active_session_id(&source).await;
    let listed = emulator
        .participants(case, &source, &session, &json!({}))
        .await;
    assert_replication(
        case,
        "the replicated publisher",
        reported(case, &listed, &replicated),
        REPLICATION_TYPE_SOURCE,
        Some(STARTED_REPLICATION_STATE),
        None,
    );
    for unreplicated in [&other_publisher, &subscriber] {
        assert_replication(
            case,
            "a participant no replication names",
            reported(case, &listed, unreplicated),
            REPLICATION_TYPE_NONE,
            None,
            None,
        );
    }
}

/// The replication members report the replications the store HOLDS: when a replication is
/// gone — its destination stage deleted, which ends exactly that replication — the
/// publisher stops reporting it, and a publisher with another replication left keeps
/// reporting that one.
#[tokio::test]
async fn a_participant_reports_only_the_replications_the_store_holds() {
    for (case, destinations) in [
        ("the one replication is ended", 1),
        ("one replication of two is ended", 2),
    ] {
        let emulator = Emulator::new();
        let source = emulator.create_stage(case).await;
        let participant = emulator.create_token(&source, Some("host")).await;
        let mut stages = Vec::new();
        for index in 0..destinations {
            let destination = emulator
                .create_stage(&format!("{case}-destination-{index}"))
                .await;
            emulator
                .replicate(
                    case,
                    &json!({
                        "sourceStageArn": source,
                        "destinationStageArn": destination,
                        "participantId": participant,
                    }),
                )
                .await;
            stages.push(destination);
        }

        let session = emulator.active_session_id(&source).await;
        emulator.delete_stage(case, &stages[0]).await;

        let listed = emulator
            .participants(case, &source, &session, &json!({}))
            .await;
        // The participant record itself is untouched by the delete: the publisher is still
        // in its own stage.
        let summary = reported(case, &listed, &participant);
        let (replication_type, replication_state) = if destinations > 1 {
            (REPLICATION_TYPE_SOURCE, Some(STARTED_REPLICATION_STATE))
        } else {
            (REPLICATION_TYPE_NONE, None)
        };
        assert_replication(
            case,
            "the publisher after a replication ended",
            summary,
            replication_type,
            replication_state,
            None,
        );
    }
}

/// The occurrences of each session say who they happened to: the replica's join and
/// publish in the destination stage are a replica's (`Event$replica`), while the
/// publisher's own occurrences in its stage are not, and the replication start is reported
/// there with the destination it names. Over each way the publisher's own session can hold
/// it — recorded by the replication itself, already in the session and not yet publishing
/// (where the media this replication carries beginning is its own occurrence), or holding a
/// record that has left (where the publisher is joining again).
#[tokio::test]
async fn the_occurrences_of_a_replica_name_a_replica() {
    let publisher_occurrences = ["JOINED", "PUBLISH_STARTED", "REPLICATION_STARTED"];
    let cases = [
        (
            "the replication records the publisher",
            Presence::Absent,
            publisher_occurrences.to_vec(),
        ),
        (
            "the publisher is already in its session",
            Presence::Joined { published: false },
            publisher_occurrences.to_vec(),
        ),
        (
            "the publisher's record has left its session",
            Presence::Left,
            // The earlier join and departure, then the same three.
            vec![
                "JOINED",
                "LEFT",
                "JOINED",
                "PUBLISH_STARTED",
                "REPLICATION_STARTED",
            ],
        ),
    ];

    for (case, presence, expected) in cases {
        let emulator = Emulator::new();
        let source = emulator.create_stage(case).await;
        let destination = emulator.create_stage("destination").await;
        let participant = emulator.create_token(&source, Some("host")).await;
        seed_presence(
            &emulator,
            case,
            &source,
            &participant,
            Some("host"),
            presence,
        )
        .await;
        emulator
            .replicate(
                case,
                &json!({
                    "sourceStageArn": source,
                    "destinationStageArn": destination,
                    "participantId": participant,
                }),
            )
            .await;

        let source_session = emulator.active_session_id(&source).await;
        let destination_session = emulator.active_session_id(&destination).await;

        let expected: Vec<(String, bool)> = expected
            .into_iter()
            // Every occurrence of the publisher in its OWN stage happened to a participant
            // of that stage, not to a replica.
            .map(|name| (name.to_string(), false))
            .collect();
        assert_eq!(
            emulator
                .occurrences(case, &source, &source_session, &participant)
                .await,
            expected,
            "case {case:?}: the publisher's occurrences are a participant of its own \
             stage's"
        );
        assert_eq!(
            emulator
                .occurrences(case, &destination, &destination_session, &participant)
                .await,
            vec![
                ("JOINED".to_string(), true),
                ("PUBLISH_STARTED".to_string(), true),
            ],
            "case {case:?}: the replica's occurrences in the destination stage are a \
             replica's"
        );
    }
}
