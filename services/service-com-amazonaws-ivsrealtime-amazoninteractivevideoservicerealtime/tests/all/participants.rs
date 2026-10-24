// SPDX-License-Identifier: CC0-1.0

//! `ListParticipants` end to end: what the emulator reports for the participants of a
//! stage session.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request,
//! exactly as a signed `aws ivs-realtime list-participants` call arrives, so what is
//! asserted is the emulator's HTTP behaviour and never a handler called directly.
//!
//! The RULE under test, stated once and then exercised over a SET of inputs:
//! `ListParticipants` REPORTS the participants the store holds for the `(stageArn,
//! sessionId)` pair the request names — every participant that joined that session,
//! in join order, the ones that have since left included — selected by the ONE filter
//! the request may carry and paged at `maxResults` (default 50). A request carrying
//! more than one of the four filters is refused with `ValidationException` (400), as
//! is a `nextToken` this operation did not issue; a pair the store holds no session
//! for is not an error (the model declares no `ResourceNotFoundException` here) and
//! selects no participant. Being a read, repeating any of it leaves every answer and
//! every stored record exactly as it was.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the participant token it was
//! issued, which is `state::join_stage_session` — the one place a participant record
//! is created, and the entry point a stage data plane will use. So these cases bring
//! participants into a session through it, reaching the store through this suite's own
//! engine handle, exactly as the sibling `ivs` emulator's `GetStream` suite brings a
//! channel live through `state::start_stream`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;

/// The five values the modelled `ParticipantRecordingFilterByRecordingState` carries,
/// from `smithy_query(query="shape",shape="ParticipantRecordingFilterByRecordingState")`.
/// Every one of them is a row of the filter table below.
const RECORDING_FILTER_STATES: [&str; 5] = ["STARTING", "ACTIVE", "STOPPING", "STOPPED", "FAILED"];

/// The published bounds of `MaxParticipantResults` (`@range 1..100`), from
/// `smithy_query(query="operation",operation="ListParticipants")`.
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

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
    fn join(&self, stage_arn: &str, joined: state::JoinedParticipant) {
        let participant_id = joined.participant_id.clone();
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(&mut store, stage_arn, joined).is_some(),
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

    /// `ListParticipants` for one session, with `extra` merged into the request (the
    /// filters, `maxResults` and `nextToken` a case supplies).
    async fn list(&self, stage_arn: &str, session_id: &str, extra: &Value) -> (u16, Value) {
        let mut request = json!({ "stageArn": stage_arn, "sessionId": session_id });
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        self.call("ListParticipants", &request).await
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// Whether the whole error body names `fragment` in any of its strings.
///
/// Which wire key carries the text is the service's own envelope
/// (`crate::scaffolded::StatedBody`), so this looks at every string of the body
/// rather than pinning one key.
fn error_states(response: &Value, fragment: &str) -> bool {
    response.as_object().is_some_and(|members| {
        members
            .values()
            .any(|value| value.as_str().is_some_and(|text| text.contains(fragment)))
    })
}

/// The `participantId`s a listing reported, in the order it reported them.
fn listed_ids(case: &str, response: &Value) -> Vec<String> {
    let participants = response["participants"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: participants is @required, got {response}");
    });
    participants
        .iter()
        .map(|summary| {
            summary["participantId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
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
/// `ParticipantSummary$firstJoinTime` targets `Time`, which carries
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

/// Whether `session_id` satisfies the modelled `StageSessionId`: exactly sixteen
/// characters matching `^st-[a-zA-Z0-9]+$`, from
/// `smithy_query(query="shape",shape="StageSessionId")`.
fn session_id_is_well_formed(session_id: &str) -> bool {
    session_id.len() == 16
        && session_id
            .strip_prefix("st-")
            .is_some_and(|body| !body.is_empty() && body.chars().all(|c| c.is_ascii_alphanumeric()))
}

/// A well-formed `StageSessionId` no session of this store carries.
const ABSENT_SESSION_ID: &str = "st-0000000000000";

/// One participant a case brings into a session: its user id, whether it publishes,
/// and the recording state the recorder reports for it.
struct Joining {
    user_id: Option<&'static str>,
    published: bool,
    recording_state: Option<&'static str>,
}

impl Joining {
    const fn new(
        user_id: Option<&'static str>,
        published: bool,
        recording_state: Option<&'static str>,
    ) -> Self {
        Self {
            user_id,
            published,
            recording_state,
        }
    }
}

/// Issue a token for each entry of `roster` and record it joining `stage_arn`,
/// returning the participant ids in join order and the session the joins opened.
async fn populate(emulator: &Emulator, stage_arn: &str, roster: &[Joining]) -> (Vec<String>, String) {
    let mut ids = Vec::new();
    for joining in roster {
        let participant_id = emulator.create_token(stage_arn, joining.user_id).await;
        emulator.join(
            stage_arn,
            state::JoinedParticipant {
                participant_id: participant_id.clone(),
                user_id: joining.user_id.map(str::to_string),
                published: joining.published,
                recording_state: joining.recording_state.map(str::to_string),
                attributes: HashMap::new(),
                ..Default::default()
            },
        );
        ids.push(participant_id);
    }
    let session_id = emulator.active_session_id(stage_arn).await;
    assert!(
        session_id_is_well_formed(&session_id),
        "a session opened by a join must carry a modelled StageSessionId, got {session_id:?}"
    );
    (ids, session_id)
}

// ---------------------------------------------------------------------------
// What a page reports
// ---------------------------------------------------------------------------

/// Every participant of the session is reported, in join order, with each modelled
/// member carrying what the join recorded — over sessions of different sizes.
#[tokio::test]
async fn list_participants_reports_every_participant_of_the_session_in_join_order() {
    let emulator = Emulator::new();

    // Rows differ in how many participants joined the session, and in what each one
    // did in it: a lone subscriber, a publisher beside a subscriber, and a session
    // whose roster covers both and an anonymous (no `userId`) participant.
    let cases: Vec<(&str, Vec<Joining>)> = vec![
        (
            "one subscriber",
            vec![Joining::new(Some("solo"), false, None)],
        ),
        (
            "a publisher and a subscriber",
            vec![
                Joining::new(Some("publisher"), true, Some("ACTIVE")),
                Joining::new(Some("subscriber"), false, None),
            ],
        ),
        (
            "five participants, one of them anonymous",
            vec![
                Joining::new(Some("one"), true, Some("STARTING")),
                Joining::new(Some("two"), false, None),
                Joining::new(None, false, None),
                Joining::new(Some("four"), true, Some("STOPPED")),
                Joining::new(Some("five"), false, None),
            ],
        ),
    ];

    for (case, roster) in cases {
        let stage_arn = emulator.create_stage(case).await;
        let joined_at = now_secs();
        let (ids, session_id) = populate(&emulator, &stage_arn, &roster).await;

        let (status, response) = emulator.list(&stage_arn, &session_id, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        assert_eq!(
            listed_ids(case, &response),
            ids,
            "case {case:?}: expected the joined participants in join order, got {response}"
        );
        assert_eq!(
            response["nextToken"],
            Value::Null,
            "case {case:?}: a complete page carries no nextToken, got {response}"
        );

        for (index, joining) in roster.iter().enumerate() {
            let summary = &response["participants"][index];
            assert_eq!(
                summary["userId"],
                joining.user_id.map_or(Value::Null, |user_id| json!(user_id)),
                "case {case:?}: participant {index} must report the token's userId, got {summary}"
            );
            assert_eq!(
                summary["state"],
                json!("CONNECTED"),
                "case {case:?}: a participant that joined is CONNECTED, got {summary}"
            );
            assert_eq!(
                summary["published"],
                json!(joining.published),
                "case {case:?}: participant {index} must report what it published, got {summary}"
            );
            assert_eq!(
                summary["recordingState"],
                json!(joining.recording_state.unwrap_or("DISABLED")),
                "case {case:?}: participant {index} must report its recording state, got {summary}"
            );
            assert_eq!(
                summary["replicationType"],
                json!("NONE"),
                "case {case:?}: an unreplicated participant is NONE, got {summary}"
            );
            assert_eq!(
                summary["redundantIngest"],
                json!(false),
                "case {case:?}: participant {index} used no redundant ingest, got {summary}"
            );
            let first_join = timestamp_secs(case, &summary["firstJoinTime"]);
            assert!(
                (joined_at - 5..=now_secs() + 5).contains(&first_join),
                "case {case:?}: participant {index} must report the instant it joined \
                 ({joined_at}..{}), got {first_join}",
                now_secs()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The one filter a request may carry
// ---------------------------------------------------------------------------

/// The filter the request carries selects which participants of the session are
/// reported, one member at a time and for every value each member admits.
#[tokio::test]
async fn list_participants_selects_the_participants_the_filter_names() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage("filtered").await;

    // One roster spanning what the filters select over: a publisher and a subscriber,
    // a participant with no `userId` at all, one that has LEFT the session, and one
    // participant in each of the five recording states the modelled recording filter
    // carries.
    let roster = vec![
        Joining::new(Some("alice"), true, Some("ACTIVE")),
        Joining::new(Some("bob"), false, None),
        Joining::new(Some("carol"), true, Some("STOPPED")),
        Joining::new(None, false, None),
        Joining::new(Some("dave"), true, Some("STARTING")),
        Joining::new(Some("erin"), true, Some("STOPPING")),
        Joining::new(Some("frank"), true, Some("FAILED")),
    ];
    let (ids, session_id) = populate(&emulator, &stage_arn, &roster).await;
    let (alice, bob, carol, anonymous, dave, erin, frank) = (
        ids[0].clone(),
        ids[1].clone(),
        ids[2].clone(),
        ids[3].clone(),
        ids[4].clone(),
        ids[5].clone(),
        ids[6].clone(),
    );
    // Carol leaves, so one participant of the session is DISCONNECTED while the rest
    // are still in it.
    emulator.leave(&stage_arn, &session_id, &carol);

    let all_connected = vec![
        alice.clone(),
        bob.clone(),
        anonymous.clone(),
        dave.clone(),
        erin.clone(),
        frank.clone(),
    ];
    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filter at all", json!({}), ids.clone()),
        (
            "filterByUserId naming a participant",
            json!({ "filterByUserId": "alice" }),
            vec![alice.clone()],
        ),
        (
            // `UserId` is `@length 0..128`, so the empty user id is a value a request
            // may filter on: it selects the participant whose token named none.
            "filterByUserId empty",
            json!({ "filterByUserId": "" }),
            vec![anonymous.clone()],
        ),
        (
            "filterByUserId naming nobody in the session",
            json!({ "filterByUserId": "zoe" }),
            Vec::new(),
        ),
        (
            "filterByPublished true",
            json!({ "filterByPublished": true }),
            vec![
                alice.clone(),
                carol.clone(),
                dave.clone(),
                erin.clone(),
                frank.clone(),
            ],
        ),
        (
            // The member's `@default false` makes an absent member and `false` one
            // value, and `false` does not ask for "only show participants who
            // published": it filters nothing.
            "filterByPublished false",
            json!({ "filterByPublished": false }),
            ids.clone(),
        ),
        (
            "filterByState CONNECTED",
            json!({ "filterByState": "CONNECTED" }),
            all_connected,
        ),
        (
            "filterByState DISCONNECTED",
            json!({ "filterByState": "DISCONNECTED" }),
            vec![carol.clone()],
        ),
    ];
    // One row per value of the modelled recording filter, each naming the participant
    // the recorder reported in that state.
    let recorded: HashMap<&str, String> = HashMap::from([
        ("STARTING", dave),
        ("ACTIVE", alice),
        ("STOPPING", erin),
        ("STOPPED", carol),
        ("FAILED", frank),
    ]);
    let recording_rows: Vec<(String, Value, Vec<String>)> = RECORDING_FILTER_STATES
        .iter()
        .map(|recording_state| {
            (
                format!("filterByRecordingState {recording_state}"),
                json!({ "filterByRecordingState": recording_state }),
                vec![recorded[*recording_state].clone()],
            )
        })
        .collect();

    for (case, request, expected) in &recording_rows {
        let (status, response) = emulator.list(&stage_arn, &session_id, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        assert_eq!(
            &listed_ids(case, &response),
            expected,
            "case {case:?}: expected {expected:?}, got {response}"
        );
    }

    // A participant nothing records is selected by NO recording-filter value:
    // `DISABLED` is the one `ParticipantRecordingState` that is not among them, which
    // the rows above show — each reports exactly the one participant the recorder put
    // in that state, never the unrecorded subscriber.
    assert!(
        !recorded.values().any(|id| *id == bob),
        "the unrecorded subscriber must be in no recording state"
    );

    for (case, request, expected) in cases {
        let (status, response) = emulator.list(&stage_arn, &session_id, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        assert_eq!(
            listed_ids(case, &response),
            expected,
            "case {case:?}: expected {expected:?}, got {response}"
        );
    }

    // The listing is a READ: every answer above is the same when it is asked again,
    // and `bob` is still in the session.
    let (status, repeated) = emulator
        .list(&stage_arn, &session_id, &json!({ "filterByUserId": "bob" }))
        .await;
    assert_eq!(status, 200, "a repeated read got {status} {repeated}");
    assert_eq!(
        listed_ids("repeated read", &repeated),
        vec![bob],
        "a read must not change what a later caller sees, got {repeated}"
    );
}

/// A request carrying more than one of the four filters is refused, and reports no
/// page at all.
#[tokio::test]
async fn list_participants_refuses_more_than_one_filter() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage("conflicting-filters").await;
    let roster = vec![
        Joining::new(Some("alice"), true, Some("ACTIVE")),
        Joining::new(Some("bob"), false, None),
    ];
    let (ids, session_id) = populate(&emulator, &stage_arn, &roster).await;

    // Rows: every pair of the four members, then a triple and the whole set.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "userId and published",
            json!({ "filterByUserId": "alice", "filterByPublished": true }),
            vec!["filterByUserId", "filterByPublished"],
        ),
        (
            "userId and state",
            json!({ "filterByUserId": "alice", "filterByState": "CONNECTED" }),
            vec!["filterByUserId", "filterByState"],
        ),
        (
            "userId and recordingState",
            json!({ "filterByUserId": "alice", "filterByRecordingState": "ACTIVE" }),
            vec!["filterByUserId", "filterByRecordingState"],
        ),
        (
            "published and state",
            json!({ "filterByPublished": true, "filterByState": "CONNECTED" }),
            vec!["filterByPublished", "filterByState"],
        ),
        (
            "published and recordingState",
            json!({ "filterByPublished": true, "filterByRecordingState": "ACTIVE" }),
            vec!["filterByPublished", "filterByRecordingState"],
        ),
        (
            "state and recordingState",
            json!({ "filterByState": "CONNECTED", "filterByRecordingState": "ACTIVE" }),
            vec!["filterByState", "filterByRecordingState"],
        ),
        (
            "three of them",
            json!({
                "filterByUserId": "alice",
                "filterByPublished": true,
                "filterByState": "CONNECTED",
            }),
            vec!["filterByUserId", "filterByPublished", "filterByState"],
        ),
        (
            "all four",
            json!({
                "filterByUserId": "alice",
                "filterByPublished": true,
                "filterByState": "CONNECTED",
                "filterByRecordingState": "ACTIVE",
            }),
            vec![
                "filterByUserId",
                "filterByPublished",
                "filterByState",
                "filterByRecordingState",
            ],
        ),
    ];

    for (case, request, named) in cases {
        let (status, response) = emulator.list(&stage_arn, &session_id, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response}"
        );
        for member in named {
            assert!(
                error_states(&response, member),
                "case {case:?}: the refusal must name {member}, got {response}"
            );
        }
        assert!(
            response["participants"].as_array().is_none(),
            "case {case:?}: a refused listing reports no page, got {response}"
        );
    }

    // The refusals changed nothing: the session still reports its whole roster.
    let (status, response) = emulator.list(&stage_arn, &session_id, &json!({})).await;
    assert_eq!(status, 200, "after the refusals: got {status} {response}");
    assert_eq!(
        listed_ids("after the refusals", &response),
        ids,
        "a refused listing must leave the session as it was, got {response}"
    );
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Walking the pages of a session reports every matching participant exactly once, in
/// join order, whatever the page size — and only the last page carries no token.
#[tokio::test]
async fn list_participants_pages_the_session_in_join_order() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage("paged").await;
    let roster: Vec<Joining> = vec![
        Joining::new(Some("p1"), true, Some("ACTIVE")),
        Joining::new(Some("p2"), false, None),
        Joining::new(Some("p3"), true, Some("ACTIVE")),
        Joining::new(Some("p4"), false, None),
        Joining::new(Some("p5"), true, Some("ACTIVE")),
    ];
    let (ids, session_id) = populate(&emulator, &stage_arn, &roster).await;
    let publishers: Vec<String> = vec![ids[0].clone(), ids[2].clone(), ids[4].clone()];

    // Rows differ in the page size — one, a divisor, a non-divisor, exactly the
    // roster, both published bounds of `MaxParticipantResults` and no `maxResults` at
    // all — and in whether a filter travels with each page.
    let cases: Vec<(&str, Option<i64>, Value, Vec<String>)> = vec![
        ("one at a time", Some(1), json!({}), ids.clone()),
        ("a divisor of the roster", Some(5), json!({}), ids.clone()),
        ("a non-divisor", Some(2), json!({}), ids.clone()),
        (
            "the published minimum",
            Some(MAX_RESULTS_MIN),
            json!({}),
            ids.clone(),
        ),
        (
            "the published maximum",
            Some(MAX_RESULTS_MAX),
            json!({}),
            ids.clone(),
        ),
        ("no maxResults at all", None, json!({}), ids.clone()),
        (
            "a filter carried on every page",
            Some(1),
            json!({ "filterByPublished": true }),
            publishers,
        ),
    ];

    for (case, max_results, filter, expected) in cases {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = filter.clone();
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let (status, response) = emulator.list(&stage_arn, &session_id, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: page {pages} got {status} {response}"
            );
            let page = listed_ids(case, &response);
            if let Some(max) = max_results {
                assert!(
                    i64::try_from(page.len()).unwrap_or(i64::MAX) <= max,
                    "case {case:?}: page {pages} must hold at most {max}, got {response}"
                );
            }
            walked.extend(page);
            next_token = response["nextToken"].as_str().map(str::to_string);
            let remaining = expected.len() - walked.len().min(expected.len());
            assert_eq!(
                next_token.is_some(),
                remaining > 0,
                "case {case:?}: page {pages} must carry a nextToken exactly when \
                 participants remain ({remaining} left), got {response}"
            );
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case {case:?}: pagination must terminate, walked {walked:?}"
            );
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, expected,
            "case {case:?}: walking every page must report each matching participant \
             once, in join order"
        );
    }
}

/// A `maxResults` outside the modelled range is refused, at each published bound and
/// one past it.
#[tokio::test]
async fn list_participants_refuses_a_max_results_outside_the_modelled_range() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage("bounded").await;
    let (_, session_id) = populate(
        &emulator,
        &stage_arn,
        &[Joining::new(Some("only"), false, None)],
    )
    .await;

    // Rows: each published bound of `@range 1..100`, and one past each of them.
    let cases: Vec<(&str, i64, u16)> = vec![
        ("one below the minimum", MAX_RESULTS_MIN - 1, 400),
        ("the minimum", MAX_RESULTS_MIN, 200),
        ("the maximum", MAX_RESULTS_MAX, 200),
        ("one above the maximum", MAX_RESULTS_MAX + 1, 400),
    ];

    for (case, max_results, expected) in cases {
        let (status, response) = emulator
            .list(&stage_arn, &session_id, &json!({ "maxResults": max_results }))
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
async fn list_participants_refuses_a_page_token_it_did_not_issue() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage("foreign-tokens").await;
    let (ids, session_id) = populate(
        &emulator,
        &stage_arn,
        &[
            Joining::new(Some("alice"), false, None),
            Joining::new(Some("bob"), false, None),
        ],
    )
    .await;

    // A real token from this service's OTHER listing: it names a stage position, which
    // is no position among participants.
    let second_stage = emulator.create_stage("another-stage").await;
    let (status, listed) = emulator
        .call("ListStages", &json!({ "maxResults": 1 }))
        .await;
    assert_eq!(status, 200, "ListStages got {status} {listed}");
    let stage_token = listed["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("two stages exist, so ListStages pages: {listed}"))
        .to_string();
    assert!(!second_stage.is_empty(), "the second stage must exist");

    // Rows: a token from the sibling listing, base64 of text this service never
    // encodes, and a value that is not base64 at all — each inside the modelled
    // `PaginationToken` pattern `^[a-zA-Z0-9+/=_-]*$`, so the refusal under test is
    // this operation's and not the constraint walk's.
    let cases: Vec<(&str, String)> = vec![
        ("a ListStages token", stage_token),
        (
            "base64 of a position this service never issued",
            aws_smithy_types::base64::encode("participant-after:1"),
        ),
        ("not base64 at all", "not-a-token".to_string()),
    ];

    for (case, token) in cases {
        let (status, response) = emulator
            .list(&stage_arn, &session_id, &json!({ "nextToken": token }))
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
    }

    // The empty token is how an SDK renders "no token": it is the first page.
    let (status, response) = emulator
        .list(&stage_arn, &session_id, &json!({ "nextToken": "" }))
        .await;
    assert_eq!(status, 200, "an empty nextToken got {status} {response}");
    assert_eq!(
        listed_ids("an empty nextToken", &response),
        ids,
        "an empty nextToken is no position, so the first page is reported, got {response}"
    );
}

// ---------------------------------------------------------------------------
// Which session a listing reports
// ---------------------------------------------------------------------------

/// A `(stageArn, sessionId)` pair this store holds no session for reports no
/// participant, rather than failing: the model declares no not-found here.
#[tokio::test]
async fn list_participants_reports_no_participant_for_a_session_the_store_does_not_hold() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage("held").await;
    let (_, session_id) = populate(
        &emulator,
        &stage_arn,
        &[Joining::new(Some("alice"), false, None)],
    )
    .await;

    let other_stage = emulator.create_stage("other").await;
    let (_, other_session) = populate(
        &emulator,
        &other_stage,
        &[Joining::new(Some("bob"), false, None)],
    )
    .await;

    let deleted_stage = emulator.create_stage("deleted").await;
    let (_, deleted_session) = populate(
        &emulator,
        &deleted_stage,
        &[Joining::new(Some("carol"), false, None)],
    )
    .await;
    let (status, deleted) = emulator
        .call("DeleteStage", &json!({ "arn": deleted_stage.clone() }))
        .await;
    assert_eq!(status, 200, "DeleteStage got {status} {deleted}");

    // Rows: a session id no stage has, another stage's session id, a stage ARN this
    // store does not hold, and a stage that has been deleted.
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a session id this store never minted",
            stage_arn.clone(),
            ABSENT_SESSION_ID.to_string(),
        ),
        (
            "another stage's session id",
            stage_arn.clone(),
            other_session,
        ),
        (
            "a stage ARN this store does not hold",
            "arn:aws:ivs:us-east-1:123456789012:stage/absent".to_string(),
            session_id.clone(),
        ),
        (
            "a stage that was deleted with its sessions",
            deleted_stage,
            deleted_session,
        ),
    ];

    for (case, stage, session) in cases {
        let (status, response) = emulator.list(&stage, &session, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        assert_eq!(
            listed_ids(case, &response),
            Vec::<String>::new(),
            "case {case:?}: expected no participant, got {response}"
        );
        assert_eq!(
            response["nextToken"],
            Value::Null,
            "case {case:?}: an empty page carries no nextToken, got {response}"
        );
    }
}

/// Each session reports its OWN participants: a session that ended keeps the ones it
/// held, and the join after it opens a new session reported separately.
#[tokio::test]
async fn each_session_reports_only_the_participants_that_joined_it() {
    let emulator = Emulator::new();
    let stage_arn = emulator.create_stage("two-sessions").await;

    let (first_ids, first_session) = populate(
        &emulator,
        &stage_arn,
        &[
            Joining::new(Some("alice"), true, Some("ACTIVE")),
            Joining::new(Some("bob"), false, None),
        ],
    )
    .await;
    // The last participant leaves, which ends the session: "a stage session begins
    // when the first participant joins a stage and ends after the last participant
    // leaves the stage".
    for participant_id in &first_ids {
        emulator.leave(&stage_arn, &first_session, participant_id);
    }
    assert_eq!(
        emulator.active_session_id(&stage_arn).await,
        "",
        "a stage whose last participant left reports no active session"
    );

    let (second_ids, second_session) = populate(
        &emulator,
        &stage_arn,
        &[Joining::new(Some("carol"), false, None)],
    )
    .await;
    assert_ne!(
        second_session, first_session,
        "a join after the session ended must open a new session"
    );

    // Rows: each session of the one stage, with the participants that joined it.
    for (case, session, expected) in [
        ("the session that ended", &first_session, &first_ids),
        ("the session that followed", &second_session, &second_ids),
    ] {
        let (status, response) = emulator.list(&stage_arn, session, &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {response}"
        );
        assert_eq!(
            &listed_ids(case, &response),
            expected,
            "case {case:?}: expected {expected:?}, got {response}"
        );
    }

    // The participants of the ended session are still reported, as the ones that left:
    // a session groups every participant that joined it, not only those still in it.
    let (status, response) = emulator
        .list(
            &stage_arn,
            &first_session,
            &json!({ "filterByState": "DISCONNECTED" }),
        )
        .await;
    assert_eq!(status, 200, "the ended session got {status} {response}");
    assert_eq!(
        listed_ids("the ended session", &response),
        first_ids,
        "every participant of the ended session has left it, got {response}"
    );
}
