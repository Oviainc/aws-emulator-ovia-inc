// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` names a position among the records ONE scope selects, so only that same
//! scope can resume it.
//!
//! The rule, stated once and then exercised over a SET of inputs: every listing of this
//! service pages over the records the request's own scope selects — the region the call is
//! processed in, and the identity members that listing requires (a stage, a session, a
//! participant) — and the `nextToken` it hands back names a position among exactly those
//! records. A request offering such a token under any other scope names no position in the
//! records IT would page, so it is refused with the modelled `ValidationException` (HTTP
//! 400) carrying this service's one foreign-token sentence, and never served a page that
//! silently omits the records lying before that position.
//!
//! Rows differ along what the rule generalises over: the listing (every listing this
//! service implements), WHICH term of the scope the resuming request changes (the region,
//! the stage, the session, the participant), and whether the resuming scope's records lie
//! BEFORE or AFTER the position the token names — both directions, since a position past
//! every record of the resuming scope is exactly what an unscoped token turns into a
//! silently empty page.
//!
//! Nothing in the IVS RealTime control plane makes a participant JOIN a stage: a
//! participant joins over the stage's data plane with the token it was issued, which is
//! `state::join_stage_session` — so, like the sibling `ListParticipants` suites, the
//! fixture brings participants into a session through that seam and asserts everything
//! else over the router's own `dispatch`, as an SDK client sees it.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The two regions this suite signs for: a scope term every listing of this service pages
/// within ("all … in your account, in the AWS region where the API request is processed").
const REGIONS: [&str; 2] = ["us-east-1", "us-west-2"];

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the broadcast destination built by hand below carries it too.
const ACCOUNT: &str = "000000000000";

/// How many participants the fixture's ENDED session holds, and how many its LIVE one
/// does: more than one each, so either can issue a page token of its own.
const ENDED_SESSION_PARTICIPANTS: usize = 2;
const LIVE_SESSION_PARTICIPANTS: usize = 3;

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
    /// scope naming `ivs` in `region` — which is how a request names the region it is
    /// processed in.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://ivsrealtime.{region}.amazonaws.com/{operation}"
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

    /// Make the call `operation` in `region`, asserting it was served, and answer with the
    /// response body.
    async fn served(&self, region: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call_in(region, operation, body).await;
        assert_eq!(
            status, 200,
            "fixture {operation} {body} in {region}: expected HTTP 200, got {status} {response}"
        );
        response
    }

    /// Create one resource in `region` through `operation` and answer with the ARN the
    /// service minted for it, read off the response member `member`.
    async fn created(
        &self,
        region: &str,
        operation: &str,
        body: &Value,
        member: &str,
    ) -> String {
        let response = self.served(region, operation, body).await;
        text(&response[member], "arn")
    }

    /// Create a stage in `region`.
    async fn stage(&self, region: &str, name: &str) -> String {
        self.created(region, "CreateStage", &json!({ "name": name }), "stage")
            .await
    }

    /// Issue a participant token for `stage_arn` and answer with the participant id it
    /// carries — the id the holder of that token joins the stage under, and the id a
    /// replication of it names.
    async fn participant(&self, region: &str, stage_arn: &str) -> String {
        let response = self
            .served(
                region,
                "CreateParticipantToken",
                &json!({ "stageArn": stage_arn }),
            )
            .await;
        text(&response["participantToken"], "participantId")
    }

    /// The stage's `activeSessionId`, read back over the wire.
    async fn active_session(&self, region: &str, stage_arn: &str) -> String {
        let response = self
            .served(region, "GetStage", &json!({ "arn": stage_arn }))
            .await;
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
    /// the last one leaves, the session ends and the next join opens another.
    fn leave(&self, stage_arn: &str, session_id: &str, participant_id: &str) {
        let service_state = state::service_state(&*self.engine).expect("this service's store");
        let mut store = service_state.lock();
        assert!(
            state::leave_stage_session(&mut store, stage_arn, session_id, participant_id),
            "the departure of {participant_id:?} from {session_id:?} must be recorded"
        );
    }

    /// Replicate the publisher `participant_id` of `source` into `destination`.
    async fn replicate(&self, region: &str, source: &str, participant_id: &str, destination: &str) {
        self.served(
            region,
            "StartParticipantReplication",
            &json!({
                "sourceStageArn": source,
                "destinationStageArn": destination,
                "participantId": participant_id,
            }),
        )
        .await;
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

/// The sentence every listing of this service refuses a token naming no position in it
/// with — the service's one wording, kept here as a client reads it.
fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
}

/// One stage of the fixture: two sessions, an ended one and a live one, each holding more
/// than one participant.
struct Stage {
    arn: String,
    ended_session: String,
    live_session: String,
    /// A participant of the live session, whose join recorded more than one occurrence.
    live_participant: String,
    /// A participant of the ended session, whose join and departure did the same.
    departed_participant: String,
}

/// One region's view of the account, with every record each listing of this service pages
/// over: at least two in every scope, so each one can issue a page token of its own.
struct World {
    region: &'static str,
    first: Stage,
    second: Stage,
    /// The two replication sources, each with its own publisher replicated into the same
    /// two destination stages — two replicas per `(sourceStageArn, participantId)` scope.
    replications: [(String, String); 2],
}

/// Build one stage with an ended session and a live one.
async fn build_stage(service: &RealTime, region: &'static str, name: &str) -> Stage {
    let arn = service.stage(region, name).await;

    // A session that has ENDED: every participant joined and then left, which closes it,
    // so the next join opens a second session of the same stage.
    let mut departed = Vec::new();
    for _ in 0..ENDED_SESSION_PARTICIPANTS {
        let participant = service.participant(region, &arn).await;
        service.join(&arn, &participant);
        departed.push(participant);
    }
    let ended_session = service.active_session(region, &arn).await;
    for participant in &departed {
        service.leave(&arn, &ended_session, participant);
    }

    // The live session, which the joins above left room for.
    let mut live = Vec::new();
    for _ in 0..LIVE_SESSION_PARTICIPANTS {
        let participant = service.participant(region, &arn).await;
        service.join(&arn, &participant);
        live.push(participant);
    }
    let live_session = service.active_session(region, &arn).await;
    assert_ne!(
        ended_session, live_session,
        "stage {name:?}: the session the departures closed and the one the later joins \
         opened must be two sessions"
    );

    Stage {
        arn,
        ended_session,
        live_session,
        live_participant: live
            .first()
            .cloned()
            .expect("the live session holds participants"),
        departed_participant: departed
            .first()
            .cloned()
            .expect("the ended session held participants"),
    }
}

/// Build one region's whole view: the records every listing of this service pages over.
async fn build_world(service: &RealTime, region: &'static str) -> World {
    let first = build_stage(service, region, "first").await;
    let second = build_stage(service, region, "second").await;

    // Two destination stages, so one publisher has two replicas and its listing pages.
    let destinations = [
        service.stage(region, "destination-one").await,
        service.stage(region, "destination-two").await,
    ];
    let mut replications = Vec::new();
    for name in ["source-one", "source-two"] {
        let source = service.stage(region, name).await;
        let publisher = service.participant(region, &source).await;
        for destination in &destinations {
            service
                .replicate(region, &source, &publisher, destination)
                .await;
        }
        replications.push((source, publisher));
    }

    // Two of each account-wide resource, so each of those listings pages too.
    let mut encoders = Vec::new();
    for name in ["encoder-one", "encoder-two"] {
        encoders.push(
            service
                .created(
                    region,
                    "CreateEncoderConfiguration",
                    &json!({ "name": name }),
                    "encoderConfiguration",
                )
                .await,
        );
        service
            .created(
                region,
                "CreateStorageConfiguration",
                &json!({ "s3": { "bucketName": name } }),
                "storageConfiguration",
            )
            .await;
        service
            .created(
                region,
                "CreateIngestConfiguration",
                &json!({ "name": name, "ingestProtocol": "RTMPS" }),
                "ingestConfiguration",
            )
            .await;
    }
    // One composition per stage, each broadcasting to a channel of this region through one
    // of the encoder configurations above.
    for (stage, encoder) in [&first.arn, &second.arn].into_iter().zip(encoders.iter()) {
        service
            .served(
                region,
                "StartComposition",
                &json!({
                    "stageArn": stage,
                    "destinations": [{
                        "channel": {
                            "channelArn": format!("arn:aws:ivs:{region}:{ACCOUNT}:channel/composed"),
                            "encoderConfigurationArn": encoder,
                        },
                    }],
                }),
            )
            .await;
    }

    World {
        region,
        first,
        second,
        replications: [
            replications[0].clone(),
            replications[1].clone(),
        ],
    }
}

/// One call to one listing: the operation, where its records and their identities live in
/// the response, and the SCOPE the call names — the region it is signed for and the
/// identity members it carries.
#[derive(Clone)]
struct Page {
    operation: &'static str,
    /// The response member holding the page's records.
    records: &'static str,
    /// The member of a record this suite reads its identity off.
    identity: &'static str,
    region: &'static str,
    scope: Value,
}

impl Page {
    /// This page's request with `extra` merged in (the `maxResults` and `nextToken` a case
    /// supplies).
    fn request(&self, extra: &Value) -> Value {
        let mut request = self.scope.clone();
        if let Some(members) = extra.as_object() {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        request
    }

    /// Call this page with `extra` merged into its request.
    async fn call(&self, service: &RealTime, extra: &Value) -> (u16, Value) {
        service
            .call_in(self.region, self.operation, &self.request(extra))
            .await
    }

    /// The identities this page reports with `extra` merged in, asserting it was served.
    async fn identities(&self, service: &RealTime, case: &str, extra: &Value) -> Vec<String> {
        let (status, response) = self.call(service, extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {} {} must answer HTTP 200, got {status} {response}",
            self.operation,
            self.request(extra)
        );
        let records = response[self.records].as_array().cloned().unwrap_or_else(|| {
            panic!(
                "case {case:?}: {} must report its records under {:?}, got {response}",
                self.operation, self.records
            )
        });
        records
            .iter()
            .map(|record| text(record, self.identity))
            .collect()
    }
}

/// A `Page` of `operation` in `region` over `scope`.
fn page(
    operation: &'static str,
    records: &'static str,
    identity: &'static str,
    region: &'static str,
    scope: Value,
) -> Page {
    Page {
        operation,
        records,
        identity,
        region,
        scope,
    }
}

/// Every listing's page in one world, keyed by the scope term a row varies.
struct Pages {
    stages: Page,
    storage_configurations: Page,
    encoder_configurations: Page,
    ingest_configurations: Page,
    compositions: Page,
    /// `ListStageSessions` over each of the world's two stages.
    stage_sessions: [Page; 2],
    /// `ListParticipants` over the live session of each stage, and over the first stage's
    /// ENDED session — another session of the SAME stage.
    participants_first_live: Page,
    participants_first_ended: Page,
    participants_second_live: Page,
    /// `ListParticipantEvents` over two participants of the first stage's live session'
    /// sibling scopes, and over a participant of its ended session.
    events_first_live: Page,
    events_first_departed: Page,
    events_second_live: Page,
    /// `ListParticipantReplicas` over each of the world's two replication sources.
    replicas: [Page; 2],
}

/// The pages of one world, one per scope a row of the table below resumes a token under.
fn pages(world: &World) -> Pages {
    let region = world.region;
    Pages {
        stages: page("ListStages", "stages", "arn", region, json!({})),
        storage_configurations: page(
            "ListStorageConfigurations",
            "storageConfigurations",
            "arn",
            region,
            json!({}),
        ),
        encoder_configurations: page(
            "ListEncoderConfigurations",
            "encoderConfigurations",
            "arn",
            region,
            json!({}),
        ),
        ingest_configurations: page(
            "ListIngestConfigurations",
            "ingestConfigurations",
            "arn",
            region,
            json!({}),
        ),
        compositions: page("ListCompositions", "compositions", "arn", region, json!({})),
        stage_sessions: [
            page(
                "ListStageSessions",
                "stageSessions",
                "sessionId",
                region,
                json!({ "stageArn": world.first.arn }),
            ),
            page(
                "ListStageSessions",
                "stageSessions",
                "sessionId",
                region,
                json!({ "stageArn": world.second.arn }),
            ),
        ],
        participants_first_live: page(
            "ListParticipants",
            "participants",
            "participantId",
            region,
            json!({ "stageArn": world.first.arn, "sessionId": world.first.live_session }),
        ),
        participants_first_ended: page(
            "ListParticipants",
            "participants",
            "participantId",
            region,
            json!({ "stageArn": world.first.arn, "sessionId": world.first.ended_session }),
        ),
        participants_second_live: page(
            "ListParticipants",
            "participants",
            "participantId",
            region,
            json!({ "stageArn": world.second.arn, "sessionId": world.second.live_session }),
        ),
        events_first_live: page(
            "ListParticipantEvents",
            "events",
            "name",
            region,
            json!({
                "stageArn": world.first.arn,
                "sessionId": world.first.live_session,
                "participantId": world.first.live_participant,
            }),
        ),
        events_first_departed: page(
            "ListParticipantEvents",
            "events",
            "name",
            region,
            json!({
                "stageArn": world.first.arn,
                "sessionId": world.first.ended_session,
                "participantId": world.first.departed_participant,
            }),
        ),
        events_second_live: page(
            "ListParticipantEvents",
            "events",
            "name",
            region,
            json!({
                "stageArn": world.second.arn,
                "sessionId": world.second.live_session,
                "participantId": world.second.live_participant,
            }),
        ),
        replicas: [
            page(
                "ListParticipantReplicas",
                "replicas",
                "destinationStageArn",
                region,
                json!({
                    "sourceStageArn": world.replications[0].0,
                    "participantId": world.replications[0].1,
                }),
            ),
            page(
                "ListParticipantReplicas",
                "replicas",
                "destinationStageArn",
                region,
                json!({
                    "sourceStageArn": world.replications[1].0,
                    "participantId": world.replications[1].1,
                }),
            ),
        ],
    }
}

/// One row: a token issued by `issuer` offered to `elsewhere`, which pages over other
/// records.
struct Row {
    case: String,
    issuer: Page,
    elsewhere: Page,
}

/// The table: for every listing of this service, each term of its scope a resuming request
/// can change, in BOTH directions — the resuming scope's records lying after the position
/// the token names, and before it.
fn rows(here: &Pages, there: &Pages) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut pair = |how: &str, left: &Page, right: &Page| {
        rows.push(Row {
            case: format!("{} / {how} / forwards", left.operation),
            issuer: left.clone(),
            elsewhere: right.clone(),
        });
        rows.push(Row {
            case: format!("{} / {how} / backwards", left.operation),
            issuer: right.clone(),
            elsewhere: left.clone(),
        });
    };

    // The account-wide listings: the region is the whole scope.
    pair("another region", &here.stages, &there.stages);
    pair(
        "another region",
        &here.storage_configurations,
        &there.storage_configurations,
    );
    pair(
        "another region",
        &here.encoder_configurations,
        &there.encoder_configurations,
    );
    pair(
        "another region",
        &here.ingest_configurations,
        &there.ingest_configurations,
    );
    pair("another region", &here.compositions, &there.compositions);

    // The stage-scoped listing.
    pair(
        "another stage",
        &here.stage_sessions[0],
        &here.stage_sessions[1],
    );
    pair(
        "another region's stage",
        &here.stage_sessions[0],
        &there.stage_sessions[0],
    );

    // The session-scoped listing: another session of the SAME stage, a session of another
    // stage, and a session of another region's stage.
    pair(
        "another session of the same stage",
        &here.participants_first_live,
        &here.participants_first_ended,
    );
    pair(
        "another stage's session",
        &here.participants_first_live,
        &here.participants_second_live,
    );
    pair(
        "another region's stage session",
        &here.participants_first_live,
        &there.participants_first_live,
    );

    // The participant-scoped listing of a session.
    pair(
        "another participant in another session of the same stage",
        &here.events_first_live,
        &here.events_first_departed,
    );
    pair(
        "another stage's participant",
        &here.events_first_live,
        &here.events_second_live,
    );
    pair(
        "another region's participant",
        &here.events_first_live,
        &there.events_first_live,
    );

    // The participant-scoped listing of a source stage.
    pair("another source stage", &here.replicas[0], &here.replicas[1]);
    pair(
        "another region's source stage",
        &here.replicas[0],
        &there.replicas[0],
    );

    rows
}

/// A token one scope issued names no position in another, so the listing refuses it there
/// — and goes on resuming it inside the scope that issued it.
#[tokio::test]
async fn a_page_token_resumes_only_the_scope_that_issued_it() {
    let service = RealTime::new();
    // The first world's records take the lower store positions, so a row whose issuer is
    // the SECOND world names a position past some or all of the first world's records:
    // the short-or-empty page an unscoped token answers with.
    let here = build_world(&service, REGIONS[0]).await;
    let there = build_world(&service, REGIONS[1]).await;
    let (here, there) = (pages(&here), pages(&there));

    for row in rows(&here, &there) {
        let case = &row.case;

        // What each scope holds, before any token is in play.
        let issued_over = row.issuer.identities(&service, case, &json!({})).await;
        let elsewhere_holds = row.elsewhere.identities(&service, case, &json!({})).await;
        assert!(
            issued_over.len() > 1,
            "case {case:?}: the issuing scope must hold more than one record for a page \
             of one to leave any behind, got {issued_over:?}"
        );
        assert!(
            !elsewhere_holds.is_empty(),
            "case {case:?}: the resuming scope must hold records of its own, so a short \
             page there is visible, got {elsewhere_holds:?}"
        );

        // A page of one, and the token it ends with.
        let (status, first) = row.issuer.call(&service, &json!({ "maxResults": 1 })).await;
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

        // Inside the scope that issued it, the token resumes exactly where that page
        // ended.
        let resumed = row
            .issuer
            .identities(&service, case, &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            resumed,
            issued_over[1..].to_vec(),
            "case {case:?}: its own scope must resume after the record the page ended at, \
             expected {:?}, got {resumed:?}",
            &issued_over[1..]
        );

        // Offered to any other scope, it names no position among the records that call
        // pages, so it is refused rather than served as a short or empty page.
        let (status, refused) = row
            .elsewhere
            .call(&service, &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token of another scope must be refused, got {status} {refused}"
        );
        assert_eq!(
            text(&refused, "Code"),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {refused}"
        );
        assert_eq!(
            text(&refused, "Message"),
            foreign_page_token_message(row.elsewhere.operation),
            "case {case:?}: expected this service's one foreign-token sentence, got {refused}"
        );

        // And the refusal changed nothing: the other scope still reports every record it
        // held.
        let still_holds = row.elsewhere.identities(&service, case, &json!({})).await;
        assert_eq!(
            still_holds, elsewhere_holds,
            "case {case:?}: the refusal must leave the other scope's records as they were, \
             expected {elsewhere_holds:?}, got {still_holds:?}"
        );
    }
}

/// The boundary of the same rule, as a client meets it: the position a token names lies
/// PAST every record of the scope it is offered to, which is the reading under which an
/// unscoped token answers HTTP 200 with an empty list and no `nextToken` — a session
/// reported as holding no participant while it holds connected ones.
#[tokio::test]
async fn a_participants_token_naming_a_position_past_another_session_is_refused() {
    let service = RealTime::new();
    let region = REGIONS[0];

    // The session that will be RESUMED is populated FIRST, so every one of its
    // participants sits at a lower store position than any participant of the session
    // that issues the token.
    let resumed_stage = build_stage(&service, region, "populated-first").await;
    let issuing_stage = build_stage(&service, region, "populated-second").await;

    let resumed = page(
        "ListParticipants",
        "participants",
        "participantId",
        region,
        json!({
            "stageArn": resumed_stage.arn,
            "sessionId": resumed_stage.live_session,
        }),
    );
    let issuer = page(
        "ListParticipants",
        "participants",
        "participantId",
        region,
        json!({
            "stageArn": issuing_stage.arn,
            "sessionId": issuing_stage.live_session,
        }),
    );

    let held = resumed.identities(&service, "before the token", &json!({})).await;
    assert_eq!(
        held.len(),
        LIVE_SESSION_PARTICIPANTS,
        "the session to be resumed must hold its participants, got {held:?}"
    );

    let (status, first) = issuer.call(&service, &json!({ "maxResults": 1 })).await;
    assert_eq!(
        status, 200,
        "a page of one must be served, got {status} {first}"
    );
    let token = first["nextToken"].as_str().unwrap_or_default().to_string();
    assert!(
        !token.is_empty(),
        "a page of one with participants left over must carry a nextToken, got {first}"
    );

    let (status, refused) = resumed.call(&service, &json!({ "nextToken": token })).await;
    assert_eq!(
        status, 400,
        "a token issued for another session must be refused, not answered with a page of \
         the participants that lie past its position, got {status} {refused}"
    );
    assert_eq!(
        text(&refused, "Code"),
        "ValidationException",
        "expected the modelled ValidationException, got {refused}"
    );
    assert_eq!(
        text(&refused, "Message"),
        foreign_page_token_message("ListParticipants"),
        "expected this service's one foreign-token sentence, got {refused}"
    );

    let after = resumed.identities(&service, "after the refusal", &json!({})).await;
    assert_eq!(
        after, held,
        "the refusal must leave the session's participants as they were, expected {held:?}, \
         got {after:?}"
    );
}
