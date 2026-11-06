// SPDX-License-Identifier: CC0-1.0

//! Participant replication over the wire: `StartParticipantReplication`, and the
//! `ListParticipantReplicas` that reports what it persisted.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. ROUND TRIP. `StartParticipantReplication` persists one replica of the named
//!    publisher in the named destination stage, and `ListParticipantReplicas` reports
//!    that replica for the source stage and participant — every `@required` member of
//!    `ParticipantReplica`, on every read.
//! 2. A REPLICA NAMES TWO SESSIONS: the session of the source stage its publisher is in
//!    and the session of the destination stage it joined, each in the modelled
//!    `StageSessionId` form, each the stage's own `activeSessionId` as `GetStage` reports
//!    it, and the same value for every replica of one stage.
//! 3. PAGINATION is over the replicas of one participant in start order: `maxResults`
//!    sizes a page, `nextToken` resumes after the one it ends at, and a token this
//!    listing did not issue is refused.
//! 4. A READ CHANGES NOTHING: `ListParticipantReplicas` is `@readonly`, so repeated reads
//!    answer identically and leave the stages and replicas where they were.
//! 5. REFUSALS: a stage or participant this store does not hold is the modelled
//!    `ResourceNotFoundException` (404); a request that describes no replication (one
//!    stage as both ends, a participant that cannot publish, an `attributes` map past the
//!    documented 1 KB) is the modelled `ValidationException` (400); replicating a
//!    participant that is already replicated into that stage is the modelled
//!    `ConflictException` (409).
//! 6. A REPLICA DOES NOT OUTLIVE EITHER STAGE IT NAMES: deleting the source stage or the
//!    destination stage ends that replication.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// `StageSessionId`'s `@pattern`, from
/// `smithy_query(query="shape",shape="ParticipantReplica")` — members `sourceSessionId`
/// and `destinationSessionId`.
const STAGE_SESSION_ID_PATTERN: &str = "^st-[a-zA-Z0-9]+$";

/// `StageSessionId`'s `@length`, which the same query reports as `16..16`: a session id is
/// exactly sixteen characters long.
const STAGE_SESSION_ID_LENGTH: usize = 16;

/// The values `ReplicationState` accepts, from
/// `smithy_query(query="shape",shape="ReplicationState")`.
const REPLICATION_STATES: [&str; 2] = ["ACTIVE", "STOPPED"];

/// The state of a replication that has been started and not stopped, named by the model's
/// `StartParticipantReplicationRequest$reconnectWindowSeconds` doc ("replication will
/// continue to be `ACTIVE`").
const STARTED_REPLICATION_STATE: &str = "ACTIVE";

/// The documented maximum size of an `attributes` map, from
/// `StartParticipantReplicationRequest$attributes`: "The maximum length of this field is
/// 1 KB total."
const MAX_ATTRIBUTES_BYTES: usize = 1024;

/// `ReconnectWindowSeconds`' modelled `@range`, from
/// `smithy_query(query="operation",operation="StartParticipantReplication")`: `0..300`.
const RECONNECT_WINDOW_MIN: i64 = 0;
const RECONNECT_WINDOW_MAX: i64 = 300;

/// The capabilities a participant token may grant, from `ParticipantTokenCapability`; the
/// first is the one that makes a participant a publisher.
const PUBLISH_CAPABILITY: &str = "PUBLISH";
const SUBSCRIBE_CAPABILITY: &str = "SUBSCRIBE";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
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

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in `us-east-1`.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = "AWS4-HMAC-SHA256 \
             Credential=AKIDTESTIVS/20260101/us-east-1/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00";
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://ivsrealtime.us-east-1.amazonaws.com/{operation}"))
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
    async fn participant(
        &self,
        case: &str,
        stage_arn: &str,
        capabilities: Option<&[&str]>,
        attributes: &Value,
    ) -> String {
        let mut request = json!({ "stageArn": stage_arn, "attributes": attributes });
        if let Some(capabilities) = capabilities {
            request["capabilities"] = json!(capabilities);
        }
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Start one replication, answering the status and body unchanged.
    async fn start(&self, request: &Value) -> (u16, Value) {
        self.call("StartParticipantReplication", request).await
    }

    /// Start one replication, asserting it was accepted.
    async fn replicate(&self, case: &str, request: &Value) {
        let (status, response) = self.start(request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
             got {status} {response}"
        );
    }

    /// The replicas this store holds for `participant_id` of `source_stage_arn`, with any
    /// extra request members (`maxResults`, `nextToken`) merged in.
    async fn list(
        &self,
        case: &str,
        source_stage_arn: &str,
        participant_id: &str,
        extra: &Value,
    ) -> Value {
        let mut request = json!({
            "sourceStageArn": source_stage_arn,
            "participantId": participant_id,
        });
        merge(&mut request, extra);
        let (status, response) = self.call("ListParticipantReplicas", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantReplicas {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response
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

    /// Delete `stage_arn`, asserting the delete was accepted.
    async fn delete_stage(&self, case: &str, stage_arn: &str) {
        let (status, response) = self.call("DeleteStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteStage on {stage_arn} must answer HTTP 200, \
             got {status} {response}"
        );
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

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Copy every member of `extra` into `target`.
fn merge(target: &mut Value, extra: &Value) {
    let Some(extra) = extra.as_object() else {
        return;
    };
    let target = target
        .as_object_mut()
        .expect("the request under construction is an object");
    for (key, value) in extra {
        target.insert(key.clone(), value.clone());
    }
}

/// An IVS stage ARN in `us-east-1` naming the stage id `id`, in the form the model's
/// `StageArn` pattern requires.
fn stage_arn(id: &str) -> String {
    format!("arn:aws:ivs:us-east-1:{ACCOUNT}:stage/{id}")
}

/// Does `value` satisfy the model `@pattern` quoted on the constant?
///
/// The one pattern this suite checks is simple enough to check directly, which keeps this
/// crate's dependency set exactly what the service itself needs.
fn matches(pattern: &str, value: &str) -> bool {
    match pattern {
        // ^st-[a-zA-Z0-9]+$
        STAGE_SESSION_ID_PATTERN => value
            .strip_prefix("st-")
            .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric())),
        other => panic!("this suite has no checker for the pattern {other:?}"),
    }
}

/// An `attributes` map whose JSON encoding is `bytes` bytes — the measure the documented
/// 1 KB limit is stated in. One entry, so its punctuation is the two braces, the two
/// quotes around the key, the colon and the two quotes around the value.
fn attributes_of_size(bytes: usize) -> HashMap<String, String> {
    HashMap::from([("a".to_string(), "v".repeat(bytes - "{\"a\":\"\"}".len()))])
}

/// Assert `session_id` is a session id in the modelled `StageSessionId` form.
fn assert_session_id(case: &str, what: &str, session_id: &str) {
    assert_eq!(
        session_id.len(),
        STAGE_SESSION_ID_LENGTH,
        "case {case:?}: {what} must be {STAGE_SESSION_ID_LENGTH} characters long, \
         got {session_id:?}"
    );
    assert!(
        matches(STAGE_SESSION_ID_PATTERN, session_id),
        "case {case:?}: {what} must satisfy {STAGE_SESSION_ID_PATTERN}, got {session_id:?}"
    );
}

/// One row of the start-request class: what the request varies, and the identity it
/// replicates.
struct StartCase {
    case: &'static str,
    /// The members `StartParticipantReplication` takes beyond the three required ones.
    extra: Value,
    /// The capabilities the publisher's token grants, or `None` for the modelled default
    /// (`PUBLISH, SUBSCRIBE`).
    capabilities: Option<&'static [&'static str]>,
    /// The attributes the publisher's token carries.
    token_attributes: Value,
}

/// The rows this suite spans the start-request class with: each differs from the others in
/// what the rule generalises over — whether a reconnect window is asked for and where in
/// the modelled range it sits, whether attributes are sent and how big they are, whether
/// they overlap the token's own, and which capabilities the publisher holds.
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "the smallest request the model allows",
            extra: json!({}),
            capabilities: None,
            token_attributes: json!({}),
        },
        StartCase {
            case: "a reconnect window at the modelled minimum",
            extra: json!({ "reconnectWindowSeconds": RECONNECT_WINDOW_MIN }),
            capabilities: None,
            token_attributes: json!({}),
        },
        StartCase {
            case: "a reconnect window one second into the modelled range",
            extra: json!({ "reconnectWindowSeconds": RECONNECT_WINDOW_MIN + 1 }),
            capabilities: None,
            token_attributes: json!({}),
        },
        StartCase {
            case: "a reconnect window at the modelled maximum",
            extra: json!({ "reconnectWindowSeconds": RECONNECT_WINDOW_MAX }),
            capabilities: None,
            token_attributes: json!({}),
        },
        StartCase {
            case: "attributes for the replica participant",
            extra: json!({ "attributes": { "seat": "left", "role": "guest" } }),
            capabilities: None,
            token_attributes: json!({}),
        },
        StartCase {
            case: "attributes overlapping the publisher's own keys",
            extra: json!({ "attributes": { "seat": "right" } }),
            capabilities: None,
            token_attributes: json!({ "seat": "left", "band": "a" }),
        },
        StartCase {
            case: "an attributes map at the documented 1 KB maximum",
            extra: json!({ "attributes": attributes_of_size(MAX_ATTRIBUTES_BYTES) }),
            capabilities: None,
            token_attributes: json!({}),
        },
        StartCase {
            case: "a publisher whose token grants only PUBLISH",
            extra: json!({ "reconnectWindowSeconds": 30 }),
            capabilities: Some(&[PUBLISH_CAPABILITY]),
            token_attributes: json!({}),
        },
        StartCase {
            case: "a publisher whose token grants both capabilities explicitly",
            extra: json!({}),
            capabilities: Some(&[PUBLISH_CAPABILITY, SUBSCRIBE_CAPABILITY]),
            token_attributes: json!({}),
        },
    ]
}

// ---------------------------------------------------------------------------
// StartParticipantReplication, then ListParticipantReplicas
// ---------------------------------------------------------------------------

/// What a start persists is what the listing reports: for every row of the input class,
/// `ListParticipantReplicas` answers exactly the one replica the request asked for, with
/// every `@required` member of `ParticipantReplica` naming the stages, the publisher, the
/// two sessions and the `ACTIVE` state.
#[tokio::test]
async fn a_started_replication_is_reported_for_every_shape_of_request() {
    let emulator = RealTime::new();

    for row in start_cases() {
        let case = row.case;
        let source = emulator.stage(case, "source").await;
        let destination = emulator.stage(case, "destination").await;
        let participant = emulator
            .participant(case, &source, row.capabilities, &row.token_attributes)
            .await;

        let mut request = json!({
            "sourceStageArn": source,
            "destinationStageArn": destination,
            "participantId": participant,
        });
        merge(&mut request, &row.extra);
        emulator.replicate(case, &request).await;

        let listed = emulator.list(case, &source, &participant, &json!({})).await;
        let replicas = listed["replicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: replicas must be a list, got {listed}"));
        assert_eq!(
            replicas.len(),
            1,
            "case {case:?}: one started replication must be reported as one replica, \
             got {listed}"
        );
        let replica = &replicas[0];
        assert_eq!(
            member(replica, "sourceStageArn"),
            source,
            "case {case:?}: sourceStageArn must name the stage the publisher is in, \
             got {replica}"
        );
        assert_eq!(
            member(replica, "destinationStageArn"),
            destination,
            "case {case:?}: destinationStageArn must name the stage replicated into, \
             got {replica}"
        );
        assert_eq!(
            member(replica, "participantId"),
            participant,
            "case {case:?}: participantId must name the replicated publisher, got {replica}"
        );
        assert_eq!(
            member(replica, "replicationState"),
            STARTED_REPLICATION_STATE,
            "case {case:?}: a started replication must report {STARTED_REPLICATION_STATE}, \
             got {replica}"
        );
        assert!(
            REPLICATION_STATES.contains(&member(replica, "replicationState").as_str()),
            "case {case:?}: replicationState must be one of {REPLICATION_STATES:?}, \
             got {replica}"
        );
        assert!(
            listed.get("nextToken").is_none(),
            "case {case:?}: a complete page carries no nextToken, got {listed}"
        );

        // Rule 2: each session id is modelled-shaped, and is the stage's own active
        // session as a client reads it off the stage.
        let source_session = member(replica, "sourceSessionId");
        let destination_session = member(replica, "destinationSessionId");
        assert_session_id(case, "sourceSessionId", &source_session);
        assert_session_id(case, "destinationSessionId", &destination_session);
        assert_ne!(
            source_session, destination_session,
            "case {case:?}: two stages must not share one session, got {replica}"
        );
        assert_eq!(
            emulator.active_session(case, &source).await,
            source_session,
            "case {case:?}: sourceSessionId must be the source stage's activeSessionId, \
             got {replica}"
        );
        assert_eq!(
            emulator.active_session(case, &destination).await,
            destination_session,
            "case {case:?}: destinationSessionId must be the destination stage's \
             activeSessionId, got {replica}"
        );
    }
}

/// A publisher is a publisher however its identity was issued: a token from
/// `CreateParticipantToken`, a token minted by `CreateStage`, or the credential of an
/// ingest configuration associated with the stage.
#[tokio::test]
async fn a_publisher_of_any_issued_identity_can_be_replicated() {
    let emulator = RealTime::new();
    let destination = emulator.stage("fixture", "destination").await;

    // (a) A token issued against an existing stage.
    let token_stage = emulator.stage("a token of the stage", "token-stage").await;
    let from_token = emulator
        .participant("a token of the stage", &token_stage, None, &json!({}))
        .await;

    // (b) A token minted by `CreateStage` itself, from a
    // `participantTokenConfigurations` entry.
    let (status, created) = emulator
        .call(
            "CreateStage",
            &json!({
                "name": "created-with-tokens",
                "participantTokenConfigurations": [{ "userId": "host" }],
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateStage with a token configuration must answer HTTP 200, got {created}"
    );
    let created_stage = member(&created["stage"], "arn");
    let from_create = member(&created["participantTokens"][0], "participantId");
    assert!(
        !from_create.is_empty(),
        "CreateStage must mint the participant of its token configuration, got {created}"
    );

    // (c) The credential of an ingest configuration associated with a stage: an ingest
    // key exists to broadcast, so its participant publishes.
    let ingest_stage = emulator.stage("an ingest credential", "ingest-stage").await;
    let (status, ingest) = emulator
        .call(
            "CreateIngestConfiguration",
            &json!({ "ingestProtocol": "RTMPS", "stageArn": ingest_stage }),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateIngestConfiguration must answer HTTP 200, got {ingest}"
    );
    let from_ingest = member(&ingest["ingestConfiguration"], "participantId");

    let identities = [
        ("a token of the stage", token_stage, from_token),
        ("a token minted by CreateStage", created_stage, from_create),
        ("an ingest credential", ingest_stage, from_ingest),
    ];
    for (case, source, participant) in identities {
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
        let listed = emulator.list(case, &source, &participant, &json!({})).await;
        assert_eq!(
            listed["replicas"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: the replication of {participant} must be reported, got {listed}"
        );
        assert_eq!(
            member(&listed["replicas"][0], "destinationStageArn"),
            destination,
            "case {case:?}: the replica must name the destination stage, got {listed}"
        );
    }
}

/// One publisher replicated into several stages has one replica per destination, reported
/// in start order and paged by `maxResults`: no replica is repeated and none is lost,
/// every page but the last carries a `nextToken`, and every replica names the ONE session
/// its source stage has.
#[tokio::test]
async fn replicas_of_one_publisher_page_in_start_order() {
    let emulator = RealTime::new();
    let case = "five destinations, two per page";
    let source = emulator.stage(case, "source").await;
    let participant = emulator.participant(case, &source, None, &json!({})).await;

    let mut destinations = Vec::new();
    for index in 0..5 {
        let destination = emulator.stage(case, &format!("destination-{index}")).await;
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
        destinations.push(destination);
    }

    // The whole set in one page, which is what a request that sizes no page gets.
    let all = emulator.list(case, &source, &participant, &json!({})).await;
    let listed_all: Vec<String> = all["replicas"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: replicas must be a list, got {all}"))
        .iter()
        .map(|replica| member(replica, "destinationStageArn"))
        .collect();
    assert_eq!(
        listed_all, destinations,
        "case {case:?}: every replica must be reported in start order, got {all}"
    );

    // The same set, two at a time.
    let mut paged: Vec<String> = Vec::new();
    let mut sessions: HashSet<String> = HashSet::new();
    let mut page_sizes: Vec<usize> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..4 {
        let mut request = json!({ "maxResults": 2 });
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let page = emulator.list(case, &source, &participant, &request).await;
        let replicas = page["replicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: replicas must be a list, got {page}"))
            .clone();
        page_sizes.push(replicas.len());
        for replica in &replicas {
            paged.push(member(replica, "destinationStageArn"));
            sessions.insert(member(replica, "sourceSessionId"));
        }
        next_token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }

    assert_eq!(
        page_sizes,
        vec![2, 2, 1],
        "case {case:?}: maxResults=2 over five replicas must page 2, 2, 1, \
         got {page_sizes:?}"
    );
    assert!(
        next_token.is_none(),
        "case {case:?}: the last page must carry no nextToken, got {next_token:?}"
    );
    assert_eq!(
        paged, destinations,
        "case {case:?}: paging must report every replica once, in start order, got {paged:?}"
    );
    assert_eq!(
        sessions.len(),
        1,
        "case {case:?}: every replica of one stage must name that stage's one session, \
         got {sessions:?}"
    );

    // A token this listing did not issue names no position in it: the one `ListStages`
    // issued is refused rather than read as a start.
    let (status, stages) = emulator.call("ListStages", &json!({ "maxResults": 1 })).await;
    assert_eq!(status, 200, "ListStages must answer HTTP 200, got {stages}");
    let foreign = member(&stages, "nextToken");
    assert!(
        !foreign.is_empty(),
        "the fixture needs a token from the other listing, got {stages}"
    );
    let (status, refused) = emulator
        .call(
            "ListParticipantReplicas",
            &json!({
                "sourceStageArn": source,
                "participantId": participant,
                "nextToken": foreign,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "a foreign pagination token must be refused with HTTP 400, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ValidationException",
        "a foreign pagination token must be refused as ValidationException, got {refused}"
    );
    assert_eq!(
        error_message(&refused),
        "ListParticipantReplicas.nextToken is required to be a pagination token issued by \
         ListParticipantReplicas",
        "the refusal must name the member and the requirement, got {refused}"
    );
}

/// `ListParticipantReplicas` is `@readonly`: reading twice answers the same replicas, and
/// leaves the stages' sessions as they were. A source stage or participant this store
/// holds no replica for answers an empty page — this operation models no not-found.
#[tokio::test]
async fn listing_replicas_changes_nothing_a_later_caller_sees() {
    let emulator = RealTime::new();
    let case = "one replication, read repeatedly";
    let source = emulator.stage(case, "source").await;
    let destination = emulator.stage(case, "destination").await;
    let participant = emulator.participant(case, &source, None, &json!({})).await;
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

    let first = emulator.list(case, &source, &participant, &json!({})).await;
    let second = emulator.list(case, &source, &participant, &json!({})).await;
    let third = emulator.list(case, &source, &participant, &json!({})).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads must answer identically, got {first} then {second}"
    );
    assert_eq!(
        second, third,
        "case {case:?}: a third read must answer identically, got {second} then {third}"
    );
    assert_eq!(
        emulator.active_session(case, &source).await,
        member(&first["replicas"][0], "sourceSessionId"),
        "case {case:?}: reading replicas must not move the source stage's session, \
         got {first}"
    );

    // Nothing to report, for each way there can be nothing: an unreplicated participant
    // of a real stage, and a stage this store does not hold at all.
    let quiet = emulator
        .participant(case, &source, None, &json!({}))
        .await;
    let empty_cases = [
        ("a participant with no replicas", source.clone(), quiet),
        (
            "a stage this store does not hold",
            stage_arn("absent-stage"),
            member(&first["replicas"][0], "participantId"),
        ),
    ];
    for (empty_case, stage, participant) in empty_cases {
        let listed = emulator.list(empty_case, &stage, &participant, &json!({})).await;
        assert_eq!(
            listed["replicas"].as_array().map(Vec::len),
            Some(0),
            "case {empty_case:?}: an unreplicated participant must answer an empty page, \
             got {listed}"
        );
        assert!(
            listed.get("nextToken").is_none(),
            "case {empty_case:?}: an empty page carries no nextToken, got {listed}"
        );
    }
}

/// One row of the refusal table: the request, and the envelope it must answer with.
struct RefusalCase {
    case: &'static str,
    request: Value,
    status: u16,
    code: &'static str,
    /// `None` for a refusal whose text no rule of this suite fixes (the framework's own
    /// constraint walk words those).
    message: Option<String>,
}

/// Every way a start request can be refused, each with the modelled error, status and the
/// sentence this service states it with.
#[tokio::test]
async fn a_start_this_store_cannot_honour_is_refused_as_modelled() {
    let emulator = RealTime::new();
    let source = emulator.stage("fixture", "source").await;
    let destination = emulator.stage("fixture", "destination").await;
    let publisher = emulator
        .participant("fixture", &source, None, &json!({}))
        .await;
    // A participant of the source stage that cannot publish, and one that belongs to the
    // DESTINATION stage instead.
    let subscriber = emulator
        .participant(
            "fixture",
            &source,
            Some(&[SUBSCRIBE_CAPABILITY]),
            &json!({}),
        )
        .await;
    let elsewhere = emulator
        .participant("fixture", &destination, None, &json!({}))
        .await;
    let absent = stage_arn("no-such-stage");

    let cases = vec![
        RefusalCase {
            case: "a source stage this store does not hold",
            request: json!({
                "sourceStageArn": absent,
                "destinationStageArn": destination,
                "participantId": publisher,
            }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(format!("Resource: {absent} not found")),
        },
        RefusalCase {
            case: "a destination stage this store does not hold",
            request: json!({
                "sourceStageArn": source,
                "destinationStageArn": absent,
                "participantId": publisher,
            }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(format!("Resource: {absent} not found")),
        },
        RefusalCase {
            case: "a participant the source stage has no identity for",
            request: json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": "ghost-1",
            }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some("Resource: participant \"ghost-1\" not found".to_string()),
        },
        RefusalCase {
            case: "a participant of the destination stage rather than the source",
            request: json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": elsewhere,
            }),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(format!("Resource: participant \"{elsewhere}\" not found")),
        },
        RefusalCase {
            case: "a participant whose token grants no publish capability",
            request: json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": subscriber,
            }),
            status: 400,
            code: "ValidationException",
            message: Some(format!(
                "StartParticipantReplication.participantId is required to name a participant \
                 publishing in {source}, found {subscriber} which holds no \
                 {PUBLISH_CAPABILITY} capability"
            )),
        },
        RefusalCase {
            case: "one stage named as both ends of the replication",
            request: json!({
                "sourceStageArn": source,
                "destinationStageArn": source,
                "participantId": publisher,
            }),
            status: 400,
            code: "ValidationException",
            message: Some(format!(
                "StartParticipantReplication.destinationStageArn is required to name a stage \
                 other than sourceStageArn, found {source}"
            )),
        },
        RefusalCase {
            case: "an attributes map one byte past the documented 1 KB",
            request: json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": publisher,
                "attributes": attributes_of_size(MAX_ATTRIBUTES_BYTES + 1),
            }),
            status: 400,
            code: "ValidationException",
            message: Some(format!(
                "Request member: attributes failed to satisfy the constraints: attributes \
                 is required to have a maximum size of 1 KB, found {} bytes",
                MAX_ATTRIBUTES_BYTES + 1
            )),
        },
        RefusalCase {
            case: "a reconnect window one second past the modelled maximum",
            request: json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": publisher,
                "reconnectWindowSeconds": RECONNECT_WINDOW_MAX + 1,
            }),
            status: 400,
            code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a reconnect window one second below the modelled minimum",
            request: json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": publisher,
                "reconnectWindowSeconds": RECONNECT_WINDOW_MIN - 1,
            }),
            status: 400,
            code: "ValidationException",
            message: None,
        },
    ];

    for row in cases {
        let (status, response) = emulator.start(&row.request).await;
        assert_eq!(
            status, row.status,
            "case {:?}: {} must answer HTTP {}, got {status} {response}",
            row.case, row.request, row.status
        );
        assert_eq!(
            error_code(&response),
            row.code,
            "case {:?}: the refusal must carry the code {}, got {response}",
            row.case,
            row.code
        );
        if let Some(message) = row.message {
            assert_eq!(
                error_message(&response),
                message,
                "case {:?}: the refusal must state {message:?}, got {response}",
                row.case
            );
        }
    }

    // Nothing above was persisted: the publisher still has no replica, and the source
    // stage still has no session.
    let listed = emulator
        .list("after the refusals", &source, &publisher, &json!({}))
        .await;
    assert_eq!(
        listed["replicas"].as_array().map(Vec::len),
        Some(0),
        "a refused start must persist no replica, got {listed}"
    );
    assert_eq!(
        emulator.active_session("after the refusals", &source).await,
        "",
        "a refused start must open no stage session"
    );
}

/// A participant is replicated into one stage ONCE: a second start for the same
/// destination is the modelled `ConflictException` (409) and changes nothing, while a
/// further destination is a different replica and is accepted.
#[tokio::test]
async fn replicating_into_a_stage_twice_conflicts() {
    let emulator = RealTime::new();
    let case = "the same destination twice";
    let source = emulator.stage(case, "source").await;
    let destination = emulator.stage(case, "destination").await;
    let other = emulator.stage(case, "other").await;
    let participant = emulator.participant(case, &source, None, &json!({})).await;
    let request = json!({
        "sourceStageArn": source,
        "destinationStageArn": destination,
        "participantId": participant,
    });

    emulator.replicate(case, &request).await;
    // The repeat differs from the first call only in what the rule does NOT generalise
    // over, so each variation of the repeated request must conflict alike.
    let repeats = [
        ("the identical request", json!({})),
        ("a different reconnect window", json!({ "reconnectWindowSeconds": 120 })),
        ("different attributes", json!({ "attributes": { "seat": "left" } })),
    ];
    for (repeat_case, extra) in repeats {
        let mut repeat = request.clone();
        merge(&mut repeat, &extra);
        let (status, response) = emulator.start(&repeat).await;
        assert_eq!(
            status, 409,
            "case {repeat_case:?}: a second replication into one stage must answer HTTP 409, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {repeat_case:?}: the refusal must carry the code ConflictException, \
             got {response}"
        );
        assert_eq!(
            error_message(&response),
            format!(
                "Resource: participant \"{participant}\" of {source} is already replicated \
                 to {destination}"
            ),
            "case {repeat_case:?}: the conflict must name the participant and both stages, \
             got {response}"
        );
    }

    let listed = emulator.list(case, &source, &participant, &json!({})).await;
    assert_eq!(
        listed["replicas"].as_array().map(Vec::len),
        Some(1),
        "case {case:?}: a conflicting start must leave exactly one replica, got {listed}"
    );

    // A further destination is a different replica.
    let mut second = request.clone();
    second["destinationStageArn"] = json!(other);
    emulator.replicate("a further destination", &second).await;
    let listed = emulator.list(case, &source, &participant, &json!({})).await;
    let destinations: Vec<String> = listed["replicas"]
        .as_array()
        .unwrap_or_else(|| panic!("replicas must be a list, got {listed}"))
        .iter()
        .map(|replica| member(replica, "destinationStageArn"))
        .collect();
    assert_eq!(
        destinations,
        vec![destination, other],
        "case {case:?}: a second destination must add a replica, got {listed}"
    );
}

/// A replica does not outlive either stage it names: deleting the source stage or the
/// destination stage ends exactly that replication and leaves the others.
#[tokio::test]
async fn deleting_either_stage_ends_that_replication() {
    for case in ["the destination is deleted", "the source is deleted"] {
        let emulator = RealTime::new();
        let source = emulator.stage(case, "source").await;
        let doomed = emulator.stage(case, "doomed").await;
        let kept = emulator.stage(case, "kept").await;
        let participant = emulator.participant(case, &source, None, &json!({})).await;
        for destination in [&doomed, &kept] {
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
        }

        let deleted = if case == "the source is deleted" {
            &source
        } else {
            &doomed
        };
        emulator.delete_stage(case, deleted).await;

        let listed = emulator.list(case, &source, &participant, &json!({})).await;
        let destinations: Vec<String> = listed["replicas"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: replicas must be a list, got {listed}"))
            .iter()
            .map(|replica| member(replica, "destinationStageArn"))
            .collect();
        let expected: Vec<String> = if case == "the source is deleted" {
            Vec::new()
        } else {
            vec![kept.clone()]
        };
        assert_eq!(
            destinations, expected,
            "case {case:?}: deleting {deleted} must end exactly the replications it is an \
             end of, got {listed}"
        );
    }
}
