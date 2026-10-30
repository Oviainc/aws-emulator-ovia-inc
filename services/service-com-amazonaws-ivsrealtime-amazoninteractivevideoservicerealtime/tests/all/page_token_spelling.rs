// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` is a position only when it is spelt as the listing that issued it spells
//! one.
//!
//! The rule, stated once and then exercised over a SET of inputs: a listing of this
//! service reads a position out of a `nextToken` only when that token is exactly one its
//! own encoder could have handed back — the position written the single way the encoder
//! writes a number, and a position the store has really handed out (its sequences count
//! up from one, so nothing below one and nothing above the last one drawn is a position
//! any page ended at). Every other token names no position, so it is refused with the
//! modelled `ValidationException` (HTTP 400) carrying this service's one foreign-token
//! sentence, rather than served a page — a resumed one, or the silently empty page a
//! position past the end would otherwise answer with.
//!
//! Rows differ along what the rule generalises over: the listing (so the scope marker the
//! token carries — account-wide, stage-scoped, session-scoped, participant-scoped, and the
//! markers that carry filter members too), HOW the position is spelt (a leading `+`, one
//! and several leading zeros, trailing space, digit grouping), and WHICH position it names
//! (zero, one past the last the store drew, the largest `u64`, one past that type
//! altogether) — plus the other half of the rule, the spellings a listing really does
//! issue: its own token, and the lowest and the highest position the store has drawn, each
//! written as the encoder writes it.
//!
//! The fixture holds no composition and no replication, so no record of it has a clock-due
//! change: the positions the store has drawn stand still while the table runs, and the
//! bound rows below are about exactly the range the fixture built.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the token it was issued, which is
//! `state::join_stage_session` — so, like the sibling page-token suites, the fixture brings
//! participants into a session through that seam and asserts everything else over the
//! router's own `dispatch`, as an SDK client sees it.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region this suite signs for: the scope every listing of this service pages within.
const REGION: &str = "us-east-1";

/// How many records the fixture puts in each scope: more than one, so every listing hands
/// back a token of its own at `maxResults: 1`, and the resumed page is not empty.
const FIXTURE_RECORDS: usize = 3;

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

    /// Make the call `operation`, asserting it was served, and answer with its body.
    async fn served(&self, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "fixture {operation} {body}: expected HTTP 200, got {status} {response}"
        );
        response
    }

    /// Create one resource through `operation` and answer with the ARN it minted, read off
    /// the response member `member`.
    async fn created(&self, operation: &str, body: &Value, member: &str) -> String {
        let response = self.served(operation, body).await;
        text(&response[member], "arn")
    }

    /// Issue a participant token for `stage_arn` and answer with the participant id it
    /// carries — the id the holder of that token joins the stage under.
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

    /// Record a publishing participant joining `stage_arn`, the way the stage's data plane
    /// does. A publisher, so the join records both the arrival and the start of its media:
    /// more than one occurrence, which is what lets `ListParticipantEvents` page.
    fn join(&self, stage_arn: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::join_stage_session(
                &mut store,
                stage_arn,
                state::JoinedParticipant {
                    participant_id: participant_id.to_string(),
                    published: true,
                    attributes: HashMap::new(),
                    ..Default::default()
                },
            )
            .is_some(),
            "the join of {participant_id:?} on {stage_arn:?} must be recorded"
        );
    }

    /// Record a participant leaving a session, the way the stage's data plane does. When
    /// the last one leaves the session ends, and the next join opens another.
    fn leave(&self, stage_arn: &str, session_id: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::leave_stage_session(&mut store, stage_arn, session_id, participant_id),
            "the departure of {participant_id:?} from {session_id:?} must be recorded"
        );
    }

    /// The highest position this store has drawn: the last sequence any record of it was
    /// given, which is the top of the range a page token can name.
    fn issued_through(&self) -> u64 {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let store = service_state.lock();
        store.issued_through()
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

/// The error code a refusal states, or `"<absent>"` so a failing assertion names the
/// missing member rather than panicking inside the reader.
fn error_code(response: &Value) -> &str {
    response
        .get("Code")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The text a refusal states.
fn error_message(response: &Value) -> &str {
    response
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The sentence every listing of this service refuses a token naming no position in it
/// with — the service's one wording, kept here as a client reads it.
fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// One listing of this service: the operation, where its records and their identities live
/// in the response, and the identity members a call to it carries.
struct Listing {
    operation: &'static str,
    /// The response member holding the page's records.
    records: &'static str,
    /// The member of a record this suite reads its identity off.
    identity: &'static str,
    scope: Value,
}

impl Listing {
    /// This listing's request with `extra` merged in (the `maxResults` and `nextToken` a
    /// case supplies).
    fn request(&self, extra: &Value) -> Value {
        let mut request = self.scope.clone();
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        request
    }

    /// The identities one page reports, asserting the call was served.
    async fn identities(&self, service: &RealTime, case: &str, extra: &Value) -> Vec<String> {
        let (status, response) = service.call(self.operation, &self.request(extra)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {} {} must answer HTTP 200, got {status} {response}",
            self.operation,
            self.request(extra)
        );
        // A listing with nothing left to report may omit its record member altogether,
        // which is the empty page a client reads it as.
        let records = match response.get(self.records) {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(records)) => records.clone(),
            Some(other) => panic!(
                "case {case:?}: {} must report its records under {:?} as a list, got {other}",
                self.operation, self.records
            ),
        };
        records
            .iter()
            .map(|record| text(record, self.identity))
            .collect()
    }

    /// The token this listing really issues at the end of a one-record page.
    async fn issued_token(&self, service: &RealTime) -> String {
        let request = self.request(&json!({ "maxResults": 1 }));
        let response = service.served(self.operation, &request).await;
        response["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "fixture {} {request}: expected a nextToken while records remain, got \
                     {response}",
                    self.operation
                )
            })
            .to_string()
    }
}

/// The payload of a token, which the codec writes as the scope marker followed by the
/// position: the marker (up to and including its final `:`) and the position after it.
fn marker_of(case: &str, token: &str) -> String {
    let decoded = ::aws_smithy_types::base64::decode(token)
        .unwrap_or_else(|error| panic!("case {case:?}: an issued token is base64: {error}"));
    let payload = String::from_utf8(decoded)
        .unwrap_or_else(|error| panic!("case {case:?}: an issued token's payload is text: {error}"));
    let (marker, position) = payload.rsplit_once(':').unwrap_or_else(|| {
        panic!("case {case:?}: an issued token's payload carries a scope marker, got {payload:?}")
    });
    assert!(
        position.parse::<u64>().is_ok(),
        "case {case:?}: an issued token's position is a plain number, got {position:?}"
    );
    format!("{marker}:")
}

/// A token spelling `position` under `marker`, built exactly as the service builds one.
fn token_for(marker: &str, position: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{marker}{position}"))
}

/// Every listing this suite drives, over the fixture's records: the scopes a page token of
/// this service is issued in, from the account-wide listings to the participant-scoped one.
async fn fixture() -> (RealTime, Vec<Listing>) {
    let service = RealTime::new();

    // The account-wide scopes: more than one record each.
    let mut stages = Vec::new();
    for index in 0..FIXTURE_RECORDS {
        stages.push(
            service
                .created(
                    "CreateStage",
                    &json!({ "name": format!("stage-{index}") }),
                    "stage",
                )
                .await,
        );
        service
            .created(
                "CreateStorageConfiguration",
                &json!({ "s3": { "bucketName": format!("bucket-{index}") } }),
                "storageConfiguration",
            )
            .await;
        service
            .created(
                "CreateEncoderConfiguration",
                &json!({ "name": format!("encoder-{index}") }),
                "encoderConfiguration",
            )
            .await;
        service
            .created(
                "CreateIngestConfiguration",
                &json!({ "name": format!("ingest-{index}"), "ingestProtocol": "RTMPS" }),
                "ingestConfiguration",
            )
            .await;
    }
    let stage = stages
        .first()
        .cloned()
        .expect("the fixture created a stage to scope the inner listings to");

    // A session that has ENDED, so the stage has more than one session to page over: its
    // participants joined and then left, which closes it.
    let departed = service.participant(&stage).await;
    service.join(&stage, &departed);
    let ended_session = service.active_session(&stage).await;
    service.leave(&stage, &ended_session, &departed);

    // The live session the next joins open, holding more than one participant, each
    // publishing — so a participant's own event list pages too.
    let mut participants = Vec::new();
    for _ in 0..FIXTURE_RECORDS {
        let participant = service.participant(&stage).await;
        service.join(&stage, &participant);
        participants.push(participant);
    }
    let live_session = service.active_session(&stage).await;
    assert_ne!(
        ended_session, live_session,
        "fixture: the session the departure closed and the one the later joins opened must \
         be two sessions"
    );
    let publisher = participants
        .first()
        .cloned()
        .expect("the live session holds participants");

    let listings = vec![
        Listing {
            operation: "ListStages",
            records: "stages",
            identity: "arn",
            scope: json!({}),
        },
        Listing {
            operation: "ListStorageConfigurations",
            records: "storageConfigurations",
            identity: "arn",
            scope: json!({}),
        },
        Listing {
            operation: "ListEncoderConfigurations",
            records: "encoderConfigurations",
            identity: "arn",
            scope: json!({}),
        },
        Listing {
            operation: "ListIngestConfigurations",
            records: "ingestConfigurations",
            identity: "arn",
            scope: json!({}),
        },
        Listing {
            operation: "ListStageSessions",
            records: "stageSessions",
            identity: "sessionId",
            scope: json!({ "stageArn": stage }),
        },
        Listing {
            operation: "ListParticipants",
            records: "participants",
            identity: "participantId",
            scope: json!({ "stageArn": stage, "sessionId": live_session }),
        },
        Listing {
            operation: "ListParticipantEvents",
            records: "events",
            identity: "name",
            scope: json!({
                "stageArn": stage,
                "sessionId": live_session,
                "participantId": publisher,
            }),
        },
    ];
    (service, listings)
}

/// A token whose position is spelt any way but the one the encoder spells it, or names a
/// position the store never drew, is refused — in every listing of this service.
#[tokio::test]
async fn a_position_the_encoder_never_wrote_is_no_page_position() {
    let (service, listings) = fixture().await;

    for listing in &listings {
        let case = listing.operation;
        let issued = listing.issued_token(&service).await;
        let marker = marker_of(case, &issued);
        // The top of the range the store has drawn, read while the table runs: the fixture
        // holds nothing the clock changes, so it stands still across these calls.
        let issued_through = service.issued_through();

        // How the payload fails to be a position this listing could have issued: a
        // spelling of a number the encoder never writes, and a number the store never
        // drew — on both ends of the range it has drawn, one step outside each.
        let refused: Vec<(&str, String)> = vec![
            ("a leading plus", "+1".to_string()),
            ("one leading zero", "01".to_string()),
            ("three leading zeros", "0001".to_string()),
            (
                "a leading zero on the highest position drawn",
                format!("0{issued_through}"),
            ),
            ("a trailing space", "1 ".to_string()),
            ("a leading space", " 1".to_string()),
            ("digit grouping", "1_0".to_string()),
            (
                "position zero, one below the lowest the store draws",
                "0".to_string(),
            ),
            (
                "one past the highest position the store has drawn",
                (issued_through + 1).to_string(),
            ),
            (
                "the largest u64, which no store of this size drew",
                u64::MAX.to_string(),
            ),
            ("one past the largest u64", "18446744073709551616".to_string()),
            ("no position at all", String::new()),
        ];

        let expected = foreign_page_token_message(listing.operation);
        for (how, payload) in refused {
            let case = format!("{} / {how}", listing.operation);
            let token = token_for(&marker, &payload);
            let request = listing.request(&json!({ "nextToken": token }));
            let (status, response) = service.call(listing.operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for the token spelling {payload:?}, got \
                 {status} {response}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                expected,
                "case {case:?}: expected the one foreign-token sentence {expected:?}, got {}",
                error_message(&response)
            );
        }
    }
}

/// The other half of the rule: a token whose position IS spelt the way the encoder spells
/// one, naming a position the store has drawn, resumes the listing — its own token, and
/// both ends of the range the store has drawn.
#[tokio::test]
async fn a_position_the_encoder_writes_resumes_the_listing() {
    let (service, listings) = fixture().await;

    for listing in &listings {
        let case = listing.operation;
        let issued = listing.issued_token(&service).await;
        let marker = marker_of(case, &issued);
        let issued_through = service.issued_through();

        // The token this listing handed back resumes it: the page it answers holds the
        // records the first page did not, and repeats none of them.
        let first = listing
            .identities(&service, case, &json!({ "maxResults": 1 }))
            .await;
        let resumed = listing
            .identities(&service, case, &json!({ "nextToken": issued.clone() }))
            .await;
        assert_eq!(
            first.len(),
            1,
            "case {case:?}: a one-record page holds one record, got {first:?}"
        );
        assert!(
            !resumed.is_empty(),
            "case {case:?}: the issued token must resume the records left over, got {resumed:?}"
        );
        assert!(
            resumed.iter().all(|record| !first.contains(record)),
            "case {case:?}: the resumed page must repeat no record of the first, got \
             {resumed:?} after {first:?}"
        );

        // Both ends of the range the store has drawn, each written as the encoder writes
        // it: a position this listing's own pages can have ended at, so it is read rather
        // than refused — the lowest one leaves every later record to report, and the
        // highest one leaves none.
        for (how, position) in [
            ("the lowest position the store draws", 1),
            ("the highest position the store has drawn", issued_through),
        ] {
            let case = format!("{} / {how}", listing.operation);
            let token = token_for(&marker, &position.to_string());
            let request = listing.request(&json!({ "nextToken": token }));
            let (status, response) = service.call(listing.operation, &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a position the store drew, spelt as the encoder spells it, \
                 names a page, got {status} {response}"
            );
        }
        let beyond = listing
            .identities(
                &service,
                "the highest position the store has drawn",
                &json!({ "nextToken": token_for(&marker, &issued_through.to_string()) }),
            )
            .await;
        assert!(
            beyond.is_empty(),
            "case {case:?}: no record of this listing lies after the last position the \
             store drew, got {beyond:?}"
        );
    }
}
