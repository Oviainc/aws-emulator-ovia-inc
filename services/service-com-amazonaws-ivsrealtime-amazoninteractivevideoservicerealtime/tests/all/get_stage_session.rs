// SPDX-License-Identifier: CC0-1.0

//! `GetStageSession` end to end: what the emulator reports for ONE session of ONE
//! stage.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request,
//! exactly as a signed `aws ivs-realtime get-stage-session` call arrives, so what is
//! asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs:
//! `GetStageSession` REPORTS the session record the store holds under the `(stageArn,
//! sessionId)` pair the request names — the id it was opened under, the instant it
//! began and, only once it has ended, the instant it ended — which is the same record
//! `ListStageSessions` reports a summary of, so the two agree member for member. A pair
//! the store resolves no session for is refused with `ResourceNotFoundException` (HTTP
//! 404), naming the identifier that resolved to nothing. Being a read, repeating any of
//! it leaves every answer and every stored record exactly as it was.
//!
//! Nothing in the IVS RealTime control plane OPENS a stage session: the model's
//! `StageSession` doc fixes one as beginning "when the first participant joins a stage"
//! and ending "after the last participant leaves the stage", and a participant reaches
//! a stage over the stage's data plane — which is `state::join_stage_session` /
//! `state::leave_stage_session`, the one place a session is opened and closed. So these
//! cases drive sessions through those, reaching the store through this suite's own
//! engine handle, exactly as the sibling `ListStageSessions` suite does.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;

/// A well-formed `StageSessionId` (`@length 16..16`, `@pattern ^st-[a-zA-Z0-9]+$`, from
/// `smithy_query(query="operation",operation="GetStageSession")`) that no session of
/// this store carries.
const ABSENT_SESSION_ID: &str = "st-0000000000000";

/// A well-formed `StageArn` (`@length 1..128`, `@pattern
/// ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$`) naming a stage this account
/// does not hold, so the constraint walk admits it and the handler decides.
const ABSENT_STAGE_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:stage/no-such-stage";

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

    /// Create a stage and return the ARN the service minted for it.
    ///
    /// The label is sent with every character outside the modelled `StageName` pattern
    /// (`^[a-zA-Z0-9-_]*$`) replaced, so a case may name itself freely.
    async fn create_stage(&self, name: &str) -> String {
        let name: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(status, 200, "CreateStage {name}: got {status} {response}");
        response["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Issue a participant token for `stage_arn` and return the participant id it
    /// carries — the id the holder of that token joins the stage under.
    async fn create_token(&self, stage_arn: &str) -> String {
        let (status, response) = self
            .call("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            status, 200,
            "CreateParticipantToken {stage_arn}: got {status} {response}"
        );
        response["participantToken"]["participantId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The stage's `activeSessionId`, read back over the wire.
    async fn active_session_id(&self, stage_arn: &str) -> String {
        let (status, response) = self.call("GetStage", &json!({ "arn": stage_arn })).await;
        assert_eq!(status, 200, "GetStage {stage_arn}: got {status} {response}");
        response["stage"]["activeSessionId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does.
    fn join(&self, stage_arn: &str, participant_id: &str) {
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
            "the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// Record a participant leaving a session, the way the stage's data plane does.
    fn leave(&self, stage_arn: &str, session_id: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::leave_stage_session(&mut store, stage_arn, session_id, participant_id),
            "the departure of {participant_id:?} from {session_id:?} must be recorded"
        );
    }

    /// Open a session on `stage_arn` by bringing `members` participants in, and return
    /// the session id the joins opened together with the ids that joined it.
    async fn open_session(&self, stage_arn: &str, members: usize) -> (String, Vec<String>) {
        let mut ids = Vec::new();
        for _ in 0..members {
            let participant_id = self.create_token(stage_arn).await;
            self.join(stage_arn, &participant_id);
            ids.push(participant_id);
        }
        let session_id = self.active_session_id(stage_arn).await;
        assert!(
            session_id_is_well_formed(&session_id),
            "a session opened by a join must carry a modelled StageSessionId, got {session_id:?}"
        );
        (session_id, ids)
    }

    /// End the session by having every participant leave it — "a stage session … ends
    /// after the last participant leaves the stage".
    async fn close_session(&self, stage_arn: &str, session_id: &str, members: &[String]) {
        for participant_id in members {
            self.leave(stage_arn, session_id, participant_id);
        }
        assert_eq!(
            self.active_session_id(stage_arn).await,
            "",
            "once every participant has left, {stage_arn:?} reports no active session"
        );
    }

    /// `GetStageSession` for one pair, as it answered.
    async fn get(&self, stage_arn: &str, session_id: &str) -> (u16, Value) {
        self.call(
            "GetStageSession",
            &json!({ "stageArn": stage_arn, "sessionId": session_id }),
        )
        .await
    }

    /// The `stageSession` of a read that must succeed.
    async fn stage_session(&self, case: &str, stage_arn: &str, session_id: &str) -> Value {
        let (status, response) = self.get(stage_arn, session_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetStageSession({stage_arn}, {session_id}) \
             got {status} {response}"
        );
        assert!(
            response["stageSession"].is_object(),
            "case {case:?}: expected a stageSession object, got {response}"
        );
        response["stageSession"].clone()
    }

    /// The `StageSessionSummary` `ListStageSessions` reports for one session.
    async fn summary(&self, case: &str, stage_arn: &str, session_id: &str) -> Value {
        let (status, response) = self
            .call("ListStageSessions", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListStageSessions({stage_arn}) got {status} {response}"
        );
        let sessions = response["stageSessions"].as_array().unwrap_or_else(|| {
            panic!("case {case:?}: stageSessions is @required, got {response}");
        });
        sessions
            .iter()
            .find(|summary| summary["sessionId"].as_str() == Some(session_id))
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: {session_id:?} must be listed, got {response}");
            })
    }
}

/// Whether `session_id` satisfies the modelled `StageSessionId`: exactly sixteen
/// characters matching `^st-[a-zA-Z0-9]+$`, from
/// `smithy_query(query="shape",shape="StageSession")`.
fn session_id_is_well_formed(session_id: &str) -> bool {
    session_id.len() == 16
        && session_id
            .strip_prefix("st-")
            .is_some_and(|body| !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// Whether the whole error body names `fragment` in any of its strings.
///
/// Which wire key carries the text is the service's own envelope, so this looks at
/// every string of the body rather than pinning one key.
fn error_states(response: &Value, fragment: &str) -> bool {
    response.as_object().is_some_and(|members| {
        members
            .values()
            .any(|value| value.as_str().is_some_and(|text| text.contains(fragment)))
    })
}

/// Seconds since the epoch, now.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The epoch-second instant a modelled timestamp member denotes.
///
/// `StageSession$startTime` targets `Time`, which carries
/// `@timestampFormat("date-time")`; whether this emulator's restJson1 serializer
/// renders that as the ISO-8601 string or as the protocol's default epoch-seconds
/// number is a separate question from the one this suite asks, so both renderings are
/// read here as the instant they denote.
fn timestamp_secs(case: &str, value: &Value) -> i64 {
    match value {
        Value::String(rendered) => DateTime::from_str(rendered, Format::DateTime)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: {rendered:?} is not an ISO-8601 instant: {error}")
            })
            .secs(),
        Value::Number(epoch) => epoch.as_f64().unwrap_or_default() as i64,
        other => panic!("case {case:?}: expected a timestamp, got {other}"),
    }
}

/// Drive the session history `members_per_session` describes on a fresh stage: one
/// session per entry, each with that many participants, every session closed again
/// except the last when `leave_last_open`.
///
/// Returns the stage ARN and the session ids in start order.
async fn history(
    emulator: &Emulator,
    label: &str,
    members_per_session: &[usize],
    leave_last_open: bool,
) -> (String, Vec<String>) {
    let stage_arn = emulator.create_stage(label).await;
    let mut session_ids = Vec::new();
    for (index, members) in members_per_session.iter().enumerate() {
        let (session_id, ids) = emulator.open_session(&stage_arn, *members).await;
        let last = index + 1 == members_per_session.len();
        if !(last && leave_last_open) {
            emulator.close_session(&stage_arn, &session_id, &ids).await;
        }
        session_ids.push(session_id);
    }
    (stage_arn, session_ids)
}

// ---------------------------------------------------------------------------
// What a read reports
// ---------------------------------------------------------------------------

/// Every session of a stage is reported with the id it was opened under, the instant it
/// began and `endTime` only once it has ended — over stages with different session
/// histories.
#[tokio::test]
async fn get_stage_session_reports_the_session_the_data_plane_recorded() {
    let emulator = Emulator::new();

    // Rows differ along what the rule generalises over: how many sessions the stage has
    // had, how many participants opened each, and whether the last one is still active.
    let cases: [(&str, &[usize], bool); 5] = [
        ("one session, still active", &[1], true),
        ("one session, ended", &[1], false),
        ("three sessions, the last active", &[1, 2, 3], true),
        ("three sessions, all ended", &[2, 1, 4], false),
        ("five sessions, the last active", &[1, 1, 1, 1, 2], true),
    ];

    for (case, members_per_session, leave_last_open) in cases {
        let before = now_secs();
        let (stage_arn, session_ids) =
            history(&emulator, case, members_per_session, leave_last_open).await;
        let after = now_secs();

        for (index, session_id) in session_ids.iter().enumerate() {
            let session = emulator.stage_session(case, &stage_arn, session_id).await;
            assert_eq!(
                session["sessionId"].as_str(),
                Some(session_id.as_str()),
                "case {case:?}: session {index} must report the id it was opened under, \
                 got {session}"
            );
            let start = timestamp_secs(case, &session["startTime"]);
            assert!(
                (before..=after).contains(&start),
                "case {case:?}: session {index} must report the instant it began \
                 (within {before}..={after}), got {start} in {session}"
            );

            let still_active = leave_last_open && index + 1 == session_ids.len();
            if still_active {
                assert!(
                    session["endTime"].is_null(),
                    "case {case:?}: session {index} is active, so endTime is null, \
                     got {session}"
                );
            } else {
                let end = timestamp_secs(case, &session["endTime"]);
                assert!(
                    (start..=after).contains(&end),
                    "case {case:?}: session {index} must report the instant it ended \
                     (within {start}..={after}), got {end} in {session}"
                );
            }
        }
    }
}

/// What `GetStageSession` reports and what `ListStageSessions` summarises are the same
/// record: every member of the summary is reported identically by the read.
#[tokio::test]
async fn get_stage_session_agrees_with_the_listing_member_for_member() {
    let emulator = Emulator::new();

    // Rows differ in the session history, so the agreement is asserted on active and
    // ended sessions alike.
    let cases: [(&str, &[usize], bool); 3] = [
        ("a single active session", &[2], true),
        ("a single ended session", &[1], false),
        ("four sessions, the last active", &[1, 3, 1, 2], true),
    ];

    for (case, members_per_session, leave_last_open) in cases {
        let (stage_arn, session_ids) =
            history(&emulator, case, members_per_session, leave_last_open).await;
        for session_id in &session_ids {
            let session = emulator.stage_session(case, &stage_arn, session_id).await;
            let summary = emulator.summary(case, &stage_arn, session_id).await;
            assert_eq!(
                session, summary,
                "case {case:?}: GetStageSession must report what ListStageSessions \
                 summarises ({summary}), got {session}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What is refused
// ---------------------------------------------------------------------------

/// Every pair the store resolves no session for is refused with
/// `ResourceNotFoundException` (HTTP 404), naming the identifier that resolved to
/// nothing.
#[tokio::test]
async fn get_stage_session_refuses_a_pair_it_holds_no_session_for() {
    let case = "unresolved pair";
    let emulator = Emulator::new();
    let (stage_arn, sessions) = history(&emulator, "present", &[1], true).await;
    let (other_stage_arn, other_sessions) = history(&emulator, "elsewhere", &[1], true).await;
    let session_id = sessions.first().expect("one session").clone();
    let elsewhere_id = other_sessions.first().expect("one session").clone();
    let bare_stage_arn = emulator.create_stage("never-joined").await;

    // Rows differ in WHICH identifier of the pair resolves to nothing.
    let cases: [(&str, &str, &str, &str); 5] = [
        (
            "a stage this account does not hold",
            ABSENT_STAGE_ARN,
            session_id.as_str(),
            ABSENT_STAGE_ARN,
        ),
        (
            "a session that stage never had",
            stage_arn.as_str(),
            ABSENT_SESSION_ID,
            ABSENT_SESSION_ID,
        ),
        (
            "a session of another stage",
            stage_arn.as_str(),
            elsewhere_id.as_str(),
            elsewhere_id.as_str(),
        ),
        (
            "a stage no participant ever joined",
            bare_stage_arn.as_str(),
            session_id.as_str(),
            session_id.as_str(),
        ),
        (
            "an absent stage and an absent session",
            ABSENT_STAGE_ARN,
            ABSENT_SESSION_ID,
            ABSENT_STAGE_ARN,
        ),
    ];

    for (row, stage, session, named) in cases {
        let (status, response) = emulator.get(stage, session).await;
        assert_eq!(
            status, 404,
            "case {row:?}: expected 404 for ({stage}, {session}), got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {row:?}: expected ResourceNotFoundException, got {response}"
        );
        assert!(
            error_states(&response, named),
            "case {row:?}: expected the refusal to name {named:?}, got {response}"
        );
    }

    // And the read that does resolve still answers, so no refusal above consumed it.
    let session = emulator.stage_session(case, &stage_arn, &session_id).await;
    assert_eq!(
        session["sessionId"].as_str(),
        Some(session_id.as_str()),
        "case {case:?}: the resolvable pair must still be reported, got {session}"
    );
    let elsewhere = emulator
        .stage_session(case, &other_stage_arn, &elsewhere_id)
        .await;
    assert_eq!(
        elsewhere["sessionId"].as_str(),
        Some(elsewhere_id.as_str()),
        "case {case:?}: the other stage's own session must still be reported, \
         got {elsewhere}"
    );
}

// ---------------------------------------------------------------------------
// A read changes nothing
// ---------------------------------------------------------------------------

/// Reading a session — successfully or not — leaves every answer and every stored
/// record exactly as it was, so polling it cannot change what a later caller sees.
#[tokio::test]
async fn get_stage_session_reads_change_nothing() {
    let case = "repeated reads";
    let emulator = Emulator::new();
    let (stage_arn, sessions) = history(&emulator, case, &[1, 2], true).await;
    let ended = sessions.first().expect("the closed session").clone();
    let active = sessions.last().expect("the open session").clone();

    let listed_before = emulator.summary(case, &stage_arn, &active).await;
    let first_active = emulator.stage_session(case, &stage_arn, &active).await;
    let first_ended = emulator.stage_session(case, &stage_arn, &ended).await;

    for read in 2..=4 {
        let again_active = emulator.stage_session(case, &stage_arn, &active).await;
        assert_eq!(
            again_active, first_active,
            "case {case:?}: read {read} of the active session must report what read 1 \
             did ({first_active}), got {again_active}"
        );
        let again_ended = emulator.stage_session(case, &stage_arn, &ended).await;
        assert_eq!(
            again_ended, first_ended,
            "case {case:?}: read {read} of the ended session must report what read 1 \
             did ({first_ended}), got {again_ended}"
        );
    }

    // A refused read changes nothing either.
    let (status, _) = emulator.get(&stage_arn, ABSENT_SESSION_ID).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the refused read must still be a 404"
    );
    let listed_after = emulator.summary(case, &stage_arn, &active).await;
    assert_eq!(
        listed_after, listed_before,
        "case {case:?}: expected the reads to leave the listing as it was \
         ({listed_before}), got {listed_after}"
    );
    let still_active = emulator.active_session_id(&stage_arn).await;
    assert_eq!(
        still_active, active,
        "case {case:?}: expected the stage to still report {active:?} as its active \
         session, got {still_active:?}"
    );
}
