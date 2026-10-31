// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate shares: one emulator over one shared
//! engine, and a signed awsJson1_0 call into it.
//!
//! Extracted here so a new suite does not copy it: the suites differ in the rules they
//! assert, not in how they reach the service.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::state;
use service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::EmulatedService;

/// The error message of an error body, under either casing the protocols render the
/// member with.
pub fn error_message(body: &Value) -> String {
    for key in ["Message", "message"] {
        if let Some(text) = body[key].as_str() {
            return text.to_string();
        }
    }
    String::new()
}

/// The `X-Amz-Target` prefix every request to this awsJson1_0 service carries (see the
/// crate's `OPERATIONS.md`).
const TARGET_PREFIX: &str = "AWSKendraRerankingFrontendService.";

/// One emulator holding this service, over the single shared engine the router builds.
pub fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the modeled behaviour, not signature verification.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call `operation` with `body`, signed for `region`, and return its status and body.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20240101/{region}/kendra-ranking/aws4_request, \
                 SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|_| {
            json!({ "unparsed": String::from_utf8_lossy(response.body()).to_string() })
        })
    };
    (status, parsed)
}

/// The region the region-less helpers here sign for: the emulator's default region, the
/// one a request carrying no credential scope is served in.
pub const DEFAULT_TEST_REGION: &str = "us-east-1";

/// [`call_in`] for the emulator's default region.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    call_in(router, DEFAULT_TEST_REGION, operation, body).await
}

/// The range `RescoreResultItem$Score` models (`Float`, range -100000..100000).
pub const SCORE_MIN: f64 = -100_000.0;
/// See [`SCORE_MIN`].
pub const SCORE_MAX: f64 = 100_000.0;

/// Two scores are the same score: the wire carries a `Float`, so they are compared with
/// a tolerance rather than bit for bit.
pub const SCORE_TOLERANCE: f64 = 1e-6;

/// A `Rescore` call against `plan` for `query` over `documents`. Shared because every
/// suite that drives the data plane reaches it the same way; they differ in the rules
/// they assert.
pub async fn rescore(
    router: &MultiServiceRouter,
    plan: &str,
    query: &str,
    documents: &Value,
) -> (u16, Value) {
    call(
        router,
        "Rescore",
        json!({
            "RescoreExecutionPlanId": plan,
            "SearchQuery": query,
            "Documents": documents.clone(),
        }),
    )
    .await
}

/// The `(DocumentId, Score)` pairs of a `Rescore` answer, in the order the service
/// returned them.
pub fn rescore_items(body: &Value) -> Vec<(String, f64)> {
    body["ResultItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            (
                item["DocumentId"].as_str().unwrap_or_default().to_string(),
                item["Score"].as_f64().unwrap_or(f64::NAN),
            )
        })
        .collect()
}

/// The wire error code of an error body (`__type`), without any namespace prefix.
pub fn error_code(body: &Value) -> String {
    let raw = body["__type"].as_str().unwrap_or_default();
    raw.rsplit('#').next().unwrap_or(raw).to_string()
}

/// The tags `ListTagsForResource` reports for `arn` in `region`, as `(Key, Value)`
/// pairs in the order the service returned them. Shared here because every tagging
/// suite reads a plan's tags back the same way; they differ in the rules they assert.
pub async fn list_tags_in(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
) -> Vec<(String, String)> {
    let (status, answer) =
        call_in(router, region, "ListTagsForResource", json!({ "ResourceARN": arn })).await;
    assert_eq!(
        status, 200,
        "list tags for {arn} in {region}: unexpected answer {answer}"
    );
    answer["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("list tags for {arn} in {region}: no Tags list in {answer}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// [`list_tags_in`] for the emulator's default region.
pub async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Vec<(String, String)> {
    list_tags_in(router, DEFAULT_TEST_REGION, arn).await
}

/// Assert `answer` is the refusal a tag the service does not admit produces: HTTP 400,
/// the `ValidationException` code, a report naming `member` — the dotted position of the
/// offending value — and no minted resource. `offender` is the value the request
/// carried, when the case has one: a report never echoes it.
///
/// Shared because every suite that drives a refused tag write asserts exactly this;
/// they differ in the inputs they drive it with.
pub fn assert_tag_rejection(
    case: &str,
    status: u16,
    answer: &Value,
    member: &str,
    offender: Option<&str>,
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {answer}"
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {answer}"
    );
    let message = error_message(answer);
    assert!(
        message.contains(member),
        "case {case:?}: the report must name {member}, got {message:?}"
    );
    if let Some(offender) = offender {
        assert!(
            !message.contains(offender),
            "case {case:?}: the report must not echo the offending value {offender:?}, \
             got {message:?}"
        );
    }
    // A refusal answers no resource: the members a create mints are absent from it.
    for minted in ["Id", "Arn"] {
        assert!(
            answer.get(minted).is_none(),
            "case {case:?}: a refused request must mint no {minted}, got {answer}"
        );
    }
}

/// The `RescoreExecutionPlanSummary` items `ListRescoreExecutionPlans` reports, in the
/// order the service returned them. Shared for the same reason [`list_tags`] is: a
/// suite that needs the plans this emulator holds projects the members it asserts on
/// out of these, rather than repeating the call and the `SummaryItems` walk.
pub async fn list_plan_summaries(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, answer) = call(router, "ListRescoreExecutionPlans", json!({})).await;
    assert_eq!(status, 200, "list plans: unexpected answer {answer}");
    answer["SummaryItems"]
        .as_array()
        .unwrap_or_else(|| panic!("list plans: no SummaryItems list in {answer}"))
        .clone()
}

/// Create a plan named `name` in `region`, carrying `tags` (`Value::Null` for a plan
/// created with no `Tags` member at all), and return its `(Id, Arn)`. The plan is
/// `CREATING` when this returns: creation is asynchronous, and the clock owes it
/// `ACTIVE`.
pub async fn create_plan_in_with_tags(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: Value,
) -> (String, String) {
    let mut request = json!({ "Name": name });
    if !tags.is_null() {
        request["Tags"] = tags;
    }
    let (status, body) = call_in(router, region, "CreateRescoreExecutionPlan", request).await;
    assert_eq!(
        status, 200,
        "create {name} in {region}: unexpected answer {body}"
    );
    (
        body["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("create {name} in {region}: no Id in {body}"))
            .to_string(),
        body["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("create {name} in {region}: no Arn in {body}"))
            .to_string(),
    )
}

/// [`create_plan_in_with_tags`] for the emulator's default region.
pub async fn create_plan_with_tags(
    router: &MultiServiceRouter,
    name: &str,
    tags: Value,
) -> (String, String) {
    create_plan_in_with_tags(router, DEFAULT_TEST_REGION, name, tags).await
}

/// Create a plan named `name` and return its id. The plan is `CREATING` when this
/// returns: creation is asynchronous, and the clock owes it `ACTIVE`. Use
/// [`create_active_plan`] where the test needs a plan ready to serve.
pub async fn create_plan(router: &MultiServiceRouter, name: &str) -> String {
    create_plan_with_tags(router, name, Value::Null).await.0
}

/// Wait until the clock owes every plan created before this call its settled status
/// (`CREATING` -> `ACTIVE`, `UPDATING` -> `ACTIVE`), with room to spare.
///
/// A BLOCKING sleep rather than `tokio::time::sleep`: the workspace pins tokio without
/// the `time` feature, and nothing else is pending on this runtime.
pub fn sleep_past_the_transition() {
    let span = u64::try_from(state::TRANSIENT_STATUS_MILLIS).unwrap_or(500);
    std::thread::sleep(Duration::from_millis(span) + Duration::from_millis(250));
}

/// [`create_plan`], then wait out the transient `CREATING` status so the plan is
/// `ACTIVE` and can serve `Rescore`.
pub async fn create_active_plan(router: &MultiServiceRouter, name: &str) -> String {
    let id = create_plan(router, name).await;
    sleep_past_the_transition();
    id
}
