// SPDX-License-Identifier: CC0-1.0

//! A `ListParticipants` `nextToken` is refused under any other filter selection, even —
//! especially — when serving it there would silently omit participants.
//!
//! The RULE, stated once and then exercised over a SET of inputs: a page position is an
//! ordinal among the records ONE request's terms select, and a `filterBy*` member is such
//! a term, since the filters are applied before the resume position is compared. A
//! `nextToken` therefore names a position only under the selection that issued it; offered
//! under any other one it names no position among the records that call would page, so it
//! is refused with the modelled `ValidationException` (HTTP 400) carrying this service's
//! one foreign-token sentence — and never served as a page that drops every record lying
//! before that position, which no further paging could ever reach.
//!
//! `tests/all/page_token_filter_scope.rs` pins the same rule across every listing of this
//! service that has filter members. What this suite adds is the DIRECTION in which the
//! defect is observable rather than merely wrong: here the issuing selection's records all
//! hold HIGHER store positions than the resuming selection's, so a served page would be
//! short or outright EMPTY, and each row states which of the two it would be.
//!
//! Rows differ along what the rule generalises over: which of the four filter members
//! issued the token (`filterByUserId`, `filterByPublished`, `filterByState`,
//! `filterByRecordingState` — the whole set, from
//! `smithy_query(query="operation",operation="ListParticipants")`), what the resuming
//! request changes it to (another value of the same member, another member entirely, or no
//! filter at all), and whether the omission a served page would commit covers every record
//! that call selects or only some.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage or LEAVE it:
//! that is the stage's data plane, which is `state::join_stage_session` /
//! `state::leave_stage_session` — so, like the sibling `ListParticipants` suites, the
//! fixture brings participants into a session through that seam and asserts everything else
//! over the router's own `dispatch`, as an SDK client sees it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region this suite signs for: the term every listing of this service pages within
/// beside its own members.
const REGION: &str = "us-east-1";

/// Two of the five values the modelled `ParticipantRecordingFilterByRecordingState`
/// carries (`STARTING | ACTIVE | STOPPING | STOPPED | FAILED`), from
/// `smithy_query(query="shape",shape="ParticipantRecordingFilterByRecordingState")`.
const RECORDING_ACTIVE: &str = "ACTIVE";
const RECORDING_STOPPED: &str = "STOPPED";

/// The two values the modelled `ParticipantState` carries, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const STATE_CONNECTED: &str = "CONNECTED";
const STATE_DISCONNECTED: &str = "DISCONNECTED";

/// The user ids the fixture's participants joined under.
const EARLY_USER: &str = "alice";
const LATE_USER: &str = "bob";

/// The participant that joined FIRST, and so holds the lowest store position of the
/// session: the one a page resumed under the wrong filter would omit.
const EARLY: &str = "early-alice";
/// The two participants that joined after it, whose own filtered page issues the token.
const LATE_ONE: &str = "late-bob-one";
const LATE_TWO: &str = "late-bob-two";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it, plus the engine handle the data-plane seeding writes through.
struct RealTime {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential scope
    /// naming `ivs` in [`REGION`] — which is how a request names the region it is
    /// processed in.
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
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Make the call `operation`, asserting it was served, and answer with its body.
    async fn served(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "fixture {operation} {body}: expected HTTP 200, got {status} {response}"
        );
        response
    }

    /// Create a stage and answer with the ARN the service minted for it.
    async fn stage(&self, name: &str) -> String {
        let response = self.served("CreateStage", &json!({ "name": name })).await;
        text(&response["stage"], "arn")
    }

    /// The stage's `activeSessionId`, read back over the wire.
    async fn active_session(&self, stage_arn: &str) -> String {
        let response = self.served("GetStage", &json!({ "arn": stage_arn })).await;
        text(&response["stage"], "activeSessionId")
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
}

/// A string member of a response, or a marker so a failing assertion names the missing
/// member rather than panicking inside the reader.
fn text(value: &Value, member: &str) -> String {
    value
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
        .to_string()
}

/// The error code a refusal states.
fn error_code(response: &Value) -> String {
    text(response, "Code")
}

/// The text a refusal states.
fn error_message(response: &Value) -> String {
    text(response, "Message")
}

/// The sentence every listing of this service refuses a token naming no position in it
/// with — the service's one wording, kept here as a client reads it.
fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// `left` with every member of `right` merged in (a row's selection, and the `maxResults`
/// or `nextToken` a step adds over it).
fn merged(left: &Value, right: &Value) -> Value {
    let mut request = left.clone();
    if let Some(members) = right.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    request
}

/// The `participantId`s a page reported, in the order it reported them.
fn listed_ids(case: &str, response: &Value) -> Vec<String> {
    let participants = response["participants"].as_array().unwrap_or_else(|| {
        panic!("case {case:?}: participants is @required, got {response}");
    });
    participants
        .iter()
        .map(|summary| text(summary, "participantId"))
        .collect()
}

/// One row: the filter selection that issues a token, the selection the token is then
/// offered to, and what serving it there would have cost.
struct Row {
    case: &'static str,
    /// The filter members the issuing call carries, whose participants all hold HIGHER
    /// store positions than the resuming selection's.
    issuer: Value,
    /// The filter members the resuming call carries instead.
    resumed: Value,
    /// Whether EVERY participant the resuming selection holds lies at or before the
    /// position the token names, so a served page would be empty rather than merely short.
    omits_every_participant: bool,
}

/// One live stage session whose participants differ in every member a `ListParticipants`
/// filter selects on, arranged so the first to join is the one a cross-filter resume would
/// drop.
struct Fixture {
    service: RealTime,
    stage_arn: String,
    session_id: String,
}

impl Fixture {
    async fn new() -> Self {
        let service = RealTime::new();
        let stage_arn = service.stage("filtered-resume").await;

        // Join order IS page order, so the participant joining first holds the lowest
        // position: it is the one every row below selects with its resuming filter and the
        // issuing page's position lies past.
        service.join(
            &stage_arn,
            state::JoinedParticipant {
                participant_id: EARLY.to_string(),
                user_id: Some(EARLY_USER.to_string()),
                published: false,
                recording_state: Some(RECORDING_STOPPED.to_string()),
                ..Default::default()
            },
        );
        for participant in [LATE_ONE, LATE_TWO] {
            service.join(
                &stage_arn,
                state::JoinedParticipant {
                    participant_id: participant.to_string(),
                    user_id: Some(LATE_USER.to_string()),
                    published: true,
                    recording_state: Some(RECORDING_ACTIVE.to_string()),
                    ..Default::default()
                },
            );
        }
        let session_id = service.active_session(&stage_arn).await;
        // The first to join also LEAVES, so the session holds both values the modelled
        // `ParticipantState` carries while staying live for the two that remain.
        service.leave(&stage_arn, &session_id, EARLY);

        Self {
            service,
            stage_arn,
            session_id,
        }
    }

    /// The identity members every call of this suite carries.
    fn base(&self) -> Value {
        json!({ "stageArn": self.stage_arn, "sessionId": self.session_id })
    }

    /// The participants one selection reports, in the order it reported them, asserting
    /// the call was served.
    async fn selects(&self, case: &str, selection: &Value) -> Vec<String> {
        let response = self
            .service
            .served("ListParticipants", &merged(&self.base(), selection))
            .await;
        listed_ids(case, &response)
    }
}

/// The table: every filter member of this listing as the issuer, against each kind of
/// selection a resuming request can change it to.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "filterByUserId, resumed under another user id",
            issuer: json!({ "filterByUserId": LATE_USER }),
            resumed: json!({ "filterByUserId": EARLY_USER }),
            omits_every_participant: true,
        },
        Row {
            case: "filterByUserId, resumed with no filter at all",
            issuer: json!({ "filterByUserId": LATE_USER }),
            resumed: json!({}),
            omits_every_participant: false,
        },
        Row {
            case: "filterByPublished, resumed under another filter member",
            issuer: json!({ "filterByPublished": true }),
            resumed: json!({ "filterByUserId": EARLY_USER }),
            omits_every_participant: true,
        },
        Row {
            case: "filterByPublished, resumed with no filter at all",
            issuer: json!({ "filterByPublished": true }),
            resumed: json!({}),
            omits_every_participant: false,
        },
        Row {
            case: "filterByState, resumed under the other participant state",
            issuer: json!({ "filterByState": STATE_CONNECTED }),
            resumed: json!({ "filterByState": STATE_DISCONNECTED }),
            omits_every_participant: true,
        },
        Row {
            case: "filterByState, resumed with no filter at all",
            issuer: json!({ "filterByState": STATE_CONNECTED }),
            resumed: json!({}),
            omits_every_participant: false,
        },
        Row {
            case: "filterByRecordingState, resumed under another recording state",
            issuer: json!({ "filterByRecordingState": RECORDING_ACTIVE }),
            resumed: json!({ "filterByRecordingState": RECORDING_STOPPED }),
            omits_every_participant: true,
        },
        Row {
            case: "filterByRecordingState, resumed under another filter member",
            issuer: json!({ "filterByRecordingState": RECORDING_ACTIVE }),
            resumed: json!({ "filterByUserId": EARLY_USER }),
            omits_every_participant: true,
        },
    ]
}

/// A token one filter selection issued is refused under every other one, rather than
/// served as the short or empty page that would drop the participants lying before the
/// position it names — while the selection that issued it resumes exactly there.
#[tokio::test]
async fn a_filtered_participants_token_is_refused_rather_than_dropping_participants() {
    let fixture = Fixture::new().await;

    // The session's own page order, which is join order: the participant that joined
    // first is the one every row's resuming selection holds.
    let join_order = fixture.selects("the whole session", &json!({})).await;
    let expected_order = vec![EARLY.to_string(), LATE_ONE.to_string(), LATE_TWO.to_string()];
    assert_eq!(
        join_order, expected_order,
        "the session must report its participants in join order, expected \
         {expected_order:?}, got {join_order:?}"
    );
    let position_of = |case: &str, participant: &str| {
        join_order
            .iter()
            .position(|held| held == participant)
            .unwrap_or_else(|| {
                panic!("case {case:?}: {participant:?} must be one of {join_order:?}")
            })
    };

    for row in rows() {
        let case = row.case;

        // What each selection holds, before any token is in play.
        let issued_over = fixture.selects(case, &row.issuer).await;
        let elsewhere_holds = fixture.selects(case, &row.resumed).await;
        assert!(
            issued_over.len() > 1,
            "case {case:?}: the issuing selection must hold more than one participant for a \
             page of one to leave any behind, got {issued_over:?}"
        );
        assert!(
            !elsewhere_holds.is_empty(),
            "case {case:?}: the resuming selection must hold participants of its own, so a \
             short page there is visible, got {elsewhere_holds:?}"
        );

        // A page of one under the issuing selection, and the token it ends with. The
        // position that token names is the page order of the participant it reported.
        let (status, first) = fixture
            .service
            .call(
                "ListParticipants",
                &merged(
                    &merged(&fixture.base(), &row.issuer),
                    &json!({ "maxResults": 1 }),
                ),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page of one must be served, got {status} {first}"
        );
        let page = listed_ids(case, &first);
        assert_eq!(
            page,
            issued_over[..1].to_vec(),
            "case {case:?}: a page of one must report the first participant the selection \
             holds, expected {:?}, got {page:?}",
            issued_over[..1].to_vec()
        );
        let token = text(&first, "nextToken");
        assert!(
            !token.is_empty() && token != "<absent>",
            "case {case:?}: a page of one with participants left over must carry a \
             nextToken, got {first}"
        );

        // What serving that token under the other selection would have cost: every
        // participant it holds at or before the position the token names is one no further
        // paging could ever reach.
        let ended_at = position_of(case, &issued_over[0]);
        let would_omit: Vec<&String> = elsewhere_holds
            .iter()
            .filter(|participant| position_of(case, participant) <= ended_at)
            .collect();
        assert!(
            !would_omit.is_empty(),
            "case {case:?}: the row must arrange a resuming selection holding a participant \
             at or before the position the token names, got {elsewhere_holds:?} against the \
             position of {:?}",
            issued_over[0]
        );
        assert_eq!(
            would_omit.len() == elsewhere_holds.len(),
            row.omits_every_participant,
            "case {case:?}: expected omits_every_participant={}, but serving the token \
             would drop {would_omit:?} of {elsewhere_holds:?}",
            row.omits_every_participant
        );

        // Under the selection that issued it, the token resumes exactly where that page
        // ended — so the refusal below is the scope rule, not a blanket rejection.
        let remainder = issued_over[1..].to_vec();
        let resumed = fixture
            .selects(case, &merged(&row.issuer, &json!({ "nextToken": token })))
            .await;
        assert_eq!(
            resumed, remainder,
            "case {case:?}: its own selection must resume after the participant the page \
             ended at, expected {remainder:?}, got {resumed:?}"
        );

        // Offered under the other selection, it names no position among the participants
        // that call pages, so it is refused.
        let (status, refused) = fixture
            .service
            .call(
                "ListParticipants",
                &merged(
                    &merged(&fixture.base(), &row.resumed),
                    &json!({ "nextToken": token }),
                ),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token of another filter selection must be refused, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {refused}"
        );
        assert_eq!(
            error_message(&refused),
            foreign_page_token_message("ListParticipants"),
            "case {case:?}: expected this service's one foreign-token sentence, got {refused}"
        );

        // And the refusal changed nothing: the other selection still reports every
        // participant it held, the would-be omitted ones included.
        let still_holds = fixture.selects(case, &row.resumed).await;
        assert_eq!(
            still_holds, elsewhere_holds,
            "case {case:?}: the refusal must leave the other selection's participants as \
             they were, expected {elsewhere_holds:?}, got {still_holds:?}"
        );
    }
}

/// Paging a filtered listing to its end reaches every participant the filter selects and
/// no other — the invariant the refusal above protects, read as a client walks it.
#[tokio::test]
async fn paging_one_filter_selection_to_its_end_reaches_every_participant_it_selects() {
    let fixture = Fixture::new().await;

    for row in rows() {
        let case = row.case;
        for selection in [&row.issuer, &row.resumed] {
            let expected = fixture.selects(case, selection).await;

            // A walk at one participant per page, bounded by the participants the session
            // holds so a token that never advanced cannot loop here.
            let mut walked: Vec<String> = Vec::new();
            let mut next = String::new();
            for _ in 0..=expected.len() {
                let mut request = merged(selection, &json!({ "maxResults": 1 }));
                if !next.is_empty() {
                    request["nextToken"] = json!(next);
                }
                let response = fixture
                    .service
                    .served("ListParticipants", &merged(&fixture.base(), &request))
                    .await;
                walked.extend(listed_ids(case, &response));
                next = response
                    .get("nextToken")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if next.is_empty() {
                    break;
                }
            }
            assert!(
                next.is_empty(),
                "case {case:?}: the walk of {selection} must end without a nextToken, got \
                 {next:?} after {walked:?}"
            );
            assert_eq!(
                walked, expected,
                "case {case:?}: paging {selection} must reach exactly what it selects, \
                 expected {expected:?}, got {walked:?}"
            );
        }
    }
}
