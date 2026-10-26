// SPDX-License-Identifier: CC0-1.0

//! `ListCompositions` over the wire: the page of compositions this store holds.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. A LISTING REPORTS THE STORE. Every composition `StartComposition` persisted, and no
//!    other, is reported in start order, and each summary agrees member for member with
//!    what `GetComposition` answers for that same composition.
//! 2. EACH FILTER NARROWS BY ITS OWN DOC. `filterByStageArn` keeps the compositions of
//!    that stage; `filterByEncoderConfigurationArn` keeps the ones with that encoder
//!    configuration attached to at least one output; both together keep the ones that
//!    satisfy both; neither keeps every composition. A filter nothing matches is an empty
//!    page, not a refusal.
//! 3. PAGINATION. A page holds at most `maxResults` compositions, ends with a `nextToken`
//!    only while compositions are left over, and that token resumes exactly where the
//!    page ended — so paging the whole listing reports each composition once, in start
//!    order. `maxResults` outside the modelled range is refused.
//! 4. THE CLOCK, NOT THE READ. A page reports the state the clock owes each composition
//!    (`STARTING` before the activation span is up, `ACTIVE` after it, `STOPPING` after a
//!    stop) and stops reporting a composition once the clock has reclaimed the ephemeral
//!    resource — and listing twice at one instant answers twice the same.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::{
    state, EmulatedService,
};

/// The account id every ARN this emulator mints carries (`engine::emulator_account_id`),
/// and so the account every ARN built by hand here carries too.
const ACCOUNT: &str = "000000000000";

/// The region every test here signs for.
const REGION: &str = "us-east-1";

/// The `CompositionState` values this suite asserts, from
/// `smithy_query(query="shape",shape="CompositionState")`:
/// `STARTING | ACTIVE | STOPPING | FAILED | STOPPED`.
const STATE_STARTING: &str = "STARTING";
const STATE_ACTIVE: &str = "ACTIVE";
const STATE_STOPPING: &str = "STOPPING";

/// The modelled `@range` of `ListCompositionsRequest$maxResults`
/// (`MaxCompositionResults`, from
/// `smithy_query(query="operation",operation="ListCompositions")`).
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// Long enough that a transition the emulator owes has fallen due, whatever scheduling
/// delay the test host adds.
fn past(span: Duration) -> Duration {
    span + Duration::from_millis(400)
}

/// Shorter than the activation span, to pin that the span is NOT zero: a caller listing
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
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
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

    /// Create a stage and answer with the ARN the service minted for it.
    async fn stage(&self, name: &str) -> String {
        let (status, response) = self.call("CreateStage", &json!({ "name": name })).await;
        assert_eq!(
            status, 200,
            "fixture CreateStage {name:?}: expected HTTP 200, got {status} {response}"
        );
        member(&response["stage"], "arn")
    }

    /// Create an encoder configuration and answer with its ARN.
    async fn encoder_configuration(&self, name: &str) -> String {
        let (status, response) = self
            .call("CreateEncoderConfiguration", &json!({ "name": name }))
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateEncoderConfiguration {name:?}: expected HTTP 200, \
             got {status} {response}"
        );
        member(&response["encoderConfiguration"], "arn")
    }

    /// Create a storage configuration and answer with its ARN.
    async fn storage_configuration(&self, bucket: &str) -> String {
        let (status, response) = self
            .call(
                "CreateStorageConfiguration",
                &json!({ "s3": { "bucketName": bucket } }),
            )
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateStorageConfiguration {bucket:?}: expected HTTP 200, \
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
            "fixture StartComposition {case:?} {body}: expected HTTP 200, \
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

    /// `ListCompositions`, asserting the modelled success, and answering with the whole
    /// response body so a caller can read `nextToken` off it too.
    async fn list(&self, case: &str, request: &Value) -> Value {
        let (status, response) = self.call("ListCompositions", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListCompositions {request} must answer HTTP 200, \
             got {status} {response}"
        );
        assert!(
            response["compositions"].is_array(),
            "case {case:?}: compositions is @required, so a page always carries the \
             list, got {response}"
        );
        response
    }

    /// The `arn` of every composition one `ListCompositions` page reports, in the order
    /// it reported them.
    async fn listed_arns(&self, case: &str, request: &Value) -> Vec<String> {
        let response = self.list(case, request).await;
        summaries(&response)
            .iter()
            .map(|summary| member(summary, "arn"))
            .collect()
    }

    /// Every composition the listing reports, walked page by page under `page_size`.
    async fn every_page(&self, case: &str, page_size: i64) -> Vec<String> {
        let mut seen: Vec<String> = Vec::new();
        let mut request = json!({ "maxResults": page_size });
        loop {
            let response = self.list(case, &request).await;
            let page = summaries(&response);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or(usize::MAX),
                "case {case:?}: a page holds at most maxResults={page_size} \
                 compositions, got {}",
                page.len()
            );
            seen.extend(page.iter().map(|summary| member(summary, "arn")));
            match response["nextToken"].as_str() {
                None => break,
                Some(token) => {
                    assert!(
                        !page.is_empty(),
                        "case {case:?}: a token says compositions are left over, so the \
                         page it ends is not empty, got {response}"
                    );
                    request = json!({ "maxResults": page_size, "nextToken": token });
                }
            }
        }
        seen
    }

    /// `StopComposition` on `arn`, asserting the modelled success.
    async fn stop(&self, case: &str, arn: &str) {
        let (status, response) = self.call("StopComposition", &json!({ "arn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StopComposition {arn} must answer HTTP 200, \
             got {status} {response}"
        );
    }
}

/// The error code a refusal states, or `"<absent>"` so a failing assertion names the
/// missing member rather than panicking inside the reader.
fn error_code(response: &Value) -> &str {
    response
        .get("Code")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
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

/// The summaries one page reported.
fn summaries(response: &Value) -> Vec<Value> {
    response["compositions"].as_array().cloned().unwrap_or_default()
}

/// The summary of the composition `arn` in `page`, if the page reports it.
fn summary_of(page: &[Value], arn: &str) -> Option<Value> {
    page.iter()
        .find(|summary| member(summary, "arn") == arn)
        .cloned()
}

/// An IVS ARN of `resource_type` in [`REGION`], naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// The resources the compositions of a fixture refer to, as this store minted them.
struct Fixture {
    stage_a: String,
    stage_b: String,
    encoder_one: String,
    encoder_two: String,
    storage: String,
}

impl Fixture {
    async fn resolve(service: &RealTime) -> Self {
        Self {
            stage_a: service.stage("stage-a").await,
            stage_b: service.stage("stage-b").await,
            encoder_one: service.encoder_configuration("enc-one").await,
            encoder_two: service.encoder_configuration("enc-two").await,
            storage: service.storage_configuration("list-compositions").await,
        }
    }

    /// A `StartComposition` request for `stage_arn` whose single channel destination
    /// broadcasts through the encoder configuration `encoder_arn`.
    fn channel_start(&self, stage_arn: &str, encoder_arn: &str) -> Value {
        json!({
            "stageArn": stage_arn,
            "destinations": [{
                "channel": {
                    "channelArn": arn("channel", "listed"),
                    "encoderConfigurationArn": encoder_arn,
                },
            }],
        })
    }

    /// A `StartComposition` request for `stage_arn` whose single S3 destination records
    /// through the encoder configuration `encoder_arn`.
    ///
    /// `S3DestinationConfiguration$encoderConfigurationArns` is `@length(1, 1)`, so one
    /// ARN is the whole list.
    fn s3_start(&self, stage_arn: &str, encoder_arn: &str) -> Value {
        json!({
            "stageArn": stage_arn,
            "destinations": [{
                "s3": {
                    "storageConfigurationArn": self.storage,
                    "encoderConfigurationArns": [encoder_arn],
                },
            }],
        })
    }
}

/// Rule 1: every composition the store holds is reported, in start order, and each
/// summary agrees with `GetComposition` on every member both carry.
///
/// Rows differ along what the rule generalises over: the stage composited, the KIND of
/// destination (channel, S3, one of each), the number of destinations, and whether the
/// start carried tags.
#[tokio::test]
async fn a_page_reports_every_started_composition_and_agrees_with_get_composition() {
    let service = RealTime::new();
    let fixture = Fixture::resolve(&service).await;

    let rows: Vec<(&str, Value)> = vec![
        (
            "one channel destination",
            fixture.channel_start(&fixture.stage_a, &fixture.encoder_one),
        ),
        (
            "one S3 destination",
            fixture.s3_start(&fixture.stage_a, &fixture.encoder_two),
        ),
        (
            "another stage, one channel destination",
            fixture.channel_start(&fixture.stage_b, &fixture.encoder_one),
        ),
        (
            "two destinations, one of each kind, and tags",
            json!({
                "stageArn": fixture.stage_b,
                "tags": { "owner": "list-compositions" },
                "destinations": [
                    {
                        "channel": {
                            "channelArn": arn("channel", "both"),
                            "encoderConfigurationArn": fixture.encoder_one,
                        },
                    },
                    {
                        "s3": {
                            "storageConfigurationArn": fixture.storage,
                            "encoderConfigurationArns": [fixture.encoder_two],
                        },
                    },
                ],
            }),
        ),
    ];

    // An empty store is an empty page, not a refusal, and not a fabricated record.
    let page = service.list("an empty store", &json!({})).await;
    assert!(
        summaries(&page).is_empty(),
        "case \"an empty store\": expected no composition, got {page}"
    );

    let mut started: Vec<(&str, String)> = Vec::new();
    for (case, request) in &rows {
        let composition = service.start(case, request).await;
        started.push((case, member(&composition, "arn")));
    }

    let listed = service
        .listed_arns("every started composition", &json!({}))
        .await;
    let expected: Vec<String> = started.iter().map(|(_, arn)| arn.clone()).collect();
    assert_eq!(
        listed, expected,
        "case \"every started composition\": expected every composition in start order, \
         got {listed:?}"
    );

    // Member for member against the read of the same composition, at one instant.
    let page = summaries(&service.list("summary agrees with read", &json!({})).await);
    for (case, composition_arn) in &started {
        let summary = summary_of(&page, composition_arn).unwrap_or_else(|| {
            panic!("case {case:?}: expected {composition_arn} on the page, got {page:?}")
        });
        let read = service.get(case, composition_arn).await;
        for key in ["arn", "stageArn", "state", "tags", "startTime"] {
            assert_eq!(
                summary.get(key),
                read.get(key),
                "case {case:?}: the summary's {key} must be what GetComposition reports, \
                 got {:?} against {:?}",
                summary.get(key),
                read.get(key)
            );
        }
        let summary_destinations = summary["destinations"].as_array().cloned().unwrap_or_default();
        let read_destinations = read["destinations"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            summary_destinations.len(),
            read_destinations.len(),
            "case {case:?}: the summary reports every destination the read does, \
             got {summary_destinations:?} against {read_destinations:?}"
        );
        for (index, (listed, read)) in summary_destinations
            .iter()
            .zip(read_destinations.iter())
            .enumerate()
        {
            for key in ["id", "state", "startTime"] {
                assert_eq!(
                    listed.get(key),
                    read.get(key),
                    "case {case:?}: destination {index}'s {key} must be what \
                     GetComposition reports, got {:?} against {:?}",
                    listed.get(key),
                    read.get(key)
                );
            }
        }
    }
}

/// Rule 2: each filter selects by the member's own doc, and two filters select what both
/// of them select.
///
/// Rows differ along which filter is carried, which value it names (a stage with several
/// compositions, a stage with one, an encoder configuration attached through a channel
/// destination, one attached through an S3 destination), the two together, and a value no
/// composition of this store matches.
#[tokio::test]
async fn each_filter_selects_the_compositions_its_doc_describes() {
    let service = RealTime::new();
    let fixture = Fixture::resolve(&service).await;

    // stage A: a channel destination through encoder one, and an S3 destination through
    // encoder two. stage B: a channel destination through encoder two.
    let a_channel_one = member(
        &service
            .start(
                "stage A / channel / encoder one",
                &fixture.channel_start(&fixture.stage_a, &fixture.encoder_one),
            )
            .await,
        "arn",
    );
    let a_s3_two = member(
        &service
            .start(
                "stage A / S3 / encoder two",
                &fixture.s3_start(&fixture.stage_a, &fixture.encoder_two),
            )
            .await,
        "arn",
    );
    let b_channel_two = member(
        &service
            .start(
                "stage B / channel / encoder two",
                &fixture.channel_start(&fixture.stage_b, &fixture.encoder_two),
            )
            .await,
        "arn",
    );

    // A stage and an encoder configuration this store holds no composition for: of the
    // modelled form, so the filter is applied rather than refused.
    let unused_stage = service.stage("stage-without-compositions").await;
    let unused_encoder = service.encoder_configuration("enc-unused").await;

    let rows: Vec<(&str, Value, Vec<&String>)> = vec![
        (
            "no filter",
            json!({}),
            vec![&a_channel_one, &a_s3_two, &b_channel_two],
        ),
        (
            "filterByStageArn: the stage with two compositions",
            json!({ "filterByStageArn": fixture.stage_a }),
            vec![&a_channel_one, &a_s3_two],
        ),
        (
            "filterByStageArn: the stage with one",
            json!({ "filterByStageArn": fixture.stage_b }),
            vec![&b_channel_two],
        ),
        (
            "filterByEncoderConfigurationArn: attached through a channel destination",
            json!({ "filterByEncoderConfigurationArn": fixture.encoder_one }),
            vec![&a_channel_one],
        ),
        (
            "filterByEncoderConfigurationArn: attached through a channel and an S3 one",
            json!({ "filterByEncoderConfigurationArn": fixture.encoder_two }),
            vec![&a_s3_two, &b_channel_two],
        ),
        (
            "both filters, satisfied together",
            json!({
                "filterByStageArn": fixture.stage_a,
                "filterByEncoderConfigurationArn": fixture.encoder_two,
            }),
            vec![&a_s3_two],
        ),
        (
            "both filters, satisfied by no one composition",
            json!({
                "filterByStageArn": fixture.stage_b,
                "filterByEncoderConfigurationArn": fixture.encoder_one,
            }),
            vec![],
        ),
        (
            "filterByStageArn: a stage with no composition",
            json!({ "filterByStageArn": unused_stage }),
            vec![],
        ),
        (
            "filterByEncoderConfigurationArn: an encoder configuration no output uses",
            json!({ "filterByEncoderConfigurationArn": unused_encoder }),
            vec![],
        ),
    ];

    for (case, request, expected) in rows {
        let listed = service.listed_arns(case, &request).await;
        let expected: Vec<String> = expected.into_iter().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected exactly the matching compositions in start order, \
             got {listed:?}"
        );
    }
}

/// Rule 3: a page holds at most `maxResults`, its token resumes where it ended, and
/// paging the listing reports each composition exactly once in start order.
///
/// Rows differ along the page size: one per page, several, exactly the number of
/// compositions, more than there are, and each published bound of the member's `@range`.
/// A value past either bound is the modelled refusal.
#[tokio::test]
async fn pages_hold_at_most_max_results_and_its_own_token_resumes() {
    let service = RealTime::new();
    let fixture = Fixture::resolve(&service).await;

    let mut all: Vec<String> = Vec::new();
    for index in 0..5 {
        // Alternating destination kinds, so the paged set is not one shape repeated.
        let request = if index % 2 == 0 {
            fixture.channel_start(&fixture.stage_a, &fixture.encoder_one)
        } else {
            fixture.s3_start(&fixture.stage_a, &fixture.encoder_two)
        };
        let composition = service.start(&format!("paged-{index}"), &request).await;
        all.push(member(&composition, "arn"));
    }

    for page_size in [MAX_RESULTS_MIN, 2, 5, 6, MAX_RESULTS_MAX] {
        let case = format!("maxResults={page_size}");
        let seen = service.every_page(&case, page_size).await;
        assert_eq!(
            seen, all,
            "case {case:?}: paging the listing reports each composition once, in start \
             order, got {seen:?}"
        );
    }

    // The last page carries no token: there is nothing left over.
    let response = service.list("the whole listing at once", &json!({})).await;
    assert!(
        response["nextToken"].is_null(),
        "case \"the whole listing at once\": a page with nothing left over carries no \
         nextToken, got {response}"
    );

    // One past each published bound is the modelled refusal, not a clamped page.
    for max_results in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let case = format!("maxResults={max_results}");
        let (status, response) = service
            .call("ListCompositions", &json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 outside the modelled range, \
             got {status} {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected the modelled ValidationException, got {response}"
        );
    }
}

/// Rule 4: the listing reports the state the clock owes, and stops reporting a
/// composition once the clock has reclaimed the ephemeral resource. Nothing the read
/// itself does changes what a later caller sees.
#[tokio::test]
async fn a_page_reports_the_state_the_clock_owes_and_drops_a_reclaimed_composition() {
    let service = RealTime::new();
    let fixture = Fixture::resolve(&service).await;

    // Rows differ along the destination kind, so the transition is not pinned to one
    // shape of composition.
    let rows: Vec<(&str, Value)> = vec![
        (
            "channel destination",
            fixture.channel_start(&fixture.stage_a, &fixture.encoder_one),
        ),
        (
            "S3 destination",
            fixture.s3_start(&fixture.stage_b, &fixture.encoder_two),
        ),
    ];

    let mut started: Vec<(&str, String)> = Vec::new();
    for (case, request) in &rows {
        let composition = service.start(case, request).await;
        started.push((case, member(&composition, "arn")));
    }

    tokio::time::sleep(WITHIN_THE_ACTIVATION_SPAN).await;
    let early = summaries(&service.list("inside the activation span", &json!({})).await);
    // Listing twice at one instant answers twice the same: the read materialises only
    // what the clock already owed.
    let again = summaries(&service.list("inside the activation span", &json!({})).await);
    assert_eq!(
        early, again,
        "case \"inside the activation span\": two listings at one instant must agree, \
         got {early:?} against {again:?}"
    );
    for (case, composition_arn) in &started {
        let summary = summary_of(&early, composition_arn).unwrap_or_else(|| {
            panic!("case {case:?}: expected {composition_arn} on the page, got {early:?}")
        });
        assert_eq!(
            member(&summary, "state"),
            STATE_STARTING,
            "case {case:?}: a listing inside the activation span must still report \
             {STATE_STARTING}, got {summary}"
        );
        for destination in summary["destinations"].as_array().cloned().unwrap_or_default() {
            assert_eq!(
                member(&destination, "state"),
                STATE_STARTING,
                "case {case:?}: each destination is {STATE_STARTING} with its \
                 composition, got {destination}"
            );
        }
    }

    tokio::time::sleep(past(state::COMPOSITION_ACTIVATION_SPAN)).await;
    let settled = summaries(&service.list("past the activation span", &json!({})).await);
    for (case, composition_arn) in &started {
        let summary = summary_of(&settled, composition_arn).unwrap_or_else(|| {
            panic!("case {case:?}: expected {composition_arn} on the page, got {settled:?}")
        });
        assert_eq!(
            member(&summary, "state"),
            STATE_ACTIVE,
            "case {case:?}: once the clock has made the transition due the listing must \
             report {STATE_ACTIVE}, got {summary}"
        );
        let early_summary = summary_of(&early, composition_arn).unwrap_or_default();
        assert_eq!(
            summary.get("startTime"),
            early_summary.get("startTime"),
            "case {case:?}: startTime does not move with the state, got {:?} against {:?}",
            summary.get("startTime"),
            early_summary.get("startTime")
        );
    }

    // A stop is reported as the stopping state while it runs, and the ephemeral resource
    // is gone from the listing once the clock has reclaimed it.
    let (stopped_case, stopped_arn) = started
        .first()
        .cloned()
        .expect("the fixture started a composition");
    service.stop(stopped_case, &stopped_arn).await;
    let stopping = summaries(&service.list("just after the stop", &json!({})).await);
    let summary = summary_of(&stopping, &stopped_arn).unwrap_or_else(|| {
        panic!("case {stopped_case:?}: a stopping composition is still reported, got {stopping:?}")
    });
    assert_eq!(
        member(&summary, "state"),
        STATE_STOPPING,
        "case {stopped_case:?}: a listing inside the stop span must report \
         {STATE_STOPPING}, got {summary}"
    );

    // Two stop spans after the stop began, the record is reclaimed.
    tokio::time::sleep(past(state::COMPOSITION_STOP_SPAN * 2)).await;
    let reclaimed = summaries(&service.list("past the reclamation", &json!({})).await);
    assert!(
        summary_of(&reclaimed, &stopped_arn).is_none(),
        "case {stopped_case:?}: a reclaimed composition is reported no more, \
         got {reclaimed:?}"
    );
    // The composition that was not stopped is untouched by the other's reclamation.
    for (case, composition_arn) in started.iter().skip(1) {
        assert!(
            summary_of(&reclaimed, composition_arn).is_some(),
            "case {case:?}: expected {composition_arn} still on the page, got {reclaimed:?}"
        );
    }
}
