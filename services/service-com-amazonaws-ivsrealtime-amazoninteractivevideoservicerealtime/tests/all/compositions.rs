// SPDX-License-Identifier: CC0-1.0

//! The composition surface over the wire: `StartComposition` and `GetComposition`.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. ROUND TRIP. `StartComposition` persists the composition the request describes,
//!    mints its ARN and one id per destination, and `GetComposition` reports that SAME
//!    composition back for its ARN — member for member, on every read.
//! 2. CLOCK-OWED TRANSITION. A started composition — and every destination of it — is
//!    `STARTING` when the call returns and becomes `ACTIVE` once the clock has made that
//!    transition due, with no further call; `startTime` does not move with it, and two
//!    reads at the same instant agree.
//! 3. DOCUMENTED LAYOUT DEFAULTS. `Composition$layout` is `@required`, so every
//!    composition reports a layout: a request that configures none gets the documented
//!    grid fallback, and every layout member the model documents a default for is
//!    reported with it.
//! 4. EVERY RESOURCE A REQUEST REFERS TO MUST EXIST. The stage being composited, and the
//!    encoder and storage configurations the destinations name, are each the modelled
//!    `ResourceNotFoundException` (HTTP 404) when this store holds no such resource;
//!    a value outside the member's modelled `@pattern`/`@length` is the modelled
//!    `ValidationException` (HTTP 400), which is a different answer from "not found".
//! 5. ONE PLACE PER DESTINATION, ONE LAYOUT PER COMPOSITION. A destination naming neither
//!    or both of `channel`/`s3`, and a layout configuring both `grid` and `pip`, are
//!    refused with HTTP 400.
//! 6. IDEMPOTENCY. `idempotencyToken` carries `@idempotencyToken`, so it is one of the
//!    attributes a start asks for: offering it again names the composition the first call
//!    created, which is the modelled conflict rather than a second composition.
//! 7. A composition is TAGGABLE through the shared `/tags/{resourceArn}` surface, and goes
//!    away with the stage it composites.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every test that does not vary it signs for.
const REGION: &str = "us-east-1";

/// `CompositionArn`'s `@pattern`, from
/// `smithy_query(query="shape",shape="Composition")` — member `arn`.
const COMPOSITION_ARN_PATTERN: &str = "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:composition/[a-zA-Z0-9-]+$";

/// The modelled `@length` maximum of `CompositionArn`.
const COMPOSITION_ARN_MAX_LENGTH: usize = 128;

/// The sentence a start whose `@idempotencyToken` already names a composition is refused
/// with, read from the crate so this suite cannot drift from the wording it answers.
const DUPLICATE_COMPOSITION_MESSAGE: &str = state::DUPLICATE_COMPOSITION_MESSAGE;

// The `CompositionState` and `DestinationState` values this suite asserts, from
// `smithy_query(query="shape",shape="CompositionState,DestinationState")`:
//   CompositionState  STARTING | ACTIVE | STOPPING | FAILED | STOPPED
//   DestinationState  STARTING | ACTIVE | STOPPING | RECONNECTING | FAILED | STOPPED
const STATE_STARTING: &str = "STARTING";
const STATE_ACTIVE: &str = "ACTIVE";

// The layout defaults the model documents, each from that member's own doc in
// `smithy_query(query="doc",shape="GridConfiguration",member="…")` /
// `…shape="PipConfiguration"`:
//   GridConfiguration$videoAspectRatio  "Default: VIDEO."
//   GridConfiguration$videoFillMode     "defaults to COVER fill mode for participants
//                                        in the grid"
//   PipConfiguration$videoFillMode      "Default: COVER."
//   PipConfiguration$pipBehavior        "Default: STATIC."
//   PipConfiguration$pipPosition        "Default: BOTTOM_RIGHT."
//   GridConfiguration$gridGap           "Default: 2."
//   PipConfiguration$gridGap            "Default: 0."
//   PipConfiguration$pipOffset          "Default: 0."
//   GridConfiguration$featuredParticipantAttribute  "Default: \"\" (no featured
//                                        participant)."
//   PipConfiguration$featuredParticipantAttribute   the same sentence
//   PipConfiguration$pipParticipantAttribute        "Default: \"\" (no PiP participant)."
const DEFAULT_GRID_ASPECT_RATIO: &str = "VIDEO";
const DEFAULT_GRID_FILL_MODE: &str = "COVER";
const DEFAULT_PIP_FILL_MODE: &str = "COVER";
const DEFAULT_PIP_BEHAVIOR: &str = "STATIC";
const DEFAULT_PIP_POSITION: &str = "BOTTOM_RIGHT";
const DEFAULT_GRID_GAP: i64 = 2;
const DEFAULT_PIP_GRID_GAP: i64 = 0;
const DEFAULT_PIP_OFFSET: i64 = 0;
const NO_FEATURED_PARTICIPANT: &str = "";
const NO_PIP_PARTICIPANT: &str = "";

/// Long enough that the `STARTING` -> `ACTIVE` transition the emulator owes has fallen
/// due, whatever scheduling delay the test host adds.
fn past_the_activation_span() -> Duration {
    state::COMPOSITION_ACTIVATION_SPAN + Duration::from_millis(400)
}

/// Shorter than the activation span, to pin that the span is NOT zero: a caller acting
/// straight after the start still sees `STARTING`.
const WITHIN_THE_ACTIVATION_SPAN: Duration = Duration::from_millis(1);

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` as an SDK would: `POST /<operation>` with a SigV4 credential
    /// scope naming `ivs` in `region`.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// `ListTagsForResource`, bound to `GET /tags/{resourceArn}`, carrying the ARN the
    /// way a client does: percent-encoded into one path segment.
    async fn list_tags(&self, arn: &str) -> (u16, Value) {
        self.send(REGION, "GET", &format!("/tags/{}", encode_path_label(arn)), None)
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
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
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

    /// Create a stage and return the ARN the service minted for it.
    async fn stage(&self, case: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": "composited" })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage must answer HTTP 200, got {status} {response}"
        );
        response["stage"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Create an encoder configuration and return its ARN.
    async fn encoder_configuration(&self, case: &str) -> String {
        let (status, response) = self
            .call("CreateEncoderConfiguration", &json!({ "name": "enc" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        response["encoderConfiguration"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// Create a storage configuration and return its ARN.
    async fn storage_configuration(&self, case: &str) -> String {
        let (status, response) = self
            .call(
                "CreateStorageConfiguration",
                &json!({ "s3": { "bucketName": "composition-recordings" } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStorageConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        response["storageConfiguration"]["arn"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// `StartComposition`, asserting the modelled success, and answering with the
    /// `composition` the service reported.
    async fn start(&self, case: &str, body: &Value) -> Value {
        let (status, response) = self.call("StartComposition", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartComposition {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["composition"].clone()
    }

    /// `GetComposition`, asserting the modelled success, and answering with the
    /// `composition` the service reported.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self.call("GetComposition", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetComposition {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["composition"].clone()
    }

    /// How many compositions this region's view holds, read through
    /// `ListCompositions` — the count a refused start must not have grown.
    async fn held_compositions(&self, case: &str) -> usize {
        let (status, response) = self.call("ListCompositions", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCompositions must answer HTTP 200, got {status} {response}"
        );
        response["compositions"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default()
    }
}

/// The wire error code of a failed call (`__type` in a restJson1 error body).
fn error_code(response: &Value) -> &str {
    response["__type"].as_str().unwrap_or("<no __type>")
}

/// A string member, read as the empty string when absent — what a client sees for a
/// member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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

/// Does `value` satisfy the one model `@pattern` this suite checks?
///
/// The pattern is quoted on the constant above, straight from `smithy_query`, and is
/// simple enough to check directly — which keeps this crate's dependency set exactly what
/// the service itself needs. A pattern with no arm panics rather than passing silently.
fn matches(pattern: &str, value: &str) -> bool {
    match pattern {
        // ^arn:aws:ivs:[a-z0-9-]+:[0-9]+:composition/[a-zA-Z0-9-]+$
        COMPOSITION_ARN_PATTERN => {
            let Some(rest) = value.strip_prefix("arn:aws:ivs:") else {
                return false;
            };
            let Some((region, rest)) = rest.split_once(':') else {
                return false;
            };
            let Some((account, resource)) = rest.split_once(':') else {
                return false;
            };
            let Some(id) = resource.strip_prefix("composition/") else {
                return false;
            };
            let region_like = !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            let id_like =
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
            region_like
                && !account.is_empty()
                && account.bytes().all(|b| b.is_ascii_digit())
                && id_like
        }
        other => panic!("this suite has no checker for the pattern {other:?}"),
    }
}

/// An IVS ARN of `resource_type` in `REGION`, naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// The destinations of a composition, as the wire reports them.
fn destinations(composition: &Value) -> Vec<Value> {
    composition["destinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The input class every round-trip row is drawn from
// ---------------------------------------------------------------------------

/// The resources a row of the start class refers to, resolved against this store.
struct Referenced {
    stage_arn: String,
    encoder_configuration_arn: String,
    storage_configuration_arn: String,
}

impl Referenced {
    async fn resolve(service: &RealTime, case: &str) -> Self {
        Self {
            stage_arn: service.stage(case).await,
            encoder_configuration_arn: service.encoder_configuration(case).await,
            storage_configuration_arn: service.storage_configuration(case).await,
        }
    }
}

/// The start requests this suite spans the class of inputs with: each row differs from
/// the others in what the rules generalise over — how many destinations the request
/// carries, which KIND each is (a low-latency channel or an S3 recording), which layout
/// it configures, and whether it carries tags.
///
/// `destinations` is `@length(1..2)` with `@uniqueItems`
/// (`smithy_query(query="operation",operation="StartComposition")`), so both published
/// bounds appear below.
fn start_cases(referenced: &Referenced) -> Vec<(&'static str, Value)> {
    let channel = arn("channel", "live-channel");
    vec![
        (
            "the smallest request the model allows: one channel destination",
            json!({ "destinations": [{ "channel": { "channelArn": channel } }] }),
        ),
        (
            "a channel destination naming an encoder configuration, and a name",
            json!({
                "destinations": [{
                    "name": "primary",
                    "channel": {
                        "channelArn": channel,
                        "encoderConfigurationArn": referenced.encoder_configuration_arn,
                    },
                }],
            }),
        ),
        (
            "one S3 recording destination",
            json!({
                "destinations": [{
                    "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                    },
                }],
            }),
        ),
        (
            "an S3 destination with a recording and a thumbnail configuration",
            json!({
                "destinations": [{
                    "name": "archive",
                    "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                        "recordingConfiguration": {
                            "format": "HLS",
                            "hlsConfiguration": { "targetSegmentDurationSeconds": 2 },
                        },
                        "thumbnailConfigurations": [{
                            "targetIntervalSeconds": 60,
                            "storage": ["SEQUENTIAL", "LATEST"],
                        }],
                    },
                }],
            }),
        ),
        (
            "both destinations the model allows at once, and a grid layout",
            json!({
                "destinations": [
                    { "channel": { "channelArn": channel } },
                    {
                        "s3": {
                            "storageConfigurationArn": referenced.storage_configuration_arn,
                            "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                        },
                    },
                ],
                "layout": { "grid": { "videoAspectRatio": "SQUARE", "gridGap": 4 } },
            }),
        ),
        (
            "a picture-in-picture layout and a full tag map",
            json!({
                "destinations": [{ "channel": { "channelArn": channel } }],
                "layout": { "pip": { "pipPosition": "TOP_LEFT", "pipWidth": 320 } },
                "tags": { "env": "test", "team": "media" },
            }),
        ),
        (
            "an explicitly empty tag map",
            json!({
                "destinations": [{ "channel": { "channelArn": channel } }],
                "tags": {},
            }),
        ),
    ]
}

/// `request` with `stageArn` pointing at the stage this store holds.
fn against(referenced: &Referenced, request: &Value) -> Value {
    let mut request = request.clone();
    request["stageArn"] = json!(referenced.stage_arn);
    request
}

// ---------------------------------------------------------------------------
// StartComposition, then GetComposition
// ---------------------------------------------------------------------------

/// What a start persists is what a read reports: for every row of the input class, the
/// composition `StartComposition` answered with is the composition `GetComposition`
/// reports for its ARN, member for member, with a well-formed minted ARN and one id per
/// destination.
#[tokio::test]
async fn a_started_composition_is_read_back_exactly() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "round trip").await;

    for (case, request) in start_cases(&referenced) {
        let request = against(&referenced, &request);
        let started = service.start(case, &request).await;

        let composition_arn = member(&started, "arn");
        assert!(
            matches(COMPOSITION_ARN_PATTERN, &composition_arn)
                && composition_arn.len() <= COMPOSITION_ARN_MAX_LENGTH,
            "case {case:?}: the minted ARN must satisfy the modelled CompositionArn \
             pattern and length, got {composition_arn:?}"
        );
        assert_eq!(
            member(&started, "stageArn"),
            referenced.stage_arn,
            "case {case:?}: the composition must name the stage it composites, got {started}"
        );
        assert_eq!(
            member(&started, "state"),
            STATE_STARTING,
            "case {case:?}: a just-started composition must be {STATE_STARTING}, got {started}"
        );

        let requested: Vec<Value> = request["destinations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let reported = destinations(&started);
        assert_eq!(
            reported.len(),
            requested.len(),
            "case {case:?}: one destination must be reported per requested destination, \
             got {started}"
        );
        let mut seen_ids: Vec<String> = Vec::new();
        for (index, (asked, answered)) in requested.iter().zip(reported.iter()).enumerate() {
            // Every row above either supplies every member of the destination
            // configuration that documents a default or supplies none of the objects
            // those members live in, so the configuration is reported exactly as it
            // arrived. What a PARTIALLY supplied configuration reports is the subject of
            // `composition_destination_configuration_defaults`.
            assert_eq!(
                &answered["configuration"], asked,
                "case {case:?}: destination {index} must report the configuration it was \
                 created from, got {answered}"
            );
            let id = member(answered, "id");
            assert!(
                !id.is_empty() && !seen_ids.contains(&id),
                "case {case:?}: destination {index} must carry an id unique within the \
                 composition, got {id:?} beside {seen_ids:?}"
            );
            seen_ids.push(id);
            assert_eq!(
                member(answered, "state"),
                STATE_STARTING,
                "case {case:?}: destination {index} of a just-started composition must be \
                 {STATE_STARTING}, got {answered}"
            );
            // Only an S3 destination records anything, so only it reports where the
            // recording lands (`Destination$detail` -> `S3Detail$recordingPrefix`).
            let records_to_s3 = asked.get("s3").is_some();
            let prefix = answered["detail"]["s3"]["recordingPrefix"].as_str();
            assert_eq!(
                prefix.is_some(),
                records_to_s3,
                "case {case:?}: destination {index} must report a recording prefix exactly \
                 when it records to S3, got {answered}"
            );
        }

        let expected_tags = request
            .get("tags")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            started["tags"], expected_tags,
            "case {case:?}: the composition must report the tags the request carried, \
             got {started}"
        );
        assert!(
            started.get("startTime").is_some() && started.get("endTime").is_none(),
            "case {case:?}: a running composition reports a start and no end, got {started}"
        );

        let read = service.get(case, &composition_arn).await;
        assert_eq!(
            read, started,
            "case {case:?}: GetComposition must report the composition StartComposition \
             answered with, got {read} against {started}"
        );
    }
}

/// The `STARTING` -> `ACTIVE` transition is owed by the CLOCK, at a span that is not
/// zero: a read inside the span still sees `STARTING`, a read past it sees `ACTIVE`
/// without any further start, `startTime` does not move, and two reads in a row agree.
#[tokio::test]
async fn a_started_composition_becomes_active_when_the_clock_makes_it_due() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "activation").await;

    // One row per destination kind, so the transition is pinned on a composition that
    // broadcasts and on one that records.
    let cases = [
        (
            "a channel destination",
            json!({ "destinations": [{ "channel": { "channelArn": arn("channel", "live") } }] }),
        ),
        (
            "an S3 destination",
            json!({
                "destinations": [{
                    "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                    },
                }],
            }),
        ),
    ];

    for (case, request) in cases {
        let started = service.start(case, &against(&referenced, &request)).await;
        let composition_arn = member(&started, "arn");
        let start_time = started["startTime"].clone();

        tokio::time::sleep(WITHIN_THE_ACTIVATION_SPAN).await;
        let early = service.get(case, &composition_arn).await;
        assert_eq!(
            member(&early, "state"),
            STATE_STARTING,
            "case {case:?}: a read inside the activation span must still report \
             {STATE_STARTING}, got {early}"
        );

        tokio::time::sleep(past_the_activation_span()).await;
        let settled = service.get(case, &composition_arn).await;
        assert_eq!(
            member(&settled, "state"),
            STATE_ACTIVE,
            "case {case:?}: once the clock has made the transition due the composition \
             must be {STATE_ACTIVE}, got {settled}"
        );
        for (index, destination) in destinations(&settled).iter().enumerate() {
            assert_eq!(
                member(destination, "state"),
                STATE_ACTIVE,
                "case {case:?}: destination {index} must settle with its composition, \
                 got {destination}"
            );
        }
        assert_eq!(
            settled["startTime"], start_time,
            "case {case:?}: the transition must not move startTime, got {} against {}",
            settled["startTime"], start_time
        );

        let again = service.get(case, &composition_arn).await;
        assert_eq!(
            again, settled,
            "case {case:?}: a second read must report the same composition, got {again} \
             against {settled}"
        );
    }
}

/// `Composition$layout` is `@required`, so every composition reports one: the documented
/// grid fallback when the request configures no layout, and the documented default for
/// every layout member the request left out.
#[tokio::test]
async fn every_composition_reports_a_layout_with_the_documented_defaults() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "layout").await;
    let channel = json!([{ "channel": { "channelArn": arn("channel", "live") } }]);

    // (case, the `layout` the request carries, the layout the composition must report)
    let cases: Vec<(&str, Option<Value>, Value)> = vec![
        (
            "no layout at all: the documented grid fallback",
            None,
            json!({ "grid": {
                "featuredParticipantAttribute": NO_FEATURED_PARTICIPANT,
                "omitStoppedVideo": false,
                "videoAspectRatio": DEFAULT_GRID_ASPECT_RATIO,
                "videoFillMode": DEFAULT_GRID_FILL_MODE,
                "gridGap": DEFAULT_GRID_GAP,
            }}),
        ),
        (
            "an empty layout object",
            Some(json!({})),
            json!({ "grid": {
                "featuredParticipantAttribute": NO_FEATURED_PARTICIPANT,
                "omitStoppedVideo": false,
                "videoAspectRatio": DEFAULT_GRID_ASPECT_RATIO,
                "videoFillMode": DEFAULT_GRID_FILL_MODE,
                "gridGap": DEFAULT_GRID_GAP,
            }}),
        ),
        (
            "an empty grid",
            Some(json!({ "grid": {} })),
            json!({ "grid": {
                "featuredParticipantAttribute": NO_FEATURED_PARTICIPANT,
                "omitStoppedVideo": false,
                "videoAspectRatio": DEFAULT_GRID_ASPECT_RATIO,
                "videoFillMode": DEFAULT_GRID_FILL_MODE,
                "gridGap": DEFAULT_GRID_GAP,
            }}),
        ),
        (
            "a grid naming one member the model documents a default for",
            Some(json!({ "grid": { "videoAspectRatio": "PORTRAIT" } })),
            json!({ "grid": {
                "featuredParticipantAttribute": NO_FEATURED_PARTICIPANT,
                "omitStoppedVideo": false,
                "videoAspectRatio": "PORTRAIT",
                "videoFillMode": DEFAULT_GRID_FILL_MODE,
                "gridGap": DEFAULT_GRID_GAP,
            }}),
        ),
        (
            "a fully configured grid",
            Some(json!({ "grid": {
                "featuredParticipantAttribute": "host",
                "omitStoppedVideo": true,
                "videoAspectRatio": "AUTO",
                "videoFillMode": "CONTAIN",
                "gridGap": 12,
                "participantOrderAttribute": "seat",
            }})),
            json!({ "grid": {
                "featuredParticipantAttribute": "host",
                "omitStoppedVideo": true,
                "videoAspectRatio": "AUTO",
                "videoFillMode": "CONTAIN",
                "gridGap": 12,
                "participantOrderAttribute": "seat",
            }}),
        ),
        (
            "an empty picture-in-picture layout",
            Some(json!({ "pip": {} })),
            json!({ "pip": {
                "featuredParticipantAttribute": NO_FEATURED_PARTICIPANT,
                "omitStoppedVideo": false,
                "videoFillMode": DEFAULT_PIP_FILL_MODE,
                "gridGap": DEFAULT_PIP_GRID_GAP,
                "pipParticipantAttribute": NO_PIP_PARTICIPANT,
                "pipBehavior": DEFAULT_PIP_BEHAVIOR,
                "pipOffset": DEFAULT_PIP_OFFSET,
                "pipPosition": DEFAULT_PIP_POSITION,
            }}),
        ),
        (
            "a picture-in-picture layout naming a corner and a width",
            Some(json!({ "pip": { "pipPosition": "TOP_RIGHT", "pipWidth": 240 } })),
            json!({ "pip": {
                "featuredParticipantAttribute": NO_FEATURED_PARTICIPANT,
                "omitStoppedVideo": false,
                "videoFillMode": DEFAULT_PIP_FILL_MODE,
                "gridGap": DEFAULT_PIP_GRID_GAP,
                "pipParticipantAttribute": NO_PIP_PARTICIPANT,
                "pipBehavior": DEFAULT_PIP_BEHAVIOR,
                "pipOffset": DEFAULT_PIP_OFFSET,
                "pipPosition": "TOP_RIGHT",
                "pipWidth": 240,
            }}),
        ),
        (
            "a fully configured picture-in-picture layout",
            Some(json!({ "pip": {
                "featuredParticipantAttribute": "host",
                "omitStoppedVideo": true,
                "videoFillMode": "FILL",
                "gridGap": 3,
                "pipParticipantAttribute": "guest",
                "pipBehavior": "DYNAMIC",
                "pipOffset": 8,
                "pipPosition": "BOTTOM_LEFT",
                "pipWidth": 320,
                "pipHeight": 180,
                "participantOrderAttribute": "seat",
            }})),
            json!({ "pip": {
                "featuredParticipantAttribute": "host",
                "omitStoppedVideo": true,
                "videoFillMode": "FILL",
                "gridGap": 3,
                "pipParticipantAttribute": "guest",
                "pipBehavior": "DYNAMIC",
                "pipOffset": 8,
                "pipPosition": "BOTTOM_LEFT",
                "pipWidth": 320,
                "pipHeight": 180,
                "participantOrderAttribute": "seat",
            }}),
        ),
    ];

    for (case, layout, expected) in cases {
        let mut request = json!({
            "stageArn": referenced.stage_arn,
            "destinations": channel,
        });
        if let Some(layout) = layout {
            request["layout"] = layout;
        }
        let started = service.start(case, &request).await;
        assert_eq!(
            started["layout"], expected,
            "case {case:?}: expected layout {expected}, got {}",
            started["layout"]
        );
        // The same layout comes back on a read, so the defaults were PERSISTED and not
        // recomputed differently by the reporting path.
        let read = service.get(case, &member(&started, "arn")).await;
        assert_eq!(
            read["layout"], expected,
            "case {case:?}: GetComposition must report the same layout, got {}",
            read["layout"]
        );
    }
}

/// Every refusal reachable through these two operations, over a table: a resource this
/// store does not hold is the modelled 404, a member outside its modelled constraints is
/// the modelled 400, and neither leaves a composition behind.
#[tokio::test]
async fn start_composition_separates_an_unknown_resource_from_a_malformed_request() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "refusals").await;
    let channel = arn("channel", "live");
    let unknown_stage = arn("stage", "nosuchstage");
    let unknown_encoder = arn("encoder-configuration", "nosuchencoder");
    let unknown_storage = arn("storage-configuration", "nosuchstorage");

    // (case, the request, the expected status, the expected wire error code)
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a stage ARN this store holds no stage for",
            json!({
                "stageArn": unknown_stage,
                "destinations": [{ "channel": { "channelArn": channel } }],
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stage ARN outside the modelled StageArn pattern",
            json!({
                "stageArn": "not-an-arn",
                "destinations": [{ "channel": { "channelArn": channel } }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a channel destination naming an unknown encoder configuration",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "channel": {
                    "channelArn": channel,
                    "encoderConfigurationArn": unknown_encoder,
                }}],
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an S3 destination naming an unknown storage configuration",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "s3": {
                    "storageConfigurationArn": unknown_storage,
                    "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                }}],
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an S3 destination naming an unknown encoder configuration",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "s3": {
                    "storageConfigurationArn": referenced.storage_configuration_arn,
                    "encoderConfigurationArns": [unknown_encoder],
                }}],
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a destination naming neither a channel nor an S3 recording",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "name": "nowhere" }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a destination naming both a channel and an S3 recording",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{
                    "channel": { "channelArn": channel },
                    "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                    },
                }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a layout configuring both a grid and a picture-in-picture window",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "channel": { "channelArn": channel } }],
                "layout": { "grid": {}, "pip": {} },
            }),
            400,
            "ValidationException",
        ),
        (
            "more destinations than the modelled list maximum",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [
                    { "channel": { "channelArn": arn("channel", "a") } },
                    { "channel": { "channelArn": arn("channel", "b") } },
                    { "channel": { "channelArn": arn("channel", "c") } },
                ],
            }),
            400,
            "ValidationException",
        ),
        (
            "no destination at all, which the modelled list minimum refuses",
            json!({ "stageArn": referenced.stage_arn, "destinations": [] }),
            400,
            "ValidationException",
        ),
        (
            "a tag key in the namespace AWS reserves for its own tags",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "channel": { "channelArn": channel } }],
                "tags": { "aws:owner": "platform" },
            }),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let (status, response) = service.call("StartComposition", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: StartComposition {request} must answer HTTP {expected_status}, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected the wire error code {expected_code}, got {response}"
        );
        assert!(
            !member(&response, "message").is_empty(),
            "case {case:?}: the refusal must state why, got {response}"
        );
    }

    // A refused start leaves nothing behind: a composition ARN of the shape every
    // refusal above would have minted names nothing.
    let (status, response) = service
        .call("GetComposition", &json!({ "arn": arn("composition", "cp000000") }))
        .await;
    assert_eq!(
        status, 404,
        "a composition ARN this store holds nothing for must be the modelled 404, \
         got {status} {response}"
    );

    // GetComposition separates the same two answers.
    let get_cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an unknown but well-formed composition ARN",
            json!({ "arn": arn("composition", "cp000001") }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN outside the modelled CompositionArn pattern",
            json!({ "arn": "arn:aws:ivs:us-east-1:000000000000:stage/not-a-composition" }),
            400,
            "ValidationException",
        ),
        (
            "an empty ARN, which the modelled length minimum refuses",
            json!({ "arn": "" }),
            400,
            "ValidationException",
        ),
    ];
    for (case, request, expected_status, expected_code) in get_cases {
        let (status, response) = service.call("GetComposition", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: GetComposition {request} must answer HTTP {expected_status}, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected the wire error code {expected_code}, got {response}"
        );
    }
}

/// `idempotencyToken` carries `@idempotencyToken`, so the token is one of the attributes a
/// start asks for: offering it again names the composition the first call created, which
/// already exists — the modelled `ConflictException` (HTTP 409) stating that, no second
/// composition, and the first one left exactly as it was. A different token is a different
/// request and starts a composition of its own.
#[tokio::test]
async fn repeating_a_start_under_one_idempotency_token_starts_one_composition() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "idempotency").await;
    let channel = arn("channel", "live");

    // The tokens span the member's modelled `@length(1..64)` and `@pattern
    // ^[a-zA-Z0-9-_]*$` (`smithy_query(query="operation",operation="StartComposition")`):
    // both published length bounds and every character class the pattern allows.
    let tokens = [
        ("the shortest token the model allows", "a".to_string()),
        ("a mixed-case token with both allowed separators", "Retry_9-A".to_string()),
        ("a token at the modelled 64-character maximum", "t".repeat(64)),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (case, token) in tokens {
        let request = json!({
            "stageArn": referenced.stage_arn,
            "destinations": [{ "channel": { "channelArn": channel } }],
            "idempotencyToken": token,
        });
        let first = service.start(case, &request).await;
        let (status, response) = service.call("StartComposition", &request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: repeating the start under {token:?} must answer HTTP 409, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: the refusal must be the modelled conflict, got {response}"
        );
        assert_eq!(
            member(&response, "message"),
            DUPLICATE_COMPOSITION_MESSAGE,
            "case {case:?}: the refusal must state {DUPLICATE_COMPOSITION_MESSAGE:?}, \
             got {response}"
        );
        let arn = member(&first, "arn");
        // The refused repeat started nothing and moved nothing: the composition the token
        // first named is still the one it names, with the start time it was minted with.
        let read = service.get(case, &arn).await;
        assert_eq!(
            read["startTime"], first["startTime"],
            "case {case:?}: the refused repeat must not restart the composition, got {} \
             against {}",
            read["startTime"], first["startTime"]
        );
        assert!(
            !minted.contains(&arn),
            "case {case:?}: a different token must start a different composition, \
             got {arn:?} beside {minted:?}"
        );
        minted.push(arn);
    }

    // Each token started exactly one composition, and each of them is still readable.
    for arn in &minted {
        let read = service.get("idempotency", arn).await;
        assert_eq!(
            member(&read, "arn"),
            *arn,
            "the composition {arn:?} must read back as itself, got {read}"
        );
    }
    let listed = service.held_compositions("idempotency").await;
    assert_eq!(
        listed, minted.len(),
        "the refused repeats must have started no composition, got {listed} against {}",
        minted.len()
    );
}

/// A token stands for ONE request, so a start that offers one already used is refused
/// whatever else it carries — and WHICH refusal says so follows the rest of the request: a
/// repeat of the same request is the composition that already exists, while a DIFFERENT
/// request under that token is a token standing for two requests. The rows differ in WHICH
/// part of the request changed.
#[tokio::test]
async fn reusing_an_idempotency_token_for_a_different_request_is_a_conflict() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "token reuse").await;
    let other_stage = service.stage("token reuse").await;
    let channel = arn("channel", "live");
    let token = "shared-token";

    let first = json!({
        "stageArn": referenced.stage_arn,
        "destinations": [{ "channel": { "channelArn": channel } }],
        "idempotencyToken": token,
    });
    let started = service.start("the first use of the token", &first).await;

    let reused_message = format!(
        "StartComposition.idempotencyToken is required to name the request it was first \
         used for, found {token} which already names a different request"
    );
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "the same token against another stage",
            json!({
                "stageArn": other_stage,
                "destinations": [{ "channel": { "channelArn": channel } }],
                "idempotencyToken": token,
            }),
            &reused_message,
        ),
        (
            "the same token with another destination",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "channel": { "channelArn": arn("channel", "elsewhere") } }],
                "idempotencyToken": token,
            }),
            &reused_message,
        ),
        (
            "the same token with another layout",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "channel": { "channelArn": channel } }],
                "layout": { "pip": {} },
                "idempotencyToken": token,
            }),
            &reused_message,
        ),
        (
            "the same token with another tag set",
            json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "channel": { "channelArn": channel } }],
                "tags": { "env": "test" },
                "idempotencyToken": token,
            }),
            &reused_message,
        ),
        (
            "the same token for the very same request",
            first.clone(),
            DUPLICATE_COMPOSITION_MESSAGE,
        ),
    ];

    for (case, request, expected_message) in cases {
        let (status, response) = service.call("StartComposition", &request).await;
        assert_eq!(
            status, 409,
            "case {case:?}: StartComposition {request} must answer HTTP 409, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected the modelled conflict code, got {response}"
        );
        assert_eq!(
            member(&response, "message"),
            expected_message,
            "case {case:?}: the refusal must state {expected_message:?}, got {response}"
        );
    }

    // The refused reuses changed nothing: the composition the token first named is
    // still the one it names.
    let read = service.get("after the refused reuses", &member(&started, "arn")).await;
    assert_eq!(
        read["stageArn"], started["stageArn"],
        "the refused reuses must leave the first composition as it was, got {read} \
         against {started}"
    );
    // A tag set spelled in another order is the SAME request, so the token names the
    // composition that already exists rather than standing for a second request.
    let tagged = json!({
        "stageArn": referenced.stage_arn,
        "destinations": [{ "channel": { "channelArn": channel } }],
        "tags": { "a": "1", "b": "2" },
        "idempotencyToken": "ordered-tags",
    });
    let reordered = json!({
        "stageArn": referenced.stage_arn,
        "destinations": [{ "channel": { "channelArn": channel } }],
        "tags": { "b": "2", "a": "1" },
        "idempotencyToken": "ordered-tags",
    });
    service.start("a tag set in one order", &tagged).await;
    let (status, response) = service.call("StartComposition", &reordered).await;
    assert_eq!(
        status, 409,
        "the same tag set in another order is the same request, got {status} {response}"
    );
    assert_eq!(
        member(&response, "message"),
        DUPLICATE_COMPOSITION_MESSAGE,
        "a tag set spelled in another order is the same request, got {response}"
    );
}

/// A composition is a resource of this endpoint: its tags round-trip through the shared
/// `/tags/{resourceArn}` surface, and the composition goes away with the stage it
/// composites.
#[tokio::test]
async fn a_composition_is_taggable_and_does_not_outlive_its_stage() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "tagging").await;
    let channel = arn("channel", "live");

    // One row per tag map, so the rule is pinned over the count the request carries.
    let cases: Vec<(&str, Value)> = vec![
        ("no tags at all", json!({})),
        ("one tag", json!({ "env": "test" })),
        (
            "several tags, including the empty value TagValue admits",
            json!({ "env": "test", "team": "media", "note": "" }),
        ),
    ];

    for (case, tags) in cases {
        let started = service
            .start(
                case,
                &json!({
                    "stageArn": referenced.stage_arn,
                    "destinations": [{ "channel": { "channelArn": channel } }],
                    "tags": tags,
                }),
            )
            .await;
        let composition_arn = member(&started, "arn");
        let (status, response) = service.list_tags(&composition_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource on {composition_arn} must answer HTTP 200, \
             got {status} {response}"
        );
        assert_eq!(
            response["tags"], tags,
            "case {case:?}: the tagging surface must report the tags the start persisted, \
             got {response}"
        );
        let expected: HashMap<String, String> = serde_json::from_value(tags.clone())
            .expect("the test tag map is a string map");
        let read: HashMap<String, String> = serde_json::from_value(started["tags"].clone())
            .expect("the reported tag map is a string map");
        assert_eq!(
            read, expected,
            "case {case:?}: the composition and the tagging surface must agree, \
             got {read:?} against {expected:?}"
        );
    }

    // Deleting the stage disconnects every participant, which is one of the conditions
    // the model's own StartComposition doc ends a composition on: nothing composites a
    // stage that no longer exists.
    let started = service
        .start(
            "teardown",
            &json!({
                "stageArn": referenced.stage_arn,
                "destinations": [{ "channel": { "channelArn": channel } }],
            }),
        )
        .await;
    let composition_arn = member(&started, "arn");
    let (status, response) = service
        .call("DeleteStage", &json!({ "arn": referenced.stage_arn }))
        .await;
    assert_eq!(
        status, 200,
        "DeleteStage must answer HTTP 200, got {status} {response}"
    );
    let (status, response) = service
        .call("GetComposition", &json!({ "arn": composition_arn }))
        .await;
    assert_eq!(
        status, 404,
        "a composition must not outlive the stage it composites, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "expected the modelled not-found code, got {response}"
    );
}
