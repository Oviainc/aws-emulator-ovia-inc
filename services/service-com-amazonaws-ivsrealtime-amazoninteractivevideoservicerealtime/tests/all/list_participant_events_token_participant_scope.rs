// SPDX-License-Identifier: CC0-1.0

//! A `ListParticipantEvents` `nextToken` names a position among the occurrences of ONE
//! participant of ONE stage session, so only that same participant's listing can resume it.
//!
//! The rule, over a set of inputs rather than on one request: a page position is an ordinal
//! among the records the resuming request's own scope selects — here the region, the stage,
//! the session and the PARTICIPANT whose events the page held — so a token offered under any
//! other scope names no position among the records that call would page, and is refused with
//! the modelled `ValidationException` (HTTP 400) carrying this service's one foreign-token
//! sentence. It is never served as a page that silently omits the occurrences lying before
//! the position it names.
//!
//! `tests/all/page_token_scope.rs` pins that rule across every listing of this service,
//! varying the region and the stage/session terms. The rows here vary the term those rows do
//! not reach for this listing: the `participantId`, with the region, the `stageArn` and the
//! `sessionId` held fixed — a SIBLING participant of the very same session. That is the
//! sharpest case, because one session's occurrences are interleaved in a single increasing
//! order: a position issued while paging one participant sits in the middle of another's, so
//! an unscoped token would answer a short page, or an empty one with no `nextToken`, which a
//! client reads as "this participant has no (more) events".
//!
//! Rows differ along what the rule generalises over: which sibling resumes the token, how
//! many occurrences that sibling holds (one, three, four), whether its occurrences lie
//! BEFORE the position the token names (the direction that turns into a short page), AFTER
//! it, straddling it, or past it entirely (the empty page), and a well-formed
//! `participantId` the session holds no occurrence of at all — where the refusal is the only
//! thing that tells a foreign token from a genuinely empty page.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a participant
//! joins over the stage's data plane with the participant token it was issued, which is
//! `state::join_stage_session` / `state::leave_stage_session`. So, like the sibling
//! `ListParticipantEvents` suite, the fixture brings participants into one session through
//! that seam and asserts everything else over the router's own `dispatch`, as an SDK client
//! sees it.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region every call of this suite is signed for.
const REGION: &str = "us-east-1";

/// A well-formed `ParticipantId` this store holds no occurrence of (the modelled
/// `ParticipantId` is `@length 0..64` and `@pattern ^[a-zA-Z0-9-_]*$`, from
/// `smithy_query(query="operation",operation="ListParticipantEvents")`).
const ABSENT_PARTICIPANT: &str = "no-such-participant";

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches it,
/// plus the engine handle the data-plane seeding writes through.
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
    /// naming `ivs` in [`REGION`].
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

    /// Make a call, asserting it was served, and answer with the response body.
    async fn served(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "fixture {operation} {body}: expected HTTP 200, got {status} {response}"
        );
        response
    }

    /// A stage, created the way a client creates one, reported by its ARN.
    async fn stage(&self, name: &str) -> String {
        let response = self.served("CreateStage", &json!({ "name": name })).await;
        text(&response["stage"], "arn")
    }

    /// A participant of `stage_arn`: the id a token carries, which is where the model says a
    /// `participantId` comes from.
    async fn participant(&self, stage_arn: &str) -> String {
        let response = self
            .served("CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        text(&response["participantToken"], "participantId")
    }

    /// The stage's `activeSessionId`, read back over the wire.
    async fn active_session(&self, stage_arn: &str) -> String {
        let response = self.served("GetStage", &json!({ "arn": stage_arn })).await;
        text(&response["stage"], "activeSessionId")
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does. A
    /// publishing join records the arrival and the start of its media; a subscribing one
    /// records the arrival alone.
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

    /// One `ListParticipantEvents` page for one participant of one session, with any extra
    /// request members (`maxResults`, `nextToken`) merged in. Answers the status and the body
    /// unchanged.
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
        if let Some(extra) = extra.as_object() {
            let target = request
                .as_object_mut()
                .expect("the request under construction is an object");
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        self.call("ListParticipantEvents", &request).await
    }

    /// The names one served page reports, in page order — the identity of an occurrence
    /// inside one participant's listing.
    async fn page(
        &self,
        case: &str,
        stage_arn: &str,
        session_id: &str,
        participant_id: &str,
        extra: &Value,
    ) -> Vec<String> {
        let (status, response) = self
            .list(stage_arn, session_id, participant_id, extra)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListParticipantEvents with {extra} must answer HTTP 200, \
             got {status} {response}"
        );
        response["events"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: events must be a list, got {response}"))
            .iter()
            .map(|event| text(event, "name"))
            .collect()
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

/// The sentence every listing of this service refuses a token naming no position in it with
/// — the service's one wording, kept here as a client reads it.
fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// The occurrence names a page reports, as owned strings, so a row's expectation reads as a
/// plain list.
fn names(expected: &[&str]) -> Vec<String> {
    expected.iter().map(|name| (*name).to_string()).collect()
}

/// One row: the participant whose page issues the token (and how long that page is), and the
/// participant of the SAME session the token is then offered to, with what each listing holds
/// of its own.
struct Row {
    case: &'static str,
    issuer: String,
    issuer_holds: Vec<String>,
    /// The `maxResults` of the issuing page, which decides WHERE in the session's single
    /// occurrence order the token's position falls relative to the resuming participant's
    /// occurrences.
    page_size: usize,
    resuming: String,
    resuming_holds: Vec<String>,
}

/// A token one participant's page issued names no position among ANOTHER participant's
/// occurrences of the same session, so that listing refuses it — and goes on resuming it for
/// the participant that issued it.
#[tokio::test]
async fn a_participant_event_token_resumes_only_the_participant_that_issued_it() {
    let service = RealTime::new();
    let stage = service.stage("one-session-many-participants").await;

    // Four participants of ONE session, brought in and out in an order that interleaves
    // their occurrences in the session's single increasing order:
    //
    //   publisher  JOINED PUBLISH_STARTED . . . . . PUBLISH_STOPPED LEFT . . . .
    //   co_host    . . . . JOINED PUBLISH_STARTED . . . . . . . . . PUBLISH_STOPPED LEFT
    //   rejoiner   . . . . . . . JOINED . . . LEFT . . . . JOINED . . . . . . . .
    //   viewer     . . . . . . . . . . . . . . . . JOINED . . . . . . . . . . . .
    //
    // so each participant's occurrences lie partly below and partly above another's, which is
    // exactly the interleaving a per-participant position has to be read inside.
    let publisher = service.participant(&stage).await;
    let co_host = service.participant(&stage).await;
    let rejoiner = service.participant(&stage).await;
    let viewer = service.participant(&stage).await;

    service.join(&stage, &publisher, true);
    let session = service.active_session(&stage).await;
    service.join(&stage, &co_host, true);
    service.join(&stage, &rejoiner, false);
    service.leave(&stage, &session, &publisher);
    service.leave(&stage, &session, &rejoiner);
    service.join(&stage, &viewer, false);
    // A rejoin of the same participant in the same session: more occurrences under the SAME
    // scope, so its own token must go on resuming it.
    service.join(&stage, &rejoiner, false);
    service.leave(&stage, &session, &co_host);

    let publisher_holds = names(&["JOINED", "PUBLISH_STARTED", "PUBLISH_STOPPED", "LEFT"]);
    let co_host_holds = names(&["JOINED", "PUBLISH_STARTED", "PUBLISH_STOPPED", "LEFT"]);
    let rejoiner_holds = names(&["JOINED", "LEFT", "JOINED"]);
    let viewer_holds = names(&["JOINED"]);

    // The rows differ along which sibling of the session resumes the token, how many
    // occurrences it holds, and where the token's position falls among them.
    let rows = vec![
        Row {
            case: "a sibling whose occurrences all lie AFTER the position",
            issuer: publisher.clone(),
            issuer_holds: publisher_holds.clone(),
            page_size: 1,
            resuming: co_host.clone(),
            resuming_holds: co_host_holds.clone(),
        },
        Row {
            case: "a sibling whose occurrences STRADDLE the position",
            issuer: publisher.clone(),
            issuer_holds: publisher_holds.clone(),
            page_size: 3,
            resuming: co_host.clone(),
            resuming_holds: co_host_holds.clone(),
        },
        Row {
            case: "a sibling whose first occurrences lie BEFORE the position",
            issuer: co_host.clone(),
            issuer_holds: co_host_holds.clone(),
            page_size: 2,
            resuming: publisher.clone(),
            resuming_holds: publisher_holds.clone(),
        },
        Row {
            case: "a sibling holding a single occurrence, lying PAST the position",
            issuer: co_host.clone(),
            issuer_holds: co_host_holds.clone(),
            page_size: 3,
            resuming: viewer.clone(),
            resuming_holds: viewer_holds.clone(),
        },
        Row {
            case: "a sibling holding a single occurrence, lying AFTER the position",
            issuer: publisher.clone(),
            issuer_holds: publisher_holds.clone(),
            page_size: 2,
            resuming: viewer.clone(),
            resuming_holds: viewer_holds.clone(),
        },
        Row {
            case: "a sibling that rejoined, whose occurrences straddle the position",
            issuer: publisher.clone(),
            issuer_holds: publisher_holds.clone(),
            page_size: 1,
            resuming: rejoiner.clone(),
            resuming_holds: rejoiner_holds.clone(),
        },
        Row {
            case: "a token issued by a participant that rejoined the same session",
            issuer: rejoiner.clone(),
            issuer_holds: rejoiner_holds.clone(),
            page_size: 2,
            resuming: publisher.clone(),
            resuming_holds: publisher_holds.clone(),
        },
        Row {
            case: "a participant of this session the store holds no occurrence of",
            issuer: publisher.clone(),
            issuer_holds: publisher_holds.clone(),
            page_size: 3,
            resuming: ABSENT_PARTICIPANT.to_string(),
            resuming_holds: Vec::new(),
        },
    ];

    for row in rows {
        let case = row.case;
        assert!(
            row.page_size < row.issuer_holds.len(),
            "case {case:?}: the issuing page must leave an occurrence behind for a token to \
             be issued, got a page of {} over {:?}",
            row.page_size,
            row.issuer_holds
        );

        // What the resuming participant's listing holds before any token is in play.
        let before = service
            .page(case, &stage, &session, &row.resuming, &json!({}))
            .await;
        assert_eq!(
            before, row.resuming_holds,
            "case {case:?}: expected the resuming participant to hold {:?}, got {before:?}",
            row.resuming_holds
        );

        // A short page for the issuing participant, and the token it ends with.
        let (status, page) = service
            .list(
                &stage,
                &session,
                &row.issuer,
                &json!({ "maxResults": row.page_size }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page of {} must be served, got {status} {page}",
            row.page_size
        );
        let token = text(&page, "nextToken");
        assert!(
            token != "<absent>" && !token.is_empty(),
            "case {case:?}: a page with occurrences left over must carry a nextToken, got {page}"
        );

        // Inside the participant that issued it, the token resumes exactly where that page
        // ended.
        let resumed = service
            .page(
                case,
                &stage,
                &session,
                &row.issuer,
                &json!({ "nextToken": token }),
            )
            .await;
        assert_eq!(
            resumed,
            row.issuer_holds[row.page_size..].to_vec(),
            "case {case:?}: the issuing participant must resume after the occurrence its page \
             ended at, expected {:?}, got {resumed:?}",
            &row.issuer_holds[row.page_size..]
        );

        // Offered to ANOTHER participant of the same session, it names no position among the
        // occurrences that call pages, so it is refused — not served as a short or empty page.
        let (status, refused) = service
            .list(
                &stage,
                &session,
                &row.resuming,
                &json!({ "nextToken": token }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token of another participant must be refused, got {status} {refused}"
        );
        assert_eq!(
            text(&refused, "Code"),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {refused}"
        );
        assert_eq!(
            text(&refused, "Message"),
            foreign_page_token_message("ListParticipantEvents"),
            "case {case:?}: expected this service's one foreign-token sentence, got {refused}"
        );
        assert!(
            refused.get("events").is_none(),
            "case {case:?}: a refused page must report no events, got {refused}"
        );

        // And the refusal changed nothing a later caller sees.
        let after = service
            .page(case, &stage, &session, &row.resuming, &json!({}))
            .await;
        assert_eq!(
            after, before,
            "case {case:?}: the refusal must leave the resuming participant's occurrences as \
             they were, expected {before:?}, got {after:?}"
        );
    }
}
