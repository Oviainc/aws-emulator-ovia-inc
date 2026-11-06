// SPDX-License-Identifier: CC0-1.0

//! Every resource of this service belongs to the region its ARN names, and a request is
//! answered from the view of the region IT was processed in.
//!
//! The rule the model states for each listing — "all stages in your account, **in the AWS
//! region where the API request is processed**", and the same sentence on
//! `ListStorageConfigurations` (`smithy_query(query="doc",shape="ListStages")`) — holds for
//! the whole surface: every ARN shape of this model carries a region element, each create
//! fills it from the region the creating request was signed for, and a request signed for
//! another region addresses another account view. So:
//!
//! * a listing reports exactly the resources of the region its own request was processed
//!   in;
//! * a read, update, delete or tagging call naming an ARN of another region is answered as
//!   the resource this view does not hold — `ResourceNotFoundException` (HTTP 404), stating
//!   the identifier the request named;
//! * a stage-keyed report of another region's stage selects nothing, exactly as one naming
//!   a stage that was never created does;
//! * a member REFERRING to another resource of this service resolves in the request's own
//!   region, so a reference to another region's resource is the unresolved-reference
//!   `ValidationException` (HTTP 400) every dangling reference of this service is.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion is
//! about what an SDK client sees: the HTTP status, the error code and the JSON body. The
//! rows of each table differ along what the rule generalises over — the resource KIND, the
//! operation, and the pair of regions involved.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::state;

/// A low-latency channel of the sibling `ivs` endpoint, which this store never holds and
/// a composition destination therefore never resolves against: it is here only so a
/// `StartComposition` request carries a well-formed destination.
const CHANNEL_ARN: &str = "arn:aws:ivs:us-east-1:000000000000:channel/live-channel";

/// A kind of resource this service mints, with the create that mints one.
#[derive(Clone, Copy, Debug)]
enum Kind {
    Stage,
    StorageConfiguration,
    EncoderConfiguration,
    IngestConfiguration,
    Composition,
}

/// The call under test, built from the ARN of the resource it names.
#[derive(Clone, Copy, Debug)]
enum Call {
    /// `POST /<Operation>`, with the body this builds from the resource ARN.
    Operation(&'static str, fn(&str) -> Value),
    /// One of the three operations bound to `/tags/{resourceArn}`: the method, the query
    /// string and the body.
    Tagging(&'static str, &'static str, Option<fn() -> Value>),
}

/// One emulator holding only this service, driven through the router that serves
/// production traffic, plus the engine handle the data-plane seeding writes through.
struct RealTime {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl RealTime {
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
    /// scope naming `ivs` in `region` — which is how a request names the region it is
    /// processed in.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in
    /// the URI the way a client does: percent-encoded into one path segment.
    async fn tagging_in(
        &self,
        region: &str,
        method: &str,
        arn: &str,
        query: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(
            region,
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com{path}"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(body.unwrap_or_default())
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

    /// Make the call `call` names against `arn`, through `region`'s endpoint.
    async fn dispatch(&self, region: &str, call: Call, arn: &str) -> (u16, Value) {
        match call {
            Call::Operation(operation, body) => {
                self.call_in(region, operation, &body(arn)).await
            }
            Call::Tagging(method, query, body) => {
                self.tagging_in(region, method, arn, query, body.map(|body| body()))
                    .await
            }
        }
    }

    /// Create one resource in `region` and return the ARN the service minted for it,
    /// read out of `member` of the answer.
    async fn created(&self, region: &str, operation: &str, body: &Value, member: &str) -> String {
        let (status, response) = self.call_in(region, operation, body).await;
        assert_eq!(
            status, 200,
            "{operation} {body} in {region} must answer HTTP 200, got {status} {response}"
        );
        let arn = member_of(&response[member], "arn");
        assert!(
            arn.contains(&format!(":{region}:")),
            "{operation} in {region} must mint an ARN naming that region, got {arn:?}"
        );
        arn
    }

    /// Create a stage in `region`.
    async fn create_stage(&self, region: &str) -> String {
        self.created(region, "CreateStage", &json!({ "name": "regional" }), "stage")
            .await
    }

    /// Create one resource of `kind` in `region`, with whatever other resource of that
    /// same region the create needs, and return the ARN the service minted for it.
    async fn create(&self, kind: Kind, region: &str) -> String {
        match kind {
            Kind::Stage => self.create_stage(region).await,
            Kind::StorageConfiguration => {
                self.created(
                    region,
                    "CreateStorageConfiguration",
                    &json!({ "s3": { "bucketName": "recordings" }, "name": "regional" }),
                    "storageConfiguration",
                )
                .await
            }
            Kind::EncoderConfiguration => {
                self.created(
                    region,
                    "CreateEncoderConfiguration",
                    &json!({ "name": "regional" }),
                    "encoderConfiguration",
                )
                .await
            }
            Kind::IngestConfiguration => {
                let stage = self.create_stage(region).await;
                self.created(
                    region,
                    "CreateIngestConfiguration",
                    &json!({ "ingestProtocol": "RTMPS", "stageArn": stage }),
                    "ingestConfiguration",
                )
                .await
            }
            Kind::Composition => {
                let stage = self.create_stage(region).await;
                self.created(
                    region,
                    "StartComposition",
                    &json!({
                        "stageArn": stage,
                        "destinations": [{ "channel": { "channelArn": CHANNEL_ARN } }],
                    }),
                    "composition",
                )
                .await
            }
        }
    }

    /// Issue a participant token for `stage_arn` through `region` and return the
    /// participant id it carries.
    async fn create_token(&self, region: &str, stage_arn: &str) -> String {
        let (status, response) = self
            .call_in(region, "CreateParticipantToken", &json!({ "stageArn": stage_arn }))
            .await;
        assert_eq!(
            status, 200,
            "CreateParticipantToken {stage_arn} in {region} must answer HTTP 200, \
             got {status} {response}"
        );
        member_of(&response["participantToken"], "participantId")
    }

    /// Record a participant joining `stage_arn`, the way the stage's data plane does —
    /// nothing in the control plane opens a stage session.
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

    /// The stage's `activeSessionId`, read back through `region`'s endpoint.
    async fn active_session_id(&self, region: &str, stage_arn: &str) -> String {
        let (status, response) = self
            .call_in(region, "GetStage", &json!({ "arn": stage_arn }))
            .await;
        assert_eq!(
            status, 200,
            "GetStage {stage_arn} in {region} must answer HTTP 200, got {status} {response}"
        );
        member_of(&response["stage"], "activeSessionId")
    }
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member_of(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> String {
    member_of(response, "__type")
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// The not-found text this service states for an identifier it holds no resource for.
fn not_found(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// The code and the sentence the `/tags/{resourceArn}` surface answers a not-found with:
/// its own, which name neither the ARN nor the kind of resource it addressed (see
/// `tagging.rs`), where every resource-specific read answers the modelled code and
/// [`not_found`].
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// What the call `call` answers for a resource this account view does not hold: the
/// tagging surface's own refusal for the three operations bound to `/tags/{resourceArn}`,
/// and the modelled one naming the ARN for every other.
fn expected_not_found(call: Call, arn: &str) -> (String, String) {
    match call {
        Call::Tagging(..) => (
            TAGGING_NOT_FOUND_CODE.to_string(),
            TAGGING_NOT_FOUND_MESSAGE.to_string(),
        ),
        Call::Operation(..) => ("ResourceNotFoundException".to_string(), not_found(arn)),
    }
}

/// The refusal this service states for a member that names a resource of this service
/// this account view holds none of.
fn unresolved_reference(operation: &str, member: &str, resource: &str, identifier: &str) -> String {
    format!(
        "{operation}.{member} is required to name a {resource} this account holds, \
         found {identifier}"
    )
}

/// The ARNs a listing reported, in the order it reported them.
fn listed_arns(response: &Value, member: &str) -> Vec<String> {
    response[member]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| member_of(summary, "arn"))
        .collect()
}

/// How many entries a listing reported under `member`.
fn listed_count(response: &Value, member: &str) -> usize {
    response[member].as_array().map_or(0, Vec::len)
}

/// A listing reports exactly the resources of the region its own request was processed
/// in: the one created through that region's endpoint is in the page, the one created
/// through another region's endpoint is not, and every ARN the page carries names the
/// listing's own region.
///
/// The rows are the resource kinds this service both mints and lists — a stage, a storage
/// configuration, an encoder configuration, an ingest configuration and a composition —
/// each with the listing that reports it. The two regions differ per row, so the rule is
/// exercised over more than one pair.
#[tokio::test]
async fn a_listing_reports_only_the_resources_of_the_region_it_was_processed_in() {
    // (case, kind, listing, the page member holding the summaries, home region, other)
    let rows: [(&str, Kind, &str, &str, &str, &str); 5] = [
        (
            "stages",
            Kind::Stage,
            "ListStages",
            "stages",
            "us-east-1",
            "eu-west-1",
        ),
        (
            "encoder configurations",
            Kind::EncoderConfiguration,
            "ListEncoderConfigurations",
            "encoderConfigurations",
            "eu-west-1",
            "ap-southeast-2",
        ),
        (
            "storage configurations",
            Kind::StorageConfiguration,
            "ListStorageConfigurations",
            "storageConfigurations",
            "eu-central-1",
            "us-west-2",
        ),
        (
            "ingest configurations",
            Kind::IngestConfiguration,
            "ListIngestConfigurations",
            "ingestConfigurations",
            "ap-southeast-2",
            "us-east-1",
        ),
        (
            "compositions",
            Kind::Composition,
            "ListCompositions",
            "compositions",
            "us-west-2",
            "eu-central-1",
        ),
    ];

    for (case, kind, listing, member, home, other) in rows {
        let realtime = RealTime::new();
        let at_home = realtime.create(kind, home).await;
        let elsewhere = realtime.create(kind, other).await;

        for (region, mine, theirs) in [(home, &at_home, &elsewhere), (other, &elsewhere, &at_home)] {
            let (status, response) = realtime.call_in(region, listing, &json!({})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: {listing} in {region} must answer HTTP 200, \
                 got {status} {response}"
            );
            let reported = listed_arns(&response, member);
            assert!(
                reported.contains(mine),
                "case {case:?}: {listing} in {region} must report {mine:?}, got {reported:?}"
            );
            assert!(
                !reported.contains(theirs),
                "case {case:?}: {listing} in {region} must not report {theirs:?}, which was \
                 created through another region's endpoint, got {reported:?}"
            );
            for arn in &reported {
                assert!(
                    arn.contains(&format!(":{region}:")),
                    "case {case:?}: {listing} in {region} must report only ARNs of that \
                     region, got {arn:?}"
                );
            }
        }
    }
}

/// A call naming a resource of another region is answered as the resource this view does
/// not hold: `ResourceNotFoundException` (HTTP 404) stating the identifier the request
/// named — and the SAME call through the resource's own region succeeds, so the refusal is
/// the region term of the lookup and not a broken fixture.
///
/// The rows differ along what the rule generalises over: the resource kind, whether the
/// call reads, mutates, deletes or tags, and whether it is bound to `POST /<Operation>` or
/// to the `/tags/{resourceArn}` surface. Each row runs the foreign-region call FIRST, so a
/// mutation that leaked across regions would be visible in the own-region call that
/// follows.
#[tokio::test]
async fn a_resource_of_another_region_is_not_found_by_the_call_that_names_it() {
    let home = "us-east-1";
    let other = "eu-west-1";

    // (case, kind, the call, the status the call answers in the resource's own region)
    let rows: Vec<(&str, Kind, Call, u16)> = vec![
        (
            "GetStage",
            Kind::Stage,
            Call::Operation("GetStage", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "UpdateStage",
            Kind::Stage,
            Call::Operation("UpdateStage", |arn| {
                json!({ "arn": arn, "name": "renamed" })
            }),
            200,
        ),
        (
            "CreateParticipantToken",
            Kind::Stage,
            Call::Operation("CreateParticipantToken", |arn| json!({ "stageArn": arn })),
            200,
        ),
        (
            "StartComposition naming the stage",
            Kind::Stage,
            Call::Operation("StartComposition", |arn| {
                json!({
                    "stageArn": arn,
                    "destinations": [{ "channel": { "channelArn": CHANNEL_ARN } }],
                })
            }),
            200,
        ),
        (
            "DeleteStage",
            Kind::Stage,
            Call::Operation("DeleteStage", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "GetStorageConfiguration",
            Kind::StorageConfiguration,
            Call::Operation("GetStorageConfiguration", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "GetEncoderConfiguration",
            Kind::EncoderConfiguration,
            Call::Operation("GetEncoderConfiguration", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "DeleteEncoderConfiguration",
            Kind::EncoderConfiguration,
            Call::Operation("DeleteEncoderConfiguration", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "GetIngestConfiguration",
            Kind::IngestConfiguration,
            Call::Operation("GetIngestConfiguration", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "UpdateIngestConfiguration",
            Kind::IngestConfiguration,
            Call::Operation("UpdateIngestConfiguration", |arn| {
                json!({ "arn": arn, "stageArn": "" })
            }),
            200,
        ),
        (
            "DeleteIngestConfiguration",
            Kind::IngestConfiguration,
            Call::Operation("DeleteIngestConfiguration", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "GetComposition",
            Kind::Composition,
            Call::Operation("GetComposition", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "StopComposition",
            Kind::Composition,
            Call::Operation("StopComposition", |arn| json!({ "arn": arn })),
            200,
        ),
        (
            "ListTagsForResource on a stage",
            Kind::Stage,
            Call::Tagging("GET", "", None),
            200,
        ),
        (
            "TagResource on a storage configuration",
            Kind::StorageConfiguration,
            Call::Tagging("POST", "", Some(|| json!({ "tags": { "team": "video" } }))),
            204,
        ),
        (
            "UntagResource on a composition",
            Kind::Composition,
            Call::Tagging("DELETE", "?tagKeys=team", None),
            204,
        ),
    ];

    for (case, kind, call, own_status) in rows {
        let realtime = RealTime::new();
        let arn = realtime.create(kind, home).await;

        let (status, response) = realtime.dispatch(other, call, &arn).await;
        let (expected_code, expected_message) = expected_not_found(call, &arn);
        assert_eq!(
            (status, error_code(&response), member_of(&response, "message")),
            (404, expected_code, expected_message),
            "case {case:?}: naming a resource of {home} through {other} must be a \
             not-found, got {status} {response}"
        );

        let (status, response) = realtime.dispatch(home, call, &arn).await;
        assert_eq!(
            status, own_status,
            "case {case:?}: the same call through {home}, the region holding the resource, \
             must answer HTTP {own_status}, got {status} {response}"
        );
    }
}

/// A stage-keyed report answers about another region's stage exactly as it does about a
/// stage that was never created: the listings select nothing and the readers answer the
/// not-found naming the stage — while the same calls through the stage's own region report
/// the session the stage really has.
///
/// The fixture is one stage with one participant in one session, seeded through the stage
/// data plane (nothing in the control plane opens a session). The rows are every
/// stage-keyed report of this service, split by what "nothing here" is for each: an empty
/// page where the operation models no not-found, and the 404 where it does.
#[tokio::test]
async fn a_stage_keyed_report_from_another_region_sees_none_of_the_stages_session() {
    let home = "eu-west-1";
    let other = "us-east-1";
    let realtime = RealTime::new();

    let stage = realtime.create_stage(home).await;
    let participant = realtime.create_token(home, &stage).await;
    realtime.join(&stage, &participant);
    let session = realtime.active_session_id(home, &stage).await;
    assert!(
        !session.is_empty(),
        "the fixture join must have opened a session on {stage:?}"
    );

    // (case, listing, request, the page member, how many entries its own region reports)
    let pages: Vec<(&str, &str, Value, &str, usize)> = vec![
        (
            "ListStageSessions",
            "ListStageSessions",
            json!({ "stageArn": stage }),
            "stageSessions",
            1,
        ),
        (
            "ListParticipants",
            "ListParticipants",
            json!({ "stageArn": stage, "sessionId": session }),
            "participants",
            1,
        ),
        (
            "ListParticipantEvents",
            "ListParticipantEvents",
            json!({
                "stageArn": stage,
                "sessionId": session,
                "participantId": participant,
            }),
            "events",
            1,
        ),
        (
            "ListParticipantReplicas",
            "ListParticipantReplicas",
            json!({ "sourceStageArn": stage, "participantId": participant }),
            "replicas",
            0,
        ),
    ];

    for (case, listing, request, member, own_count) in pages {
        let (status, response) = realtime.call_in(other, listing, &request).await;
        assert_eq!(
            (status, listed_count(&response, member)),
            (200, 0),
            "case {case:?}: {listing} on a stage of {home} asked through {other} must report \
             nothing, got {status} {response}"
        );

        let (status, response) = realtime.call_in(home, listing, &request).await;
        assert_eq!(
            (status, listed_count(&response, member)),
            (200, own_count),
            "case {case:?}: {listing} through {home}, the region holding the stage, must \
             report {own_count} entries, got {status} {response}"
        );
    }

    // (case, operation, request, the status its own region answers)
    let readers: Vec<(&str, &str, Value, u16)> = vec![
        (
            "GetStageSession",
            "GetStageSession",
            json!({ "stageArn": stage, "sessionId": session }),
            200,
        ),
        (
            "GetParticipant",
            "GetParticipant",
            json!({
                "stageArn": stage,
                "sessionId": session,
                "participantId": participant,
            }),
            200,
        ),
        (
            "DisconnectParticipant",
            "DisconnectParticipant",
            json!({ "stageArn": stage, "participantId": participant }),
            200,
        ),
    ];

    for (case, operation, request, own_status) in readers {
        let (status, response) = realtime.call_in(other, operation, &request).await;
        assert_eq!(
            (status, error_code(&response), member_of(&response, "message")),
            (
                404,
                "ResourceNotFoundException".to_string(),
                not_found(&stage)
            ),
            "case {case:?}: naming a stage of {home} through {other} must be a not-found \
             stating that stage, got {status} {response}"
        );

        let (status, response) = realtime.call_in(home, operation, &request).await;
        assert_eq!(
            status, own_status,
            "case {case:?}: the same call through {home}, the region holding the stage, must \
             answer HTTP {own_status}, got {status} {response}"
        );
    }
}

/// A member that REFERS to another resource of this service resolves in the region the
/// request was processed in, so a reference to another region's resource is the
/// unresolved-reference `ValidationException` (HTTP 400) this service words every dangling
/// reference with — while the same request naming a resource of its own region is served.
///
/// The rows are every member of this service that carries such a reference, on both the
/// create and the update that take it, each paired with the accepted call that differs
/// from it only in the region the referenced resource lives in.
#[tokio::test]
async fn a_reference_to_a_resource_of_another_region_is_an_unresolved_reference() {
    let home = "us-east-1";
    let other = "ap-southeast-2";
    let realtime = RealTime::new();

    let home_storage = realtime.create(Kind::StorageConfiguration, home).await;
    let other_storage = realtime.create(Kind::StorageConfiguration, other).await;
    let home_stage = realtime.create_stage(home).await;
    let other_stage = realtime.create_stage(other).await;
    let home_ingest = realtime.create(Kind::IngestConfiguration, home).await;

    let recording_member = "autoParticipantRecordingConfiguration.storageConfigurationArn";

    // (case, operation, request, the status it answers, the sentence a refusal states)
    let rows: Vec<(&str, &str, Value, u16, String)> = vec![
        (
            "CreateStage recording into a storage configuration of another region",
            "CreateStage",
            json!({
                "name": "regional",
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": other_storage,
                },
            }),
            400,
            unresolved_reference(
                "CreateStage",
                recording_member,
                "storage configuration",
                &other_storage,
            ),
        ),
        (
            "CreateStage recording into one of its own region",
            "CreateStage",
            json!({
                "name": "regional",
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": home_storage,
                },
            }),
            200,
            String::new(),
        ),
        (
            "UpdateStage recording into a storage configuration of another region",
            "UpdateStage",
            json!({
                "arn": home_stage,
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": other_storage,
                },
            }),
            400,
            unresolved_reference(
                "UpdateStage",
                recording_member,
                "storage configuration",
                &other_storage,
            ),
        ),
        (
            "UpdateStage recording into one of its own region",
            "UpdateStage",
            json!({
                "arn": home_stage,
                "autoParticipantRecordingConfiguration": {
                    "storageConfigurationArn": home_storage,
                },
            }),
            200,
            String::new(),
        ),
        (
            "CreateIngestConfiguration associated with a stage of another region",
            "CreateIngestConfiguration",
            json!({ "ingestProtocol": "RTMPS", "stageArn": other_stage }),
            400,
            unresolved_reference(
                "CreateIngestConfiguration",
                "stageArn",
                "stage",
                &other_stage,
            ),
        ),
        (
            "CreateIngestConfiguration associated with a stage of its own region",
            "CreateIngestConfiguration",
            json!({ "ingestProtocol": "RTMPS", "stageArn": home_stage }),
            200,
            String::new(),
        ),
        (
            "UpdateIngestConfiguration moved onto a stage of another region",
            "UpdateIngestConfiguration",
            json!({ "arn": home_ingest, "stageArn": other_stage }),
            400,
            unresolved_reference(
                "UpdateIngestConfiguration",
                "stageArn",
                "stage",
                &other_stage,
            ),
        ),
        (
            "UpdateIngestConfiguration moved onto a stage of its own region",
            "UpdateIngestConfiguration",
            json!({ "arn": home_ingest, "stageArn": home_stage }),
            200,
            String::new(),
        ),
    ];

    for (case, operation, request, expected_status, expected_message) in rows {
        let (status, response) = realtime.call_in(home, operation, &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: {operation} {request} through {home} must answer HTTP \
             {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                (error_code(&response), member_of(&response, "message")),
                ("ValidationException".to_string(), expected_message),
                "case {case:?}: the refusal must state the unresolved reference, \
                 got {response}"
            );
        }
    }
}

/// The regions' views are independent of each other: what one region holds is neither
/// reported nor changed through another, and deleting a resource through the region that
/// holds it leaves every other region's resources exactly as they were.
///
/// The regions differ in how many stages each holds — two, one and none — so the rule is
/// asserted over a view with several resources, a view with one, and an empty view.
#[tokio::test]
async fn each_regions_view_of_the_account_is_independent() {
    let realtime = RealTime::new();

    // (region, how many stages it holds)
    let counts: [(&str, usize); 3] = [("us-east-1", 2), ("eu-west-1", 1), ("us-gov-west-1", 0)];
    let mut held: Vec<(&str, Vec<String>)> = Vec::new();
    for (region, count) in counts {
        let mut arns = Vec::new();
        for _ in 0..count {
            arns.push(realtime.create_stage(region).await);
        }
        held.push((region, arns));
    }

    for (region, arns) in &held {
        let (status, response) = realtime.call_in(region, "ListStages", &json!({})).await;
        let mut reported = listed_arns(&response, "stages");
        reported.sort();
        let mut expected = arns.clone();
        expected.sort();
        assert_eq!(
            (status, reported),
            (200, expected.clone()),
            "region {region:?}: ListStages must report exactly the {} stages created \
             through it, got {response}",
            expected.len()
        );
    }

    // One region deletes one of its own stages: every other region's view is untouched.
    let (region, arns) = held.first().cloned().expect("the first region holds stages");
    let deleted = arns.first().cloned().expect("it holds at least one stage");
    let (status, response) = realtime
        .call_in(region, "DeleteStage", &json!({ "arn": deleted }))
        .await;
    assert_eq!(
        status, 200,
        "DeleteStage {deleted} through {region} must answer HTTP 200, got {status} {response}"
    );

    for (other_region, other_arns) in held.iter().skip(1) {
        for arn in other_arns {
            let (status, response) = realtime
                .call_in(other_region, "GetStage", &json!({ "arn": arn }))
                .await;
            assert_eq!(
                status, 200,
                "region {other_region:?}: {arn:?} must survive a delete in {region:?}, \
                 got {status} {response}"
            );
        }
    }
    let (status, response) = realtime
        .call_in(region, "GetStage", &json!({ "arn": deleted }))
        .await;
    assert_eq!(
        (status, error_code(&response)),
        (404, "ResourceNotFoundException".to_string()),
        "region {region:?}: the deleted stage must be gone from its own view, \
         got {status} {response}"
    );
}
