// SPDX-License-Identifier: CC0-1.0

//! `GetComposition` end to end: what it REPORTS for one ARN, and what reading it does
//! not do.
//!
//! Every case goes through the router's own `dispatch` with a restJson1 request, exactly
//! as a signed `aws ivs-realtime get-composition` call arrives, so what is asserted is
//! the emulator's HTTP behaviour and never a handler called directly.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. ONE ARN NAMES ONE COMPOSITION. With several compositions live in a single store at
//!    once, every read answers the record its `arn` names and no neighbour's — its own
//!    stage, its own destinations with their own minted ids, its own layout and its own
//!    tags — whatever ORDER the reads arrive in and however many times each is repeated.
//!    A well-formed `CompositionArn` the store holds no record for is the modelled
//!    `ResourceNotFoundException` (HTTP 404), while every neighbouring ARN still reads
//!    HTTP 200 unchanged.
//! 2. READING IS NOT A WRITE. The `STARTING` -> `ACTIVE` transition is owed by the CLOCK:
//!    a composition polled repeatedly inside the activation span is still `STARTING`
//!    there however many reads it took, and a composition NOBODY read is `ACTIVE` on its
//!    very first read once the span has passed. No read moves `startTime`, mints a second
//!    destination id, or ends a composition.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries (`engine::mock_account_id`), and
/// so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every case signs for.
const REGION: &str = "us-east-1";

// The `CompositionState` values these cases assert, from
// `smithy_query(query="shape",shape="CompositionState")`:
//   STARTING | ACTIVE | STOPPING | FAILED | STOPPED
const STATE_STARTING: &str = "STARTING";
const STATE_ACTIVE: &str = "ACTIVE";

/// The modelled error a request naming a composition this store does not hold is refused
/// with, from `smithy_query(query="operation",operation="GetComposition")` — HTTP 404.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// Slack added to (or held back from) a span so a scheduling delay on the test host
/// cannot decide an assertion.
const SLACK: Duration = Duration::from_millis(400);

/// Long enough that the `STARTING` -> `ACTIVE` transition the clock owes has fallen due.
fn past_the_activation_span() -> Duration {
    state::COMPOSITION_ACTIVATION_SPAN + SLACK
}

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
    /// scope naming `ivs` in [`REGION`].
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

    /// Create a stage named `name` and answer with the ARN the service minted for it.
    async fn stage(&self, case: &str, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStage must answer HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Create an encoder configuration and answer with its ARN.
    async fn encoder_configuration(&self, case: &str) -> String {
        let (status, response) = self
            .call("CreateEncoderConfiguration", &json!({ "name": "enc" }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        member(&response["encoderConfiguration"], "arn")
    }

    /// Create a storage configuration and answer with its ARN.
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
        member(&response["storageConfiguration"], "arn")
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

/// An IVS ARN of `resource_type` in [`REGION`], naming the resource `id`.
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

/// The ids the service minted for a composition's destinations.
fn destination_ids(composition: &Value) -> Vec<String> {
    destinations(composition)
        .iter()
        .map(|destination| member(destination, "id"))
        .collect()
}

/// One row of the class: the start request, and the composition the store answered it
/// with.
struct Live {
    case: &'static str,
    request: Value,
    started: Value,
    arn: String,
}

/// The resources the rows refer to, resolved against this store.
struct Referenced {
    first_stage_arn: String,
    second_stage_arn: String,
    encoder_configuration_arn: String,
    storage_configuration_arn: String,
}

impl Referenced {
    async fn resolve(service: &RealTime, case: &str) -> Self {
        Self {
            first_stage_arn: service.stage(case, "composited-one").await,
            second_stage_arn: service.stage(case, "composited-two").await,
            encoder_configuration_arn: service.encoder_configuration(case).await,
            storage_configuration_arn: service.storage_configuration(case).await,
        }
    }
}

/// The start requests the reads below span the class of compositions with: the rows differ
/// in WHICH stage is composited, how many destinations the composition has, which KIND
/// each is (a low-latency channel or an S3 recording), which layout it configures and
/// whether it carries tags — every dimension a read could confuse one record with
/// another over.
fn live_cases(referenced: &Referenced) -> Vec<(&'static str, Value)> {
    let channel = arn("channel", "live-channel");
    let other_channel = arn("channel", "second-channel");
    vec![
        (
            "one channel destination, no layout, no tags",
            json!({
                "stageArn": referenced.first_stage_arn,
                "destinations": [{ "channel": { "channelArn": channel } }],
            }),
        ),
        (
            "one channel destination with an encoder configuration, a grid layout and tags",
            json!({
                "stageArn": referenced.first_stage_arn,
                "destinations": [{
                    "name": "primary",
                    "channel": {
                        "channelArn": channel,
                        "encoderConfigurationArn": referenced.encoder_configuration_arn,
                    },
                }],
                "layout": { "grid": { "videoAspectRatio": "SQUARE", "gridGap": 4 } },
                "tags": { "env": "test" },
            }),
        ),
        (
            "one S3 recording destination and a picture-in-picture layout",
            json!({
                "stageArn": referenced.first_stage_arn,
                "destinations": [{
                    "name": "archive",
                    "s3": {
                        "storageConfigurationArn": referenced.storage_configuration_arn,
                        "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                    },
                }],
                "layout": { "pip": { "pipPosition": "TOP_LEFT", "pipWidth": 320 } },
            }),
        ),
        (
            "both destinations the model allows at once, and a full tag map",
            json!({
                "stageArn": referenced.first_stage_arn,
                "destinations": [
                    { "channel": { "channelArn": other_channel } },
                    {
                        "s3": {
                            "storageConfigurationArn": referenced.storage_configuration_arn,
                            "encoderConfigurationArns": [referenced.encoder_configuration_arn],
                        },
                    },
                ],
                "tags": { "env": "test", "team": "media" },
            }),
        ),
        (
            "one channel destination on a SECOND stage",
            json!({
                "stageArn": referenced.second_stage_arn,
                "destinations": [{ "channel": { "channelArn": channel } }],
            }),
        ),
    ]
}

/// Start every row of the class in ONE store and answer with what each start reported.
async fn start_the_class(service: &RealTime, referenced: &Referenced) -> Vec<Live> {
    let mut live = Vec::new();
    for (case, request) in live_cases(referenced) {
        let started = service.start(case, &request).await;
        let composition_arn = member(&started, "arn");
        assert!(
            !composition_arn.is_empty(),
            "case {case:?}: StartComposition must mint an ARN, got {started}"
        );
        live.push(Live {
            case,
            request,
            started,
            arn: composition_arn,
        });
    }
    live
}

// ---------------------------------------------------------------------------
// Rule 1: one ARN names one composition
// ---------------------------------------------------------------------------

/// Several compositions live at once: each read reports the record its ARN names, with
/// that row's stage, destinations, layout and tags — in any order, repeated any number of
/// times — and an ARN the store holds no record for is the modelled 404 that leaves every
/// neighbour readable.
#[tokio::test]
async fn get_composition_reports_the_composition_its_arn_names_and_no_other() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "one arn one composition").await;
    let live = start_the_class(&service, &referenced).await;

    // Past the activation span first, so every row is settled: what the reads below
    // differ in is the ARN asked for and the order asked in, nothing else.
    tokio::time::sleep(past_the_activation_span()).await;

    let mut first_read: HashMap<String, Value> = HashMap::new();
    for row in &live {
        let case = row.case;
        let read = service.get(case, &row.arn).await;

        assert_eq!(
            member(&read, "arn"),
            row.arn,
            "case {case:?}: the report must name the composition asked for, got {read}"
        );
        assert_eq!(
            member(&read, "stageArn"),
            member(&row.request, "stageArn"),
            "case {case:?}: the report must name the stage this composition composites, \
             got {read}"
        );
        assert_eq!(
            read["tags"],
            row.request.get("tags").cloned().unwrap_or_else(|| json!({})),
            "case {case:?}: the report must carry this composition's own tags, got {read}"
        );
        assert_eq!(
            read["layout"], row.started["layout"],
            "case {case:?}: the report must carry this composition's own layout, got {} \
             against {}",
            read["layout"], row.started["layout"]
        );

        let requested = destinations(&row.request);
        let reported = destinations(&read);
        assert_eq!(
            reported.len(),
            requested.len(),
            "case {case:?}: one destination must be reported per requested destination, \
             got {read}"
        );
        for (index, (asked, answered)) in requested.iter().zip(reported.iter()).enumerate() {
            assert_eq!(
                &answered["configuration"], asked,
                "case {case:?}: destination {index} must report the configuration it was \
                 created from, got {answered}"
            );
        }
        assert_eq!(
            destination_ids(&read),
            destination_ids(&row.started),
            "case {case:?}: the report must carry the destination ids the start minted, \
             got {read}"
        );

        first_read.insert(row.arn.clone(), read);
    }

    // No two rows are the same resource: distinct ARNs, and no id of one composition's
    // destinations reported for another's.
    for (left_index, left) in live.iter().enumerate() {
        for right in live.iter().skip(left_index + 1) {
            assert_ne!(
                left.arn, right.arn,
                "cases {:?} and {:?}: two starts must be two compositions, both got {}",
                left.case, right.case, left.arn
            );
            let left_read = &first_read[&left.arn];
            let right_read = &first_read[&right.arn];
            assert_ne!(
                left_read, right_read,
                "cases {:?} and {:?}: two compositions must not read back identically, \
                 both got {left_read}",
                left.case, right.case
            );
        }
    }

    // The same ARNs again, in a sequence that is neither the start order nor one read per
    // composition: forward, backward, then a few repeats. Order and repeat count are what
    // the rule generalises over, so a report that depended on either shows up here.
    let order: Vec<usize> = (0..live.len())
        .chain((0..live.len()).rev())
        .chain([live.len() - 1, 0, live.len() / 2, 0])
        .collect();
    for (visit, index) in order.into_iter().enumerate() {
        let row = &live[index];
        let case = row.case;
        let read = service.get(case, &row.arn).await;
        assert_eq!(
            read, first_read[&row.arn],
            "case {case:?}: read {visit} of {} must report exactly what the first read \
             did, got {read} against {}",
            row.arn, first_read[&row.arn]
        );
    }

    // A well-formed CompositionArn no start minted: the modelled 404, worded with the
    // identifier the request named.
    let absent = arn("composition", "aaaaaaaaaaaa");
    let (status, response) = service
        .call("GetComposition", &json!({ "arn": absent }))
        .await;
    assert_eq!(
        status, 404,
        "case \"an ARN this store holds no composition for\": GetComposition {absent} \
         must answer HTTP 404, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        RESOURCE_NOT_FOUND,
        "case \"an ARN this store holds no composition for\": the refusal must carry the \
         modelled error code, got {response}"
    );
    assert_eq!(
        member(&response, "message"),
        state::resource_not_found_message(&absent),
        "case \"an ARN this store holds no composition for\": the refusal must name the \
         identifier the request carried, got {response}"
    );

    // The refusal changed nothing: every neighbour still reads exactly as before.
    for row in &live {
        let case = row.case;
        let read = service.get(case, &row.arn).await;
        assert_eq!(
            read, first_read[&row.arn],
            "case {case:?}: a refused read of another ARN must leave this composition as \
             it was, got {read} against {}",
            first_read[&row.arn]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: reading is not a write
// ---------------------------------------------------------------------------

/// The `STARTING` -> `ACTIVE` transition is the clock's, not a reader's: a composition
/// polled throughout the activation span is still `STARTING` there, a composition nobody
/// read is `ACTIVE` on its first read once the span has passed, and no read moves
/// `startTime`, re-mints a destination id or ends anything.
#[tokio::test]
async fn polling_a_composition_neither_hastens_nor_delays_the_transition_the_clock_owes() {
    let service = RealTime::new();
    let referenced = Referenced::resolve(&service, "reading is not a write").await;

    // Two compositions of DIFFERENT kinds (one broadcasts, one records) started together
    // and treated differently: the first is polled throughout the span, the second is not
    // read until the span has passed.
    let polled_request = json!({
        "stageArn": referenced.first_stage_arn,
        "destinations": [{ "channel": { "channelArn": arn("channel", "polled") } }],
    });
    let untouched_request = json!({
        "stageArn": referenced.second_stage_arn,
        "destinations": [{
            "s3": {
                "storageConfigurationArn": referenced.storage_configuration_arn,
                "encoderConfigurationArns": [referenced.encoder_configuration_arn],
            },
        }],
    });

    let polled_case = "a composition polled throughout the activation span";
    let untouched_case = "a composition nobody read until the span had passed";
    let polled = service.start(polled_case, &polled_request).await;
    let untouched = service.start(untouched_case, &untouched_request).await;
    let started_at = Instant::now();

    let polled_arn = member(&polled, "arn");
    let untouched_arn = member(&untouched, "arn");
    let polled_start_time = polled["startTime"].clone();
    let polled_destination_ids = destination_ids(&polled);

    // Poll the first composition. Only a read the clock cannot yet have settled is
    // asserted on, so a stalled test host weakens no assertion instead of failing one.
    let mut reads_inside_the_span = 0_usize;
    for poll in 0..5_u32 {
        let read = service.get(polled_case, &polled_arn).await;
        if started_at.elapsed() + SLACK < state::COMPOSITION_ACTIVATION_SPAN {
            reads_inside_the_span += 1;
            assert_eq!(
                member(&read, "state"),
                STATE_STARTING,
                "case {polled_case:?}: poll {poll} falls inside the activation span, so \
                 the composition must still be {STATE_STARTING}, got {read}"
            );
            assert!(
                read.get("endTime").is_none(),
                "case {polled_case:?}: poll {poll} must not end a running composition, \
                 got {read}"
            );
        }
        assert_eq!(
            read["startTime"], polled_start_time,
            "case {polled_case:?}: poll {poll} must not move startTime, got {} against {}",
            read["startTime"], polled_start_time
        );
        assert_eq!(
            destination_ids(&read),
            polled_destination_ids,
            "case {polled_case:?}: poll {poll} must not re-mint the destination ids, \
             got {read}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        reads_inside_the_span > 0,
        "case {polled_case:?}: the activation span must be observable, got {} polls \
         inside it",
        reads_inside_the_span
    );

    tokio::time::sleep(past_the_activation_span()).await;

    // The composition nobody read settled anyway: its FIRST read is already ACTIVE.
    let first_read_of_untouched = service.get(untouched_case, &untouched_arn).await;
    assert_eq!(
        member(&first_read_of_untouched, "state"),
        STATE_ACTIVE,
        "case {untouched_case:?}: the transition is owed by the clock, so the first read \
         past the span must already be {STATE_ACTIVE}, got {first_read_of_untouched}"
    );
    assert_eq!(
        first_read_of_untouched["startTime"], untouched["startTime"],
        "case {untouched_case:?}: settling must not move startTime, got {} against {}",
        first_read_of_untouched["startTime"], untouched["startTime"]
    );
    let read_again = service.get(untouched_case, &untouched_arn).await;
    assert_eq!(
        read_again, first_read_of_untouched,
        "case {untouched_case:?}: a second read must report the same composition, got \
         {read_again} against {first_read_of_untouched}"
    );

    // The composition that WAS polled settled at the same span, no sooner and no later.
    let settled_poll = service.get(polled_case, &polled_arn).await;
    assert_eq!(
        member(&settled_poll, "state"),
        STATE_ACTIVE,
        "case {polled_case:?}: being polled must not keep a composition from settling, \
         got {settled_poll}"
    );
    assert_eq!(
        settled_poll["startTime"], polled_start_time,
        "case {polled_case:?}: the whole poll sequence must leave startTime where the \
         start put it, got {} against {}",
        settled_poll["startTime"], polled_start_time
    );
    assert_eq!(
        destination_ids(&settled_poll),
        polled_destination_ids,
        "case {polled_case:?}: the whole poll sequence must leave the destination ids the \
         start minted, got {settled_poll}"
    );
}
