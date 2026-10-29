// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` names a position among the records ONE request's terms select, and a
//! `filterBy*` member is such a term.
//!
//! The rule, stated once and then exercised over a SET of inputs: every listing of this
//! service pages over the records its request selects — the region it is processed in, the
//! identity members it carries, AND every filter member it carries, since the filters are
//! applied before the resume position is compared. The `nextToken` such a page hands back
//! therefore names a position only under that same selection: offered under any other one
//! it names no position among the records that call would page, so it is refused with the
//! modelled `ValidationException` (HTTP 400) carrying this service's one foreign-token
//! sentence, and never served as a short or empty page that silently omits every record
//! lying before that position.
//!
//! Rows differ along what the rule generalises over: the listing (every listing of this
//! service that has filter members), WHICH filter member the resuming request changes, and
//! what it changes it to — another value of the same filter, a different filter member, or
//! no filter at all. Each row's resuming selection holds a record the issuing page's
//! position lies past, so a 200 there would be the short page the rule forbids.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage or LEAVE it:
//! that is the stage's data plane, which is `state::join_stage_session` /
//! `state::leave_stage_session` — so, like the sibling `ListParticipants` suites, the
//! fixture brings participants into a session through that seam and asserts everything
//! else over the router's own `dispatch`, as an SDK client sees it.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The region this suite signs for: the term every listing of this service pages within
/// beside its own members.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the broadcast destination built by hand below carries it too.
const ACCOUNT: &str = "000000000000";

/// The two values the modelled `IngestConfigurationState` carries, from
/// `smithy_query(query="shape",shape="IngestConfigurationState")`.
const INGEST_ACTIVE: &str = "ACTIVE";
const INGEST_INACTIVE: &str = "INACTIVE";

/// Two values of the modelled `ParticipantState`, from
/// `smithy_query(query="shape",shape="ParticipantState")`.
const PARTICIPANT_CONNECTED: &str = "CONNECTED";
const PARTICIPANT_DISCONNECTED: &str = "DISCONNECTED";

/// Two values of the modelled `ParticipantRecordingFilterByRecordingState`, from
/// `smithy_query(query="shape",shape="ParticipantRecordingFilterByRecordingState")` —
/// whose five values are `STARTING | ACTIVE | STOPPING | STOPPED | FAILED`.
const RECORDING_ACTIVE: &str = "ACTIVE";
const RECORDING_STOPPED: &str = "STOPPED";

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

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in [`REGION`] — which is how a request names the region it is
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

    /// Create one resource through `operation` and answer with the ARN the service minted
    /// for it, read off the response member `member`.
    async fn created(&self, operation: &str, body: &Value, member: &str) -> String {
        let response = self.served(operation, body).await;
        text(&response[member], "arn")
    }

    /// Create a stage.
    async fn stage(&self, name: &str) -> String {
        self.created("CreateStage", &json!({ "name": name }), "stage")
            .await
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

/// `left` with every member of `right` merged in (the `maxResults` and `nextToken` a case
/// supplies, over a row's own selection).
fn merged(left: &Value, right: &Value) -> Value {
    let mut request = left.clone();
    if let Some(members) = right.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    request
}

/// One row of the table: a listing, the selection that issues a token, and the selection
/// the token is then offered to.
struct Row {
    case: String,
    operation: &'static str,
    /// The response member holding the page's records.
    records: &'static str,
    /// The member of a record this suite reads its identity off.
    identity: &'static str,
    /// The identity members every call of this row carries.
    base: Value,
    /// The filter members the issuing call carries.
    issuer: Value,
    /// The filter members the resuming call carries instead.
    resumed: Value,
}

/// The records one call of `row` reports, in the order it reported them, asserting it was
/// served.
async fn identities(service: &RealTime, row: &Row, selection: &Value) -> Vec<String> {
    let request = merged(&row.base, selection);
    let case = &row.case;
    let response = service.served(row.operation, &request).await;
    let records = response[row.records]
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} must report its records under {:?}, got {response}",
                row.operation, row.records
            )
        });
    records
        .iter()
        .map(|record| text(record, row.identity))
        .collect()
}

/// Everything the table below pages over, created through this service's own API (and, for
/// the participants, through the stage data plane that records a join).
struct Fixture {
    service: RealTime,
    /// The two stages whose ingest configurations the first rows page over, the first
    /// created (and so holding the LOWER store positions).
    ingest_stages: [String; 2],
    /// The two encoder configurations the compositions use, and the stage of the
    /// composition that uses the first of them.
    first_encoder: String,
    second_encoder: String,
    composition_stage: String,
    /// The stage session whose participants the last rows page over.
    participant_stage: String,
    participant_session: String,
}

impl Fixture {
    async fn new() -> Self {
        let service = RealTime::new();

        // Two ingest configurations per stage, the first stage's created first so its
        // configurations hold the lower store positions — the arrangement under which an
        // unscoped token resumed under the other stage's filter drops records.
        let ingest_stages = [
            service.stage("ingest-first").await,
            service.stage("ingest-second").await,
        ];
        for (index, stage) in ingest_stages.iter().enumerate() {
            for which in ["primary", "secondary"] {
                let configuration = service
                    .created(
                        "CreateIngestConfiguration",
                        &json!({
                            "name": format!("ingest-{index}-{which}"),
                            "stageArn": stage,
                            "ingestProtocol": "RTMPS",
                        }),
                        "ingestConfiguration",
                    )
                    .await;
                // One configuration of each stage gets a publisher broadcasting with it,
                // which is what makes it report ACTIVE while the other stays INACTIVE: a
                // publisher reaches the stage with the configuration's own ARN.
                if which == "primary" {
                    service.join(
                        stage,
                        state::JoinedParticipant {
                            participant_id: format!("broadcaster-{index}"),
                            published: true,
                            ingest_configuration_arn: Some(configuration),
                            ..Default::default()
                        },
                    );
                }
            }
        }

        // Three compositions: two through the FIRST encoder configuration (so a page
        // filtered by it has a record left over), one through the second.
        let mut encoders = Vec::new();
        for name in ["encoder-first", "encoder-second"] {
            encoders.push(
                service
                    .created(
                        "CreateEncoderConfiguration",
                        &json!({ "name": name }),
                        "encoderConfiguration",
                    )
                    .await,
            );
        }
        let mut composition_stages = Vec::new();
        for (index, encoder) in [0usize, 0, 1].into_iter().enumerate() {
            let stage = service.stage(&format!("composed-{index}")).await;
            service
                .served(
                    "StartComposition",
                    &json!({
                        "stageArn": stage,
                        "destinations": [{
                            "channel": {
                                "channelArn": format!("arn:aws:ivs:{REGION}:{ACCOUNT}:channel/composed"),
                                "encoderConfigurationArn": encoders[encoder],
                            },
                        }],
                    }),
                )
                .await;
            composition_stages.push(stage);
        }

        // One session holding participants that differ in every member a
        // `ListParticipants` filter selects on: the user id they joined under, whether
        // they published, what the recorder reports for them, and whether they are still
        // connected.
        let participant_stage = service.stage("participants").await;
        for (participant, user, published, recording) in [
            ("alice-one", "alice", true, Some(RECORDING_ACTIVE)),
            ("alice-two", "alice", true, Some(RECORDING_ACTIVE)),
            ("bob-one", "bob", false, Some(RECORDING_STOPPED)),
            ("bob-two", "bob", false, Some(RECORDING_STOPPED)),
        ] {
            service.join(
                &participant_stage,
                state::JoinedParticipant {
                    participant_id: participant.to_string(),
                    user_id: Some(user.to_string()),
                    published,
                    recording_state: recording.map(str::to_string),
                    ..Default::default()
                },
            );
        }
        let participant_session = service.active_session(&participant_stage).await;
        // One participant leaves, so the session holds both states the modelled
        // `ParticipantState` carries.
        service.leave(&participant_stage, &participant_session, "bob-two");

        Self {
            service,
            ingest_stages,
            first_encoder: encoders[0].clone(),
            second_encoder: encoders[1].clone(),
            composition_stage: composition_stages[0].clone(),
            participant_stage,
            participant_session,
        }
    }

    /// The table: for every listing of this service that has filter members
    /// (`smithy_query(query="operation",operation="ListIngestConfigurations,ListCompositions,
    /// ListParticipants")` — the only three), each filter member a resuming request can
    /// change, changed to another value of itself, to another filter member, and to no
    /// filter at all.
    fn rows(&self) -> Vec<Row> {
        let ingest = |case: &str, issuer: Value, resumed: Value| Row {
            case: format!("ListIngestConfigurations / {case}"),
            operation: "ListIngestConfigurations",
            records: "ingestConfigurations",
            identity: "arn",
            base: json!({}),
            issuer,
            resumed,
        };
        let compositions = |case: &str, issuer: Value, resumed: Value| Row {
            case: format!("ListCompositions / {case}"),
            operation: "ListCompositions",
            records: "compositions",
            identity: "arn",
            base: json!({}),
            issuer,
            resumed,
        };
        let participants = |case: &str, issuer: Value, resumed: Value| Row {
            case: format!("ListParticipants / {case}"),
            operation: "ListParticipants",
            records: "participants",
            identity: "participantId",
            base: json!({
                "stageArn": self.participant_stage,
                "sessionId": self.participant_session,
            }),
            issuer,
            resumed,
        };
        let [first_ingest_stage, second_ingest_stage] = &self.ingest_stages;

        vec![
            // `filterByStageArn`: the stage whose configurations the page holds. The
            // issuing stage is the one created SECOND, so its position lies past the other
            // stage's configurations entirely — the empty page the rule forbids.
            ingest(
                "another stage's configurations",
                json!({ "filterByStageArn": second_ingest_stage }),
                json!({ "filterByStageArn": first_ingest_stage }),
            ),
            ingest(
                "every configuration of the account",
                json!({ "filterByStageArn": second_ingest_stage }),
                json!({}),
            ),
            ingest(
                "a stage's configurations, resumed from the whole account",
                json!({}),
                json!({ "filterByStageArn": first_ingest_stage }),
            ),
            // `filterByState`: the state the configurations report right now.
            ingest(
                "the configurations in another state",
                json!({ "filterByState": INGEST_INACTIVE }),
                json!({ "filterByState": INGEST_ACTIVE }),
            ),
            ingest(
                "another filter member entirely",
                json!({ "filterByState": INGEST_INACTIVE }),
                json!({ "filterByStageArn": first_ingest_stage }),
            ),
            // `filterByEncoderConfigurationArn` and `filterByStageArn` of the composition
            // listing.
            compositions(
                "another encoder configuration's compositions",
                json!({ "filterByEncoderConfigurationArn": self.first_encoder }),
                json!({ "filterByEncoderConfigurationArn": self.second_encoder }),
            ),
            compositions(
                "every composition of the account",
                json!({ "filterByEncoderConfigurationArn": self.first_encoder }),
                json!({}),
            ),
            compositions(
                "one stage's compositions, resumed from the whole account",
                json!({}),
                json!({ "filterByStageArn": self.composition_stage }),
            ),
            // The four filter members of the participant listing.
            participants(
                "another user's participants",
                json!({ "filterByUserId": "alice" }),
                json!({ "filterByUserId": "bob" }),
            ),
            participants(
                "every participant of the session",
                json!({ "filterByUserId": "alice" }),
                json!({}),
            ),
            participants(
                "the participants that published, resumed from all of them",
                json!({ "filterByPublished": true }),
                json!({}),
            ),
            participants(
                "the participants that published, resumed under another filter",
                json!({ "filterByPublished": true }),
                json!({ "filterByUserId": "bob" }),
            ),
            participants(
                "the participants in another state",
                json!({ "filterByState": PARTICIPANT_CONNECTED }),
                json!({ "filterByState": PARTICIPANT_DISCONNECTED }),
            ),
            participants(
                "the participants in another recording state",
                json!({ "filterByRecordingState": RECORDING_ACTIVE }),
                json!({ "filterByRecordingState": RECORDING_STOPPED }),
            ),
        ]
    }
}

/// A token one filter selection issued names no position under another, so the listing
/// refuses it there — and goes on resuming it under the selection that issued it.
#[tokio::test]
async fn a_page_token_resumes_only_the_filter_selection_that_issued_it() {
    let fixture = Fixture::new().await;
    let service = &fixture.service;

    for row in fixture.rows() {
        let case = &row.case;

        // What each selection holds, before any token is in play.
        let issued_over = identities(service, &row, &row.issuer).await;
        let elsewhere_holds = identities(service, &row, &row.resumed).await;
        assert!(
            issued_over.len() > 1,
            "case {case:?}: the issuing selection must hold more than one record for a page \
             of one to leave any behind, got {issued_over:?}"
        );
        assert!(
            !elsewhere_holds.is_empty(),
            "case {case:?}: the resuming selection must hold records of its own, so a short \
             page there is visible, got {elsewhere_holds:?}"
        );
        let remainder = issued_over[1..].to_vec();
        assert!(
            elsewhere_holds
                .iter()
                .any(|record| !remainder.contains(record)),
            "case {case:?}: the resuming selection must hold a record lying before the \
             position the token names, so serving the token there would be a short page, \
             got {elsewhere_holds:?} against {remainder:?}"
        );

        // A page of one under the issuing selection, and the token it ends with.
        let (status, first) = service
            .call(
                row.operation,
                &merged(&merged(&row.base, &row.issuer), &json!({ "maxResults": 1 })),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a page of one must be served, got {status} {first}"
        );
        let token = first["nextToken"].as_str().unwrap_or_default().to_string();
        assert!(
            !token.is_empty(),
            "case {case:?}: a page of one with records left over must carry a nextToken, \
             got {first}"
        );

        // Under the selection that issued it, the token resumes exactly where that page
        // ended.
        let resumed = identities(
            service,
            &row,
            &merged(&row.issuer, &json!({ "nextToken": token })),
        )
        .await;
        assert_eq!(
            resumed, remainder,
            "case {case:?}: its own selection must resume after the record the page ended \
             at, expected {remainder:?}, got {resumed:?}"
        );

        // Offered under any other selection, it names no position among the records that
        // call pages, so it is refused rather than served as a short or empty page.
        let (status, refused) = service
            .call(
                row.operation,
                &merged(
                    &merged(&row.base, &row.resumed),
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
            foreign_page_token_message(row.operation),
            "case {case:?}: expected this service's one foreign-token sentence, got {refused}"
        );

        // And the refusal changed nothing: the other selection still reports every record
        // it held.
        let still_holds = identities(service, &row, &row.resumed).await;
        assert_eq!(
            still_holds, elsewhere_holds,
            "case {case:?}: the refusal must leave the other selection's records as they \
             were, expected {elsewhere_holds:?}, got {still_holds:?}"
        );
    }
}

/// The other side of the same rule: a selection is the terms that SELECT the records, so
/// two requests selecting the same records resume each other's tokens — a filter member
/// carrying the value that selects nothing (`filterByPublished`'s modelled default
/// `false`, which asks for no filtering) is the same selection as the member absent.
#[tokio::test]
async fn a_filter_member_carrying_its_default_resumes_an_unfiltered_token() {
    let fixture = Fixture::new().await;
    let service = &fixture.service;

    let row = Row {
        case: "ListParticipants / filterByPublished=false is no filter".to_string(),
        operation: "ListParticipants",
        records: "participants",
        identity: "participantId",
        base: json!({
            "stageArn": fixture.participant_stage,
            "sessionId": fixture.participant_session,
        }),
        issuer: json!({}),
        resumed: json!({ "filterByPublished": false }),
    };
    let case = &row.case;

    let held = identities(service, &row, &row.issuer).await;
    assert!(
        held.len() > 1,
        "case {case:?}: the session must hold more than one participant, got {held:?}"
    );

    let (status, first) = service
        .call(
            row.operation,
            &merged(&merged(&row.base, &row.issuer), &json!({ "maxResults": 1 })),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: a page of one must be served, got {status} {first}"
    );
    let token = first["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty(),
        "case {case:?}: a page of one with participants left over must carry a nextToken, \
         got {first}"
    );

    let resumed = identities(
        service,
        &row,
        &merged(&row.resumed, &json!({ "nextToken": token })),
    )
    .await;
    let expected = held[1..].to_vec();
    assert_eq!(
        resumed, expected,
        "case {case:?}: the same records are selected, so the token must resume there, \
         expected {expected:?}, got {resumed:?}"
    );
}
