// SPDX-License-Identifier: CC0-1.0

//! `DisconnectParticipant` as an IDEMPOTENT request: what the emulator answers for a
//! participant id the named stage holds no connection for.
//!
//! Every case goes through the router's own `dispatch` with restJson1 requests, exactly as
//! a signed `aws ivs-realtime disconnect-participant` call arrives, so what is asserted is
//! the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, exercised over a SET of inputs rather than on one request: the
//! resource this operation is addressed to is the STAGE — the only resource its model
//! names a `ResourceNotFoundException` for — while the `participantId` names a connection
//! within that stage. So once the stage resolves, an id it holds no connection for leaves
//! nothing to disconnect: the request is already satisfied, and the operation answers its
//! modelled empty 200 and writes nothing. A `stageArn` this account holds no stage for is
//! still the modelled `ResourceNotFoundException` (HTTP 404), and a member outside its
//! modelled `@pattern` / `@length` is still the modelled `ValidationException` (HTTP 400).
//!
//! The rows differ in what the rule generalises over: WHY the id names no connection of
//! this stage (nothing was ever minted for it; it is the empty id the member's `@length`
//! minimum admits; it is at the `@length` maximum; it is a token participant of another
//! stage; it is the publisher of another stage's ingest configuration; it is one of that
//! configuration's redundant ingest participants; it is a participant of this stage that
//! has already been disconnected), and in whether the stage is otherwise occupied.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the participant token it was issued,
//! which is `state::join_stage_session` — the one place a participant record is created. So
//! the occupied cases bring a participant in through it, reaching the store through this
//! suite's own engine handle, exactly as the `DisconnectParticipant` suite beside this one
//! does.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region this suite signs for.
const REGION: &str = "us-east-1";

/// The modelled `@length` maximum of `ParticipantTokenId`, from
/// `smithy_query(query="operation",operation="DisconnectParticipant")`.
const PARTICIPANT_ID_MAX_LENGTH: usize = 64;

/// The state a participant that is in the stage reports, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const CONNECTED: &str = "CONNECTED";

/// The state a participant that has left reports: the other value of that list.
const DISCONNECTED: &str = "DISCONNECTED";

/// One IVS RealTime emulator over one shared engine, plus the engine handle the data-plane
/// seeding writes through.
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
    /// naming `ivs` in [`REGION`], and report the status and body a client sees.
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
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, body)
    }

    /// Create a stage and return the ARN the service minted for it.
    async fn create_stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage {name} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Issue a participant token for `stage_arn` and return the participant id it carries.
    async fn create_token(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self
            .call("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateParticipantToken must answer HTTP 200, got {status} {response}"
        );
        member(&response["participantToken"], "participantId")
    }

    /// Create an ingest configuration with redundant ingest enabled, associated with
    /// `stage_arn`, and return the `ingestConfiguration` reported for it.
    async fn create_ingest_configuration(&self, case: &str, stage_arn: &str) -> Value {
        let (status, response) = self
            .call(
                "CreateIngestConfiguration",
                &json!({
                    "ingestProtocol": "RTMPS",
                    "redundantIngest": true,
                    "stageArn": stage_arn,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateIngestConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        response["ingestConfiguration"].clone()
    }

    /// `DisconnectParticipant` for one stage and one participant id.
    async fn disconnect(&self, stage_arn: &str, participant_id: &str) -> (u16, Value) {
        self.call(
            "DisconnectParticipant",
            &json!({
                "stageArn": stage_arn,
                "participantId": participant_id,
                "reason": "cleanup",
            }),
        )
        .await
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does.
    fn join(&self, case: &str, stage_arn: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(
                &mut store,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.to_string(),
                    published: true,
                    ..Default::default()
                },
            )
            .is_some(),
            "case {case:?}: the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// The `state` the stage's own reader reports for `participant_id`, or `None` when the
    /// session holds no such participant.
    async fn participant_state(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
    ) -> Option<String> {
        let (status, response) = self
            .call(
                "GetParticipant",
                &json!({
                    "stageArn": stage_arn,
                    "sessionId": session_id,
                    "participantId": participant_id,
                }),
            )
            .await;
        if status == 404 {
            return None;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: GetParticipant must answer HTTP 200 or 404, got {status} {response}"
        );
        Some(member(&response["participant"], "state"))
    }

    /// The participants `ListParticipants` reports for one session of a stage.
    async fn participants(&self, case: &str, stage_arn: &str, session_id: &str) -> Vec<Value> {
        let (status, response) = self
            .call(
                "ListParticipants",
                &json!({ "stageArn": stage_arn, "sessionId": session_id }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipants must answer HTTP 200, got {status} {response}"
        );
        response["participants"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    /// The stage's `activeSessionId`, read back over the wire.
    async fn active_session_id(&self, case: &str, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStage {stage_arn} must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "activeSessionId")
    }
}

/// A string member, read as the empty string when absent.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A disconnect naming a participant the stage holds no connection for answers the
/// modelled empty 200 and leaves every record of the stage exactly as it was.
#[tokio::test]
async fn a_participant_the_stage_holds_nothing_for_is_an_empty_success() {
    let emulator = RealTime::new();
    let stage_arn = emulator.create_stage("the stage", "idempotent").await;
    let other_stage_arn = emulator.create_stage("another stage", "elsewhere").await;

    // The stage is occupied: one participant of its own, which no case names, so every
    // answer below is checked against a stage that really holds something.
    let resident = emulator.create_token("the resident", &stage_arn).await;
    emulator.join("the resident", &stage_arn, &resident);
    let session_id = emulator.active_session_id("the resident", &stage_arn).await;

    // A participant of this stage that has already left: attached, but with no connection
    // left to end.
    let departed = emulator.create_token("the departed", &stage_arn).await;
    emulator.join("the departed", &stage_arn, &departed);
    let (status, response) = emulator.disconnect(&stage_arn, &departed).await;
    assert_eq!(
        status, 200,
        "the first disconnect of the departed participant must answer HTTP 200, \
         got {status} {response}"
    );

    // Identities of ANOTHER stage: a token participant, and the participants an ingest
    // configuration of that stage mints — its own and each of its redundant ones.
    let foreign_token = emulator
        .create_token("another stage's token", &other_stage_arn)
        .await;
    let foreign_configuration = emulator
        .create_ingest_configuration("another stage's configuration", &other_stage_arn)
        .await;
    let foreign_publisher = member(&foreign_configuration, "participantId");
    let foreign_redundant = member(
        &foreign_configuration["redundantIngestCredentials"][0],
        "participantId",
    );

    // Rows differ in WHY the id names no connection of this stage.
    let rows: Vec<(&str, String)> = vec![
        ("an id nothing was ever minted for", "no-such-participant".to_string()),
        ("the empty id the member's length minimum admits", String::new()),
        (
            "an id at the member's length maximum",
            "a".repeat(PARTICIPANT_ID_MAX_LENGTH),
        ),
        (
            "an id using every character the member's pattern admits",
            "Id-9_x".to_string(),
        ),
        ("a token participant of another stage", foreign_token.clone()),
        (
            "the publisher of another stage's ingest configuration",
            foreign_publisher.clone(),
        ),
        (
            "a redundant ingest participant of another stage's configuration",
            foreign_redundant.clone(),
        ),
        (
            "a participant of this stage that already left",
            departed.clone(),
        ),
    ];

    for (case, participant_id) in rows {
        let before = emulator.participants(case, &stage_arn, &session_id).await;
        let (status, response) = emulator.disconnect(&stage_arn, &participant_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for participantId {participant_id:?}, \
             got {status} {response}"
        );
        // `DisconnectParticipantResponse` models no member at all, so the whole answer is
        // the empty JSON object.
        assert_eq!(
            response,
            json!({}),
            "case {case:?}: expected an answer carrying no member, got {response}"
        );

        // Nothing was written: the stage's own participant is still connected, the departed
        // one is still disconnected, the session roster is unchanged, and the stage keeps
        // the session it had.
        let actual = emulator
            .participant_state(case, &stage_arn, &session_id, &resident)
            .await;
        assert_eq!(
            actual.as_deref(),
            Some(CONNECTED),
            "case {case:?}: expected {resident:?} to stay {CONNECTED:?}, got {actual:?}"
        );
        let actual = emulator
            .participant_state(case, &stage_arn, &session_id, &departed)
            .await;
        assert_eq!(
            actual.as_deref(),
            Some(DISCONNECTED),
            "case {case:?}: expected {departed:?} to stay {DISCONNECTED:?}, got {actual:?}"
        );
        let after = emulator.participants(case, &stage_arn, &session_id).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the session roster unchanged, expected {before:?}, \
             got {after:?}"
        );
        let active = emulator.active_session_id(case, &stage_arn).await;
        assert_eq!(
            active, session_id,
            "case {case:?}: expected the stage to keep session {session_id:?}, got {active:?}"
        );
    }

    // And the configuration of the other stage keeps its association: a call naming one of
    // its participants against a stage it does not publish to disconnects nothing.
    let arn = member(&foreign_configuration, "arn");
    let (status, response) = emulator
        .call("GetIngestConfiguration", &json!({ "arn": arn }))
        .await;
    assert_eq!(
        status, 200,
        "the other stage's configuration must still read, got {status} {response}"
    );
    let reported = member(&response["ingestConfiguration"], "stageArn");
    assert_eq!(
        reported, other_stage_arn,
        "expected {arn} to keep stageArn {other_stage_arn:?}, got {reported:?}"
    );
}

/// The stage is the resource this operation is addressed to: a `stageArn` this account
/// holds no stage for is the modelled `ResourceNotFoundException` (404) naming that ARN,
/// whatever the participant id says.
#[tokio::test]
async fn a_stage_this_account_does_not_hold_is_still_refused() {
    let emulator = RealTime::new();
    let held = emulator.create_stage("the stage", "held").await;
    let resident = emulator.create_token("the resident", &held).await;
    let deleted = emulator.create_stage("a stage to delete", "deleted").await;
    let (status, response) = emulator.call("DeleteStage", &json!({ "arn": deleted })).await;
    assert_eq!(
        status, 200,
        "the fixture stage must delete, got {status} {response}"
    );

    let absent = {
        let (prefix, _) = held.rsplit_once('/').expect("a stage ARN names its resource");
        format!("{prefix}/absent123456")
    };
    let another_region = held.replace(REGION, "eu-west-1");

    // Rows differ in HOW the stage fails to resolve: never created, created and deleted,
    // and one of another region; each with a participant id that IS an identity of a held
    // stage, so the refusal is about the stage alone.
    for (case, stage_arn) in [
        ("a stage nothing was created for", absent),
        ("a stage that was deleted", deleted.clone()),
        ("a stage of another region", another_region),
    ] {
        let (status, response) = emulator.disconnect(&stage_arn, &resident).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for stageArn {stage_arn:?}, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response}"
        );
        let expected = format!("Resource: {stage_arn} not found");
        let message = member(&response, "message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to name the stage ({expected:?}), \
             got {message:?}"
        );
    }
}
