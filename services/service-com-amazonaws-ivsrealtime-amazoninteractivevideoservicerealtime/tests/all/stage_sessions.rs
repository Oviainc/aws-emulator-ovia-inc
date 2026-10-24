// SPDX-License-Identifier: CC0-1.0

//! `ListStageSessions` end to end: what the emulator reports for the sessions one
//! stage has had.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request,
//! exactly as a signed `aws ivs-realtime list-stage-sessions` call arrives, so what is
//! asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs:
//! `ListStageSessions` REPORTS the stage sessions the store holds under the `stageArn`
//! the request names — every session that stage has had, in start order, the ones that
//! have ended included — paged at `maxResults` (default 50), each session reported with
//! the id it was opened under, the instant it began and, only once it has ended, the
//! instant it ended. A `nextToken` this operation did not issue is refused with
//! `ValidationException` (400), as is a `maxResults` outside the modelled range; a
//! `stageArn` the store holds no session for is not an error (the model declares no
//! `ResourceNotFoundException` here) and reports no session. Being a read, repeating
//! any of it leaves every answer and every stored record exactly as it was.
//!
//! Nothing in the IVS RealTime control plane OPENS a stage session: the model's
//! `StageSession` doc fixes one as beginning "when the first participant joins a stage"
//! and ending "after the last participant leaves the stage", and a participant reaches
//! a stage over the stage's data plane — which is `state::join_stage_session` /
//! `state::leave_stage_session`, the one place a session is opened and closed. So these
//! cases drive sessions through those, reaching the store through this suite's own
//! engine handle, exactly as the sibling `ListParticipants` suite does.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;

/// The published bounds of `MaxStageSessionResults` (`@range 1..100`), from
/// `smithy_query(query="operation",operation="ListStageSessions")`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// The page size a listing of this service uses when the request omits `maxResults`,
/// documented on `ListStageSessionsRequest$maxResults` ("Default: 50").
const DEFAULT_PAGE_SIZE: usize = 50;

/// A `StageArn` that satisfies the modelled pattern
/// (`^arn:aws:ivs:[a-z0-9-]+:[0-9]+:stage/[a-zA-Z0-9-]+$`, from
/// `smithy_query(query="operation",operation="ListStageSessions")`) and names no stage
/// this store holds, so the constraint walk admits it and the handler decides.
const ABSENT_STAGE_ARN: &str = "arn:aws:ivs:us-east-1:123456789012:stage/no-such-stage";

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

    /// `ListStageSessions` for one stage, with `extra` merged into the request
    /// (`maxResults` and `nextToken` a case supplies).
    async fn list(&self, stage_arn: &str, extra: &Value) -> (u16, Value) {
        let mut request = json!({ "stageArn": stage_arn });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        self.call("ListStageSessions", &request).await
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

/// The response body without the per-request id the emulator's error envelope carries.
///
/// A request id is minted per call by design, so it is the one member two identical
/// calls may differ in; everything else must match for the call to have changed
/// nothing.
fn without_request_id(response: &Value) -> Value {
    let mut stripped = response.clone();
    if let Some(members) = stripped.as_object_mut() {
        members.remove("RequestID");
    }
    stripped
}

/// Whether `session_id` satisfies the modelled `StageSessionId`: exactly sixteen
/// characters matching `^st-[a-zA-Z0-9]+$`, from
/// `smithy_query(query="shape",shape="StageSessionSummary")`.
fn session_id_is_well_formed(session_id: &str) -> bool {
    session_id.len() == 16
        && session_id
            .strip_prefix("st-")
            .is_some_and(|body| !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// The `sessionId`s a listing reported, in the order it reported them.
fn listed_ids(case: &str, response: &Value) -> Vec<String> {
    let sessions = response["stageSessions"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: stageSessions is @required, got {response}");
    });
    sessions
        .iter()
        .map(|summary| summary["sessionId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Seconds since the epoch, now.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// The epoch-second instant a modelled timestamp member denotes.
///
/// `StageSessionSummary$startTime` targets `Time`, which carries
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

// ---------------------------------------------------------------------------
// What a page reports
// ---------------------------------------------------------------------------

/// Every session the stage has had is reported, in start order, with the id it was
/// opened under, the instant it began, and `endTime` only on the sessions that have
/// ended — over stages with different session histories.
#[tokio::test]
async fn list_stage_sessions_reports_every_session_of_the_stage_in_start_order() {
    let emulator = Emulator::new();

    // Rows differ in how many sessions the stage has had, how many participants each
    // one held, and whether the stage is in a session right now.
    let cases: Vec<(&str, Vec<usize>, bool)> = vec![
        ("a stage no participant ever joined", vec![], false),
        ("one session, still active", vec![1], true),
        ("one session, ended", vec![2], false),
        ("three sessions, the last still active", vec![1, 2, 1], true),
        ("four sessions, every one ended", vec![1, 1, 3, 2], false),
    ];

    for (case, members_per_session, leave_last_open) in cases {
        let opened_at = now_secs();
        let (stage_arn, expected_ids) =
            history(&emulator, case, &members_per_session, leave_last_open).await;
        let (status, response) = emulator.list(&stage_arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        assert_eq!(
            listed_ids(case, &response),
            expected_ids,
            "case {case:?}: expected the sessions {expected_ids:?} in start order, got {response}"
        );
        assert!(
            response["nextToken"].is_null(),
            "case {case:?}: {} sessions fit one default page of {DEFAULT_PAGE_SIZE}, so no \
             nextToken is reported, got {response}",
            expected_ids.len(),
        );

        let completed_at = now_secs();
        let sessions = response["stageSessions"].as_array().cloned().unwrap_or_default();
        let mut previous_start = opened_at;
        for (index, summary) in sessions.iter().enumerate() {
            let start = timestamp_secs(case, &summary["startTime"]);
            assert!(
                (opened_at..=completed_at).contains(&start),
                "case {case:?}: session {index} began at {start}, outside the window \
                 {opened_at}..={completed_at} this case ran in"
            );
            assert!(
                start >= previous_start,
                "case {case:?}: session {index} began at {start}, before the previous one \
                 ({previous_start}): sessions are reported in start order"
            );
            previous_start = start;

            // "This is null if the stage is active", as `StageSessionSummary$endTime`
            // states: exactly the last session of a case that left it open is active.
            let still_open = leave_last_open && index + 1 == sessions.len();
            if still_open {
                assert!(
                    summary["endTime"].is_null(),
                    "case {case:?}: session {index} is active, so it reports no endTime, \
                     got {summary}"
                );
            } else {
                let end = timestamp_secs(case, &summary["endTime"]);
                assert!(
                    end >= start && end <= completed_at,
                    "case {case:?}: session {index} ended at {end}, outside \
                     {start}..={completed_at}"
                );
            }
            assert!(
                session_id_is_well_formed(summary["sessionId"].as_str().unwrap_or_default()),
                "case {case:?}: session {index} must carry a modelled StageSessionId, \
                 got {summary}"
            );
        }
    }
}

/// A `stageArn` the store holds no session for is not an error — the model declares no
/// `ResourceNotFoundException` here — it reports no session at all.
#[tokio::test]
async fn list_stage_sessions_reports_no_session_for_a_stage_that_has_had_none() {
    let emulator = Emulator::new();
    let empty_stage = emulator.create_stage("never-joined").await;
    let busy_stage = emulator.create_stage("busy").await;
    let (session_id, ids) = emulator.open_session(&busy_stage, 1).await;
    emulator
        .close_session(&busy_stage, &session_id, &ids)
        .await;

    // Rows: a stage this store holds with no session history, and a well-formed ARN
    // naming no stage of this store at all.
    let cases: Vec<(&str, &str)> = vec![
        ("a stage that exists but was never joined", &empty_stage),
        ("a stage this store does not hold", ABSENT_STAGE_ARN),
    ];

    for (case, stage_arn) in cases {
        let (status, response) = emulator.list(stage_arn, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        assert_eq!(
            listed_ids(case, &response),
            Vec::<String>::new(),
            "case {case:?}: expected no session, got {response}"
        );
        assert!(
            response["nextToken"].is_null(),
            "case {case:?}: an empty page carries no nextToken, got {response}"
        );
    }

    // The sessions of the other stage are untouched by any of that: a session belongs
    // to the stage it was opened on.
    let (status, response) = emulator.list(&busy_stage, &json!({})).await;
    assert_eq!(status, 200, "the busy stage got {status} {response}");
    assert_eq!(
        listed_ids("the busy stage", &response),
        vec![session_id],
        "a session is reported under the stage it was opened on, got {response}"
    );
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// Paging the stage's sessions at every page size yields exactly the same sequence as
/// one unpaged call, with each page but the last full and only the last carrying no
/// `nextToken`.
#[tokio::test]
async fn list_stage_sessions_pages_the_whole_history_at_every_page_size() {
    let emulator = Emulator::new();
    let (stage_arn, expected_ids) =
        history(&emulator, "paged", &[1, 2, 1, 1, 3, 1, 2], true).await;

    // Rows differ in the page size: one at a time, a non-divisor, a divisor, exactly
    // the history, both published bounds of `MaxStageSessionResults`, and no
    // `maxResults` at all.
    let cases: Vec<(&str, Option<i64>)> = vec![
        ("one at a time", Some(1)),
        ("a non-divisor of the history", Some(2)),
        ("the history length", Some(expected_ids.len() as i64)),
        ("one more than the history", Some(expected_ids.len() as i64 + 1)),
        ("the published minimum", Some(MAX_RESULTS_MIN)),
        ("the published maximum", Some(MAX_RESULTS_MAX)),
        ("no maxResults at all", None),
    ];

    for (case, max_results) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({});
            if let Some(max_results) = max_results {
                request["maxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, response) = emulator.list(&stage_arn, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} got {status} {response}"
            );
            let page = listed_ids(case, &response);
            next_token = response["nextToken"].as_str().map(str::to_string);
            let page_size = max_results.unwrap_or(DEFAULT_PAGE_SIZE as i64) as usize;
            assert!(
                page.len() <= page_size,
                "case {case:?}: page {pages} holds {} sessions, more than maxResults \
                 {page_size}",
                page.len()
            );
            if next_token.is_some() {
                assert_eq!(
                    page.len(),
                    page_size,
                    "case {case:?}: page {pages} is not the last, so it must be full"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected_ids.len() + 1,
                "case {case:?}: pagination must terminate, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, expected_ids,
            "case {case:?}: paging must report every session once, in start order"
        );
    }
}

/// A `maxResults` outside the modelled range is refused, and each published bound is
/// served.
#[tokio::test]
async fn list_stage_sessions_refuses_a_max_results_outside_the_modelled_range() {
    let emulator = Emulator::new();
    let (stage_arn, _) = history(&emulator, "bounded", &[1], true).await;

    // Rows: each published bound of `@range 1..100`, and one past each of them.
    let cases: Vec<(&str, i64, u16)> = vec![
        ("one below the minimum", MAX_RESULTS_MIN - 1, 400),
        ("the minimum", MAX_RESULTS_MIN, 200),
        ("the maximum", MAX_RESULTS_MAX, 200),
        ("one above the maximum", MAX_RESULTS_MAX + 1, 400),
    ];

    for (case, max_results, expected) in cases {
        let (status, response) = emulator
            .list(&stage_arn, &json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: maxResults {max_results} expected {expected}, got {status} {response}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response}"
            );
        }
    }
}

/// A `nextToken` this operation did not issue names no position, so it is refused —
/// while an empty token is no token at all and is served from the first page.
#[tokio::test]
async fn list_stage_sessions_refuses_a_page_token_it_did_not_issue() {
    let emulator = Emulator::new();
    let (stage_arn, expected_ids) = history(&emulator, "foreign-tokens", &[1, 2], true).await;

    // A real token from this service's OTHER listings: one names a stage position and
    // one a participant position, neither of which is a position among sessions.
    let second_stage = emulator.create_stage("another-stage").await;
    assert!(!second_stage.is_empty(), "the second stage must exist");
    let (status, listed) = emulator
        .call("ListStages", &json!({ "maxResults": 1 }))
        .await;
    assert_eq!(status, 200, "ListStages got {status} {listed}");
    let stage_token = listed["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("more than one stage exists, so ListStages pages: {listed}"))
        .to_string();

    let active_session = emulator.active_session_id(&stage_arn).await;
    let (status, listed) = emulator
        .call(
            "ListParticipants",
            &json!({ "stageArn": stage_arn, "sessionId": active_session, "maxResults": 1 }),
        )
        .await;
    assert_eq!(status, 200, "ListParticipants got {status} {listed}");
    let participant_token = listed["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("two participants paged at one must leave a token: {listed}"))
        .to_string();

    // Rows: a token from each sibling listing, base64 of text this service never
    // encodes, and a value that is not base64 at all — each inside the modelled
    // `PaginationToken` pattern `^[a-zA-Z0-9+/=_-]*$`, so the refusal under test is
    // this operation's and not the constraint walk's.
    let cases: Vec<(&str, String)> = vec![
        ("a ListStages token", stage_token),
        ("a ListParticipants token", participant_token),
        (
            "base64 of a position this service never issued",
            aws_smithy_types::base64::encode("stage-session-after:1"),
        ),
        ("not base64 at all", "not-a-token".to_string()),
    ];

    for (case, token) in cases {
        let (status, response) = emulator
            .list(&stage_arn, &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token:?}, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        assert!(
            error_states(&response, "nextToken"),
            "case {case:?}: the refusal must name nextToken, got {response}"
        );
        assert!(
            response["stageSessions"].as_array().is_none(),
            "case {case:?}: a refused listing reports no page, got {response}"
        );
    }

    // The empty token is how an SDK renders "no token": it is the first page.
    let (status, response) = emulator
        .list(&stage_arn, &json!({ "nextToken": "" }))
        .await;
    assert_eq!(status, 200, "an empty nextToken got {status} {response}");
    assert_eq!(
        listed_ids("an empty nextToken", &response),
        expected_ids,
        "an empty nextToken is the first page, got {response}"
    );
}

// ---------------------------------------------------------------------------
// The listing is a read
// ---------------------------------------------------------------------------

/// Reporting the sessions changes nothing a later caller sees: repeated calls — plain,
/// paged and refused — answer identically, and the stage's own reading is untouched.
#[tokio::test]
async fn list_stage_sessions_is_a_read_that_changes_nothing() {
    let emulator = Emulator::new();
    let (stage_arn, expected_ids) = history(&emulator, "repeatable", &[1, 2, 1], true).await;
    let active_before = emulator.active_session_id(&stage_arn).await;

    // Rows: the plain listing, a paged first page, a page resumed by a token, and a
    // refused one — each repeated, since a read that moved something would answer
    // differently the second time.
    let first_page = {
        let (status, response) = emulator.list(&stage_arn, &json!({ "maxResults": 1 })).await;
        assert_eq!(status, 200, "the first page got {status} {response}");
        response
    };
    let resume_token = first_page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("three sessions paged at one must leave a token: {first_page}"))
        .to_string();
    let cases: Vec<(&str, Value, u16)> = vec![
        ("the whole history", json!({}), 200),
        ("a page of one", json!({ "maxResults": 1 }), 200),
        (
            "a resumed page",
            json!({ "maxResults": 1, "nextToken": resume_token }),
            200,
        ),
        ("a refused token", json!({ "nextToken": "not-a-token" }), 400),
    ];

    for (case, request, expected) in cases {
        let (first_status, first) = emulator.list(&stage_arn, &request).await;
        let (second_status, second) = emulator.list(&stage_arn, &request).await;
        assert_eq!(
            first_status, expected,
            "case {case:?}: expected {expected}, got {first_status} {first}"
        );
        assert_eq!(
            (first_status, without_request_id(&first)),
            (second_status, without_request_id(&second)),
            "case {case:?}: repeating a read must answer identically, got {first} then {second}"
        );
    }

    let (status, response) = emulator.list(&stage_arn, &json!({})).await;
    assert_eq!(status, 200, "the whole history got {status} {response}");
    assert_eq!(
        listed_ids("after every read", &response),
        expected_ids,
        "every read left the stored sessions as they were, got {response}"
    );
    assert_eq!(
        emulator.active_session_id(&stage_arn).await,
        active_before,
        "reading the sessions must not move the stage's active session"
    );
}
