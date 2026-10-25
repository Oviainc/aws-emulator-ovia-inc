// SPDX-License-Identifier: CC0-1.0

//! `ListParticipantEvents` end to end: what the emulator reports for the occurrences of
//! one participant during one stage session.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request,
//! exactly as a signed `aws ivs-realtime list-participant-events` call arrives, so what
//! is asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs:
//! `ListParticipantEvents` REPORTS the occurrences the store holds for the `(stageArn,
//! sessionId)` pair the request names whose own `participantId` is the one the request
//! names, in the order they occurred, paged at `maxResults` (default 50). An occurrence
//! is recorded where it happens — a participant joining or leaving the session, a
//! publisher's media beginning and ending with it, and a replication starting — so no
//! event names something that never happened, and another participant's occurrences are
//! not this participant's. A `nextToken` the operation did not issue is refused with
//! `ValidationException` (400); a session or participant the store holds nothing for is
//! not an error (the model declares no `ResourceNotFoundException` here) and selects no
//! event. Being a read, repeating any of it leaves every answer and every stored record
//! exactly as it was.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the participant token it was
//! issued, which is `state::join_stage_session` / `state::leave_stage_session` — the one
//! place a participant record and the occurrences around it are written. So these cases
//! bring participants into a session through it, reaching the store through this suite's
//! own engine handle, exactly as the `ListParticipants` suite beside it does.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;

/// The published bounds of `MaxParticipantEventResults` (`@range 1..100`), from
/// `smithy_query(query="operation",operation="ListParticipantEvents")`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// A well-formed `StageSessionId` no session of this store carries (the modelled
/// `StageSessionId` is exactly sixteen characters matching `^st-[a-zA-Z0-9]+$`).
const ABSENT_SESSION_ID: &str = "st-0000000000000";

/// A well-formed `ParticipantId` no participant of this store carries (the modelled
/// `ParticipantId` matches `^[a-zA-Z0-9-_]*$`, length 0..64).
const ABSENT_PARTICIPANT_ID: &str = "absent-participant";

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
    /// The name is the case's label with every character outside the modelled
    /// `StageName` pattern (`^[a-zA-Z0-9-_]*$`) replaced, so a case may name itself
    /// freely and still send a name the model accepts.
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
        let request = json!({ "stageArn": stage_arn });
        let (status, response) = self.call("CreateParticipantToken", &request).await;
        assert_eq!(
            status, 200,
            "CreateParticipantToken {request}: got {status} {response}"
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
    fn join(&self, stage_arn: &str, participant_id: &str, published: bool) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(
                &mut store,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.to_string(),
                    published,
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

    /// `ListParticipantEvents` for one participant of one session, with `extra` merged
    /// into the request (`maxResults` and `nextToken` a case supplies).
    async fn list(
        &self,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
        extra: &Value,
    ) -> (u16, Value) {
        let mut request = json!({
            "stageArn": stage_arn,
            "sessionId": session_id,
            "participantId": participant_id,
        });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        self.call("ListParticipantEvents", &request).await
    }

    /// `ListParticipantEvents`, asserting it was served.
    async fn events(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
        extra: &Value,
    ) -> Value {
        let (status, response) = self.list(stage_arn, session_id, participant_id, extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantEvents {extra} must answer HTTP 200, \
             got {status} {response}"
        );
        response
    }
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

/// The events a page reported, as a borrowed slice. `events` is `@required`, so a served
/// page always carries the member.
fn page<'a>(case: &str, response: &'a Value) -> &'a Vec<Value> {
    response["events"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: events is @required, got {response}");
    })
}

/// The `name`s a page reported, in the order it reported them.
fn names(case: &str, response: &Value) -> Vec<String> {
    page(case, response)
        .iter()
        .map(|event| event["name"].as_str().unwrap_or_default().to_string())
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
/// `Event$eventTime` targets `Time`, which carries `@timestampFormat("date-time")`;
/// whether this emulator's restJson1 serializer renders that as the ISO-8601 string or
/// as the protocol's default epoch-seconds number is a separate question from the one
/// this suite asks, so both renderings are read here as the instant they denote.
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

/// One thing the stage's data plane does to the subject participant of a case.
#[derive(Debug, Clone, Copy)]
enum Action {
    /// It joins the session, publishing or only subscribing.
    Join { published: bool },
    /// It leaves the session.
    Leave,
}

/// Run `script` against the subject participant of a fresh stage, and report the stage,
/// the session every action happened in, the subject, and the bystander that keeps the
/// session open.
///
/// A session "ends after the last participant leaves the stage", so a case whose subject
/// leaves and rejoins would otherwise be looking at two different sessions: the
/// bystander joins first and stays, which keeps one session open for the whole script
/// and gives every case a second participant whose own occurrences must not be reported
/// for the subject.
async fn run(emulator: &Emulator, case: &str, script: &[Action]) -> (String, String, String, String) {
    let stage_arn = emulator.create_stage(case).await;
    let bystander = emulator.create_token(&stage_arn).await;
    emulator.join(&stage_arn, &bystander, false);
    let session_id = emulator.active_session_id(&stage_arn).await;
    assert_eq!(
        session_id.len(),
        16,
        "case {case:?}: a session opened by a join must carry a modelled StageSessionId, \
         got {session_id:?}"
    );

    let subject = emulator.create_token(&stage_arn).await;
    for action in script {
        match action {
            Action::Join { published } => emulator.join(&stage_arn, &subject, *published),
            Action::Leave => emulator.leave(&stage_arn, &session_id, &subject),
        }
    }
    (stage_arn, session_id, subject, bystander)
}

// ---------------------------------------------------------------------------
// What a page reports
// ---------------------------------------------------------------------------

/// Every occurrence of the subject participant is reported, in occurrence order, and
/// only its own — over scripts that differ in what the participant did, how often, and
/// how many occurrences that leaves.
#[tokio::test]
async fn list_participant_events_reports_every_occurrence_of_the_participant_in_order() {
    let emulator = Emulator::new();

    // Rows differ along what the data plane did to the subject (subscribe, publish,
    // leave, rejoin), and so along the count and the order of the occurrences.
    let cases: [(&str, &[Action], &[&str]); 6] = [
        (
            "a subscriber that joined",
            &[Action::Join { published: false }],
            &["JOINED"],
        ),
        (
            "a publisher that joined",
            &[Action::Join { published: true }],
            &["JOINED", "PUBLISH_STARTED"],
        ),
        (
            "a subscriber that joined and left",
            &[Action::Join { published: false }, Action::Leave],
            &["JOINED", "LEFT"],
        ),
        (
            "a publisher that joined and left",
            &[Action::Join { published: true }, Action::Leave],
            &["JOINED", "PUBLISH_STARTED", "PUBLISH_STOPPED", "LEFT"],
        ),
        (
            "a subscriber that rejoined",
            &[
                Action::Join { published: false },
                Action::Leave,
                Action::Join { published: false },
            ],
            &["JOINED", "LEFT", "JOINED"],
        ),
        (
            "a publisher that rejoined",
            &[
                Action::Join { published: true },
                Action::Leave,
                Action::Join { published: true },
            ],
            &[
                "JOINED",
                "PUBLISH_STARTED",
                "PUBLISH_STOPPED",
                "LEFT",
                "JOINED",
                "PUBLISH_STARTED",
            ],
        ),
    ];

    for (case, script, expected) in cases {
        let started_at = now_secs();
        let (stage_arn, session_id, subject, bystander) = run(&emulator, case, script).await;

        let response = emulator
            .events(case, &stage_arn, &session_id, &subject, &json!({}))
            .await;
        assert_eq!(
            names(case, &response),
            expected,
            "case {case:?}: expected the subject's occurrences in order, got {response}"
        );
        assert_eq!(
            response["nextToken"],
            Value::Null,
            "case {case:?}: a complete page carries no nextToken, got {response}"
        );

        for (index, event) in page(case, &response).iter().enumerate() {
            assert_eq!(
                event["participantId"],
                json!(subject),
                "case {case:?}: event {index} must name the participant it happened to, \
                 got {event}"
            );
            assert_eq!(
                event["errorCode"],
                Value::Null,
                "case {case:?}: event {index} is not an error event, got {event}"
            );
            assert_eq!(
                event["remoteParticipantId"],
                Value::Null,
                "case {case:?}: event {index} involves no remote participant, got {event}"
            );
            assert_eq!(
                event["destinationStageArn"],
                Value::Null,
                "case {case:?}: event {index} is no replication event, got {event}"
            );
            assert!(
                !event["replica"].as_bool().unwrap_or(false),
                "case {case:?}: event {index} happened to a participant of this stage, \
                 not a replica, got {event}"
            );
            let occurred = timestamp_secs(case, &event["eventTime"]);
            assert!(
                (started_at - 5..=now_secs() + 5).contains(&occurred),
                "case {case:?}: event {index} must report when it occurred \
                 ({started_at}..{}), got {occurred}",
                now_secs()
            );
        }

        // The bystander joined and did nothing else, so its own listing holds exactly
        // that one occurrence: an occurrence belongs to the participant it happened to.
        let others = emulator
            .events(case, &stage_arn, &session_id, &bystander, &json!({}))
            .await;
        assert_eq!(
            names(case, &others),
            vec!["JOINED".to_string()],
            "case {case:?}: the bystander's listing holds only its own join, got {others}"
        );
    }
}

/// A replication start is an occurrence of the replicated publisher, and it names the
/// destination the replication was started into — over one destination and two.
#[tokio::test]
async fn list_participant_events_reports_a_replication_start_with_its_destination() {
    let emulator = Emulator::new();

    for (case, destinations) in [("one destination", 1), ("two destinations", 2)] {
        let (stage_arn, session_id, subject, _) =
            run(&emulator, case, &[Action::Join { published: true }]).await;

        let mut expected_destinations = Vec::new();
        for index in 0..destinations {
            let destination = emulator
                .create_stage(&format!("{case}-destination-{index}"))
                .await;
            let request = json!({
                "sourceStageArn": stage_arn,
                "destinationStageArn": destination,
                "participantId": subject,
            });
            let (status, response) = emulator.call("StartParticipantReplication", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartParticipantReplication {request} must answer HTTP 200, \
                 got {status} {response}"
            );
            let destination_session = emulator.active_session_id(&destination).await;
            expected_destinations.push((destination, destination_session));
        }

        let response = emulator
            .events(case, &stage_arn, &session_id, &subject, &json!({}))
            .await;
        let mut expected = vec!["JOINED".to_string(), "PUBLISH_STARTED".to_string()];
        expected.extend(std::iter::repeat_n(
            "REPLICATION_STARTED".to_string(),
            destinations,
        ));
        assert_eq!(
            names(case, &response),
            expected,
            "case {case:?}: a started replication is an occurrence of the publisher, \
             got {response}"
        );

        for (index, (destination, destination_session)) in
            expected_destinations.iter().enumerate()
        {
            // The two members the model marks applicable only to a replication event.
            let event = &page(case, &response)[2 + index];
            assert_eq!(
                event["destinationStageArn"],
                json!(destination),
                "case {case:?}: replication {index} must name the stage it replicates \
                 into, got {event}"
            );
            assert_eq!(
                event["destinationSessionId"],
                json!(destination_session),
                "case {case:?}: replication {index} must name the destination session, \
                 got {event}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Every page holds at most `maxResults` events, a token is issued exactly while
/// occurrences are left over, and walking the tokens reports every occurrence once, in
/// order — over page sizes from the modelled minimum to its maximum, and with the
/// member left out.
#[tokio::test]
async fn list_participant_events_pages_at_max_results() {
    let emulator = Emulator::new();

    // Six occurrences: three join/leave cycles of a publisher.
    let script = [
        Action::Join { published: false },
        Action::Leave,
        Action::Join { published: false },
        Action::Leave,
        Action::Join { published: false },
        Action::Leave,
    ];
    let whole = ["JOINED", "LEFT", "JOINED", "LEFT", "JOINED", "LEFT"];

    // Rows differ along the page size: each published bound of
    // `MaxParticipantEventResults`, a size that divides the six, one that does not, one
    // larger than the whole run, and the member left out (the documented default of 50).
    let cases: [(&str, Option<i64>); 6] = [
        ("the modelled minimum", Some(MAX_RESULTS_MIN)),
        ("a size that divides the run", Some(2)),
        ("a size that does not divide the run", Some(4)),
        ("a size larger than the run", Some(7)),
        ("the modelled maximum", Some(MAX_RESULTS_MAX)),
        ("no maxResults at all", None),
    ];

    for (case, max_results) in cases {
        let (stage_arn, session_id, subject, _) = run(&emulator, case, &script).await;
        let page_size = max_results.unwrap_or(50);

        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(max_results) = max_results {
                request["maxResults"] = json!(max_results);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let response = emulator
                .events(case, &stage_arn, &session_id, &subject, &request)
                .await;
            let reported = names(case, &response);
            assert!(
                reported.len() as i64 <= page_size,
                "case {case:?}: a page holds at most {page_size} events, got {response}"
            );
            walked.extend(reported);
            next_token = response["nextToken"].as_str().map(str::to_string);
            assert_eq!(
                next_token.is_some(),
                walked.len() < whole.len(),
                "case {case:?}: a token is issued exactly while events are left over, \
                 got {response}"
            );
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: the walk must report every occurrence once, in order"
        );
    }
}

/// A `maxResults` outside the modelled range is refused, and each published bound is
/// accepted.
#[tokio::test]
async fn list_participant_events_holds_the_modelled_max_results_range() {
    let emulator = Emulator::new();
    let (stage_arn, session_id, subject, _) = run(
        &emulator,
        "max results range",
        &[Action::Join { published: false }],
    )
    .await;

    // Each published bound of `MaxParticipantEventResults` (`@range 1..100`), and one
    // past each of them.
    let cases: [(&str, i64, u16); 4] = [
        ("the minimum", MAX_RESULTS_MIN, 200),
        ("the maximum", MAX_RESULTS_MAX, 200),
        ("one below the minimum", MAX_RESULTS_MIN - 1, 400),
        ("one above the maximum", MAX_RESULTS_MAX + 1, 400),
    ];

    for (case, max_results, expected) in cases {
        let (status, response) = emulator
            .list(
                &stage_arn,
                &session_id,
                &subject,
                &json!({ "maxResults": max_results }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: maxResults {max_results} must answer HTTP {expected}, \
             got {status} {response}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: an out-of-range maxResults is a ValidationException, \
                 got {response}"
            );
        }
    }
}

/// A `nextToken` this operation did not issue names no position, so it is refused with
/// `ValidationException` (400) rather than read as "start from the beginning".
#[tokio::test]
async fn list_participant_events_refuses_a_next_token_it_did_not_issue() {
    let emulator = Emulator::new();
    let script = [
        Action::Join { published: false },
        Action::Leave,
        Action::Join { published: false },
    ];
    let (stage_arn, session_id, subject, _) = run(&emulator, "foreign tokens", &script).await;

    // A token from another listing of this same service, and two tokens from nowhere:
    // one that is not base64 at all, one that is base64 of text naming no position.
    let (status, participants) = emulator
        .call(
            "ListParticipants",
            &json!({ "stageArn": stage_arn, "sessionId": session_id, "maxResults": 1 }),
        )
        .await;
    assert_eq!(
        status, 200,
        "the fixture listing must be served, got {status} {participants}"
    );
    let participant_token = participants["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !participant_token.is_empty(),
        "the fixture listing must leave a participant left over, got {participants}"
    );

    let cases: [(&str, String); 3] = [
        ("a ListParticipants token", participant_token),
        ("text that is no token at all", "not-a-token".to_string()),
        (
            "base64 of text naming no position",
            aws_smithy_types::base64::encode("somewhere-else:7"),
        ),
    ];

    for (case, token) in cases {
        let (status, response) = emulator
            .list(
                &stage_arn,
                &session_id,
                &subject,
                &json!({ "nextToken": token }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a foreign nextToken must be refused, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: a foreign nextToken is a ValidationException, got {response}"
        );
        assert!(
            error_states(&response, "ListParticipantEvents.nextToken"),
            "case {case:?}: the refusal must name the member it refuses, got {response}"
        );
    }

    // The refusals changed nothing: the whole run is still reported.
    let response = emulator
        .events("after the refusals", &stage_arn, &session_id, &subject, &json!({}))
        .await;
    assert_eq!(
        names("after the refusals", &response),
        vec!["JOINED", "LEFT", "JOINED"],
        "a refused page must leave the stored occurrences as they were, got {response}"
    );
}

// ---------------------------------------------------------------------------
// What selects nothing, and what a read leaves behind
// ---------------------------------------------------------------------------

/// A session or participant this store holds nothing for is not an error: it selects no
/// event.
#[tokio::test]
async fn list_participant_events_selects_nothing_for_what_the_store_does_not_hold() {
    let emulator = Emulator::new();
    let (stage_arn, session_id, subject, _) = run(
        &emulator,
        "nothing to report",
        &[Action::Join { published: true }],
    )
    .await;
    let other_stage = emulator.create_stage("another stage").await;

    // Rows differ along WHICH part of the request names something the store holds no
    // occurrence under: the session, the participant, or the stage the session belongs
    // to.
    let cases: [(&str, &str, &str, &str); 4] = [
        (
            "a session this stage never had",
            &stage_arn,
            ABSENT_SESSION_ID,
            &subject,
        ),
        (
            "a participant that did nothing",
            &stage_arn,
            &session_id,
            ABSENT_PARTICIPANT_ID,
        ),
        (
            "a session of another stage",
            &other_stage,
            &session_id,
            &subject,
        ),
        (
            "a stage with no session at all",
            &other_stage,
            ABSENT_SESSION_ID,
            &subject,
        ),
    ];

    for (case, stage, session, participant) in cases {
        let response = emulator
            .events(case, stage, session, participant, &json!({}))
            .await;
        assert_eq!(
            names(case, &response),
            Vec::<String>::new(),
            "case {case:?}: expected no event and no error, got {response}"
        );
        assert_eq!(
            response["nextToken"],
            Value::Null,
            "case {case:?}: an empty page carries no nextToken, got {response}"
        );
    }
}

/// Reading the events changes nothing a later caller sees: the listing repeats, and the
/// participants of the session are reported exactly as before.
#[tokio::test]
async fn list_participant_events_is_a_read() {
    let emulator = Emulator::new();
    let script = [
        Action::Join { published: true },
        Action::Leave,
        Action::Join { published: true },
    ];
    let case = "repeated reads";
    let (stage_arn, session_id, subject, _) = run(&emulator, case, &script).await;

    let before = emulator
        .call(
            "ListParticipants",
            &json!({ "stageArn": stage_arn, "sessionId": session_id }),
        )
        .await;
    let first = emulator
        .events(case, &stage_arn, &session_id, &subject, &json!({}))
        .await;
    let second = emulator
        .events(case, &stage_arn, &session_id, &subject, &json!({}))
        .await;
    let third = emulator
        .events(case, &stage_arn, &session_id, &subject, &json!({ "maxResults": 1 }))
        .await;
    let after = emulator
        .call(
            "ListParticipants",
            &json!({ "stageArn": stage_arn, "sessionId": session_id }),
        )
        .await;

    assert_eq!(
        first, second,
        "case {case:?}: a repeated read must answer identically, got {first} then {second}"
    );
    assert_eq!(
        names(case, &third),
        vec!["JOINED".to_string()],
        "case {case:?}: a one-event page holds the first occurrence, got {third}"
    );
    assert_eq!(
        before, after,
        "case {case:?}: reading the events must leave the participants as they were"
    );
}
