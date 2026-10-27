// SPDX-License-Identifier: CC0-1.0

//! `ListEncoderConfigurations` over the wire: the page of encoder configurations this
//! account holds, in the region the request was processed in.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body —
//! never an internal call.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. LISTING REPORTS PERSISTED STATE. The listing answers a page of the encoder
//!    configurations this account holds, in creation order, bounded by `maxResults`
//!    (default: 100), with a `nextToken` exactly when configurations are left over — and
//!    that token resumes where the page ended. A configuration created later joins the
//!    listing at the end; a deleted one leaves it.
//! 2. A SUMMARY AND A READ AGREE. Every member `EncoderConfigurationSummary` carries is
//!    the member of the same name `GetEncoderConfiguration` reports for that same
//!    configuration; the member the summary shape does not carry (`video`) is absent from
//!    a page.
//! 3. REGION. A page holds exactly the configurations of the region its own request was
//!    processed in — "in your account, in the AWS region where the API request is
//!    processed".
//! 4. A READ CHANGES NOTHING: the listing is `@readonly`, so repeated pages over an
//!    unchanged store are identical and a read in between answers the same.
//! 5. REFUSALS: a `maxResults` outside the modelled `@range` is the modelled
//!    `ValidationException` (HTTP 400), and a pagination token this listing did not issue
//!    is refused in this service's one wording rather than read as "start again".

use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// The region this suite signs for unless a row varies it.
const REGION: &str = "us-east-1";

/// `PaginationToken`'s `@pattern`, from
/// `smithy_query(query="operation",operation="ListEncoderConfigurations")` — member
/// `nextToken`.
const PAGINATION_TOKEN_PATTERN: &str = "^[a-zA-Z0-9+/=_-]*$";

/// The published bounds of `MaxEncoderConfigurationResults`' `@range`, from
/// `smithy_query(query="shape",shape="MaxEncoderConfigurationResults")`: `1..100`.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// The page size the listing uses when the request omits `maxResults`, from
/// `ListEncoderConfigurationsRequest$maxResults`: "Maximum number of results to return.
/// Default: 100."
const DEFAULT_PAGE_SIZE: usize = 100;

/// The members `EncoderConfigurationSummary` carries, from
/// `smithy_query(query="shape",shape="EncoderConfigurationSummary")`.
const SUMMARY_MEMBERS: [&str; 3] = ["arn", "name", "tags"];

/// The member the full `EncoderConfiguration` carries that the summary shape does NOT
/// (`smithy_query(query="shape",shape="EncoderConfiguration,EncoderConfigurationSummary")`),
/// so a page reports fewer members than a read rather than different ones.
const MEMBERS_NOT_IN_A_SUMMARY: [&str; 1] = ["video"];

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

    /// [`Self::call_in`] against the region every test that does not vary it uses.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Create an encoder configuration in `region` and return the `encoderConfiguration`
    /// the service reported for it.
    async fn create_in(&self, case: &str, region: &str, body: &Value) -> Value {
        let (status, response) = self
            .call_in(region, "CreateEncoderConfiguration", body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateEncoderConfiguration {body} must answer HTTP 200, \
             got {status} {response}"
        );
        response["encoderConfiguration"].clone()
    }

    /// [`Self::create_in`] in this suite's own region.
    async fn create(&self, case: &str, body: &Value) -> Value {
        self.create_in(case, REGION, body).await
    }

    /// Read one configuration back, asserting the read succeeded.
    async fn get(&self, case: &str, arn: &str) -> Value {
        let (status, response) = self
            .call("GetEncoderConfiguration", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetEncoderConfiguration on {arn} must answer HTTP 200, \
             got {status} {response}"
        );
        response["encoderConfiguration"].clone()
    }

    /// One `ListEncoderConfigurations` page from `region`, asserting the call succeeded.
    async fn list_in(&self, case: &str, region: &str, request: &Value) -> Value {
        let (status, response) = self
            .call_in(region, "ListEncoderConfigurations", request)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListEncoderConfigurations {request} must answer HTTP 200, \
             got {status} {response}"
        );
        response
    }

    /// [`Self::list_in`] in this suite's own region.
    async fn list(&self, case: &str, request: &Value) -> Value {
        self.list_in(case, REGION, request).await
    }

    /// The ARNs one page reports, in the order it reported them.
    async fn listed_arns(&self, case: &str, request: &Value) -> Vec<String> {
        let response = self.list(case, request).await;
        summaries(&response)
            .iter()
            .map(|summary| member(summary, "arn"))
            .collect()
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

/// The text a refusal states.
fn error_message(response: &Value) -> &str {
    response
        .get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<absent>")
}

/// The sentence every listing of this service refuses a token it did not issue with.
fn foreign_page_token_message(operation: &str) -> String {
    format!("{operation}.nextToken is required to be a pagination token issued by {operation}")
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

/// The summaries one page reported, as an empty list when the member is absent — so a
/// failing assertion names what came back rather than panicking inside the reader.
fn summaries(response: &Value) -> Vec<Value> {
    response["encoderConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `value` restricted to the members [`SUMMARY_MEMBERS`] names, so a summary and a full
/// read can be compared member for member on exactly the members both carry.
fn summary_members(value: &Value) -> Value {
    let mut projected = Map::new();
    for name in SUMMARY_MEMBERS {
        if let Some(found) = value.get(name) {
            projected.insert(name.to_string(), found.clone());
        }
    }
    Value::Object(projected)
}

/// Does `value` satisfy the one model `@pattern` this suite checks?
///
/// The pattern is quoted on [`PAGINATION_TOKEN_PATTERN`], straight from `smithy_query`,
/// and is simple enough to check directly — which keeps this crate's dependency set
/// exactly what the service itself needs. A pattern with no arm panics rather than
/// passing silently.
fn matches(pattern: &str, value: &str) -> bool {
    match pattern {
        PAGINATION_TOKEN_PATTERN => value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=' | b'_' | b'-')
        }),
        other => panic!("this suite checks no pattern {other:?}"),
    }
}

/// The create requests this suite spans the class of configurations with: each row
/// differs from the others in what a summary reports or in what the full record holds —
/// whether the request names the configuration, whether it carries tags, and which
/// `Video` members it sets.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("no members at all", json!({})),
        ("named only", json!({ "name": "studio-1080p" })),
        (
            "named, with tags",
            json!({ "name": "lobby", "tags": { "env": "test", "team": "video" } }),
        ),
        (
            "a full video configuration and no name",
            json!({
                "video": { "width": 1920, "height": 1080, "framerate": 60, "bitrate": 6_000_000 },
            }),
        ),
        (
            "a partial video configuration, the rest defaulted",
            json!({ "name": "portrait", "video": { "width": 720, "height": 1280 } }),
        ),
        (
            "the empty name, which the member's length allows",
            json!({ "name": "" }),
        ),
        (
            "an empty tag map",
            json!({ "name": "no-tags", "tags": {} }),
        ),
    ]
}

/// The suite's fixture: the configurations [`create_rows`] describes, in creation order.
struct Fixture {
    emulator: RealTime,
    /// The `encoderConfiguration` each create reported, in creation order.
    created: Vec<Value>,
}

impl Fixture {
    async fn new() -> Self {
        let emulator = RealTime::new();
        let mut created = Vec::new();
        for (case, request) in create_rows() {
            created.push(emulator.create(case, &request).await);
        }
        Self { emulator, created }
    }

    /// The ARNs of the created configurations, in creation order.
    fn arns(&self) -> Vec<String> {
        self.created
            .iter()
            .map(|value| member(value, "arn"))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Rule 1: the listing reports persisted state, a page at a time
// ---------------------------------------------------------------------------

/// A listing over an empty store reports the required member as an empty list and no
/// `nextToken` — it never answers "not implemented" and never invents a configuration.
#[tokio::test]
async fn listing_an_empty_account_reports_no_encoder_configurations() {
    let emulator = RealTime::new();

    // Rows differ in the page bound, which changes nothing when there is nothing stored.
    let requests = [
        json!({}),
        json!({ "maxResults": MAX_RESULTS_RANGE.0 }),
        json!({ "maxResults": 7 }),
        json!({ "maxResults": MAX_RESULTS_RANGE.1 }),
        json!({ "nextToken": "" }),
    ];
    for request in requests {
        let case = format!("request {request}");
        let response = emulator.list(&case, &request).await;
        assert_eq!(
            response["encoderConfigurations"],
            json!([]),
            "case {case:?}: expected an empty encoderConfigurations list, got {response}"
        );
        assert_eq!(
            response.get("nextToken"),
            None,
            "case {case:?}: expected no nextToken on the only page, got {response}"
        );
    }
}

/// Whatever the page bound, paging through the listing yields every stored configuration
/// exactly once, in creation order, and only the last page omits `nextToken`.
#[tokio::test]
async fn the_listing_pages_over_every_configuration_in_creation_order() {
    let fixture = Fixture::new().await;
    let expected = fixture.arns();
    let stored = expected.len();

    // Rows differ in the page bound, which is what the rule generalises over: every bound
    // below, at and above the stored count, both published `@range` bounds, and the absent
    // member (the documented default).
    for max_results in [
        Some(MAX_RESULTS_RANGE.0),
        Some(2),
        Some(3),
        Some(stored as i64 - 1),
        Some(stored as i64),
        Some(stored as i64 + 1),
        Some(MAX_RESULTS_RANGE.1),
        None,
    ] {
        let case = format!("maxResults={max_results:?}");
        let bound = max_results.map_or(DEFAULT_PAGE_SIZE, |max| max as usize);
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({});
            if let Some(max) = max_results {
                request["maxResults"] = json!(max);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let response = fixture.emulator.list(&case, &request).await;
            let page = summaries(&response);
            assert!(
                page.len() <= bound,
                "case {case:?}: expected at most {bound} configurations per page, got {}",
                page.len()
            );
            for summary in &page {
                seen.push(member(summary, "arn"));
            }

            pages += 1;
            assert!(pages <= 20, "case {case:?}: pagination did not terminate");
            match response["nextToken"].as_str() {
                Some(token) => {
                    assert!(
                        matches(PAGINATION_TOKEN_PATTERN, token),
                        "case {case:?}: expected a nextToken matching \
                         {PAGINATION_TOKEN_PATTERN}, got {token:?}"
                    );
                    assert_eq!(
                        page.len(),
                        bound,
                        "case {case:?}: expected a full page before a nextToken, got {response}"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }

        assert_eq!(
            seen, expected,
            "case {case:?}: expected every created configuration once, in creation order, \
             got {seen:?}"
        );
    }
}

/// The documented default bound is the one the listing applies: with more configurations
/// stored than the default, a request that omits `maxResults` reports exactly that many
/// and leaves the rest for the next page.
#[tokio::test]
async fn omitting_max_results_pages_by_the_documented_default() {
    let emulator = RealTime::new();

    // One more than the default, so "every configuration" and "the default page" are
    // different answers.
    let stored = DEFAULT_PAGE_SIZE + 1;
    let mut expected: Vec<String> = Vec::new();
    for index in 0..stored {
        let created = emulator
            .create(
                "the default-bound fixture",
                &json!({ "name": format!("cfg-{index}") }),
            )
            .await;
        expected.push(member(&created, "arn"));
    }

    let response = emulator.list("the default bound", &json!({})).await;
    let page = summaries(&response);
    assert_eq!(
        page.len(),
        DEFAULT_PAGE_SIZE,
        "expected the documented default of {DEFAULT_PAGE_SIZE} per page, got {}",
        page.len()
    );
    let token = response["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !token.is_empty(),
        "expected a nextToken while a configuration is left over, got {response}"
    );
    let rest = emulator
        .listed_arns("the default bound", &json!({ "nextToken": token }))
        .await;
    assert_eq!(
        rest,
        expected[DEFAULT_PAGE_SIZE..].to_vec(),
        "expected the token to resume at the configuration the default page left, got {rest:?}"
    );
}

/// A configuration that goes away leaves the listing, and one created later joins it at
/// the end — the page is read off the store rather than remembered.
#[tokio::test]
async fn the_listing_follows_creates_and_deletes() {
    let fixture = Fixture::new().await;
    let mut expected = fixture.arns();

    // Rows differ in WHERE the configuration that goes away sat: the first, the middle and
    // the last of what the listing reported at that moment.
    for position in ["the first", "the middle", "the last"] {
        let index = match position {
            "the first" => 0,
            "the middle" => expected.len() / 2,
            _ => expected.len() - 1,
        };
        let doomed = expected[index].clone();
        let case = format!("deleting {position} ({doomed})");
        let (status, response) = fixture
            .emulator
            .call("DeleteEncoderConfiguration", &json!({ "arn": doomed }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteEncoderConfiguration must answer HTTP 200, \
             got {status} {response}"
        );
        expected.remove(index);
        let listed = fixture
            .emulator
            .listed_arns(&case, &json!({ "maxResults": 2 }))
            .await;
        assert_eq!(
            listed,
            expected.iter().take(2).cloned().collect::<Vec<String>>(),
            "case {case:?}: expected the first page of what is left, got {listed:?}"
        );
        let listed = fixture.emulator.listed_arns(&case, &json!({})).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the deleted configuration to leave the listing, \
             got {listed:?}"
        );
    }

    // And a configuration created now joins the listing at the end, since the order is
    // creation order.
    let created = fixture
        .emulator
        .create("a late configuration", &json!({ "name": "late" }))
        .await;
    expected.push(member(&created, "arn"));
    let listed = fixture
        .emulator
        .listed_arns("a late configuration", &json!({}))
        .await;
    assert_eq!(
        listed, expected,
        "case \"a late configuration\": expected it listed last, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: a summary and a read agree
// ---------------------------------------------------------------------------

/// Every member a summary carries is the member of the same name
/// `GetEncoderConfiguration` reports for that same configuration, and the member the
/// summary shape does not carry is absent from a page.
#[tokio::test]
async fn every_summary_reports_what_a_read_reports() {
    let fixture = Fixture::new().await;

    let listed = fixture.emulator.list("the whole listing", &json!({})).await;
    let page = summaries(&listed);
    assert_eq!(
        page.len(),
        fixture.created.len(),
        "expected every created configuration on one page, got {listed}"
    );

    for (position, summary) in page.iter().enumerate() {
        let arn = member(summary, "arn");
        let case = format!("summary {position} ({arn})");
        let read = fixture.emulator.get(&case, &arn).await;
        assert_eq!(
            summary_members(summary),
            summary_members(&read),
            "case {case:?}: expected the summary to report what the read reports, \
             got {summary} against {read}"
        );
        // The create's own answer is the same configuration too, so a page, a read and a
        // create cannot report three different things.
        assert_eq!(
            summary_members(summary),
            summary_members(&fixture.created[position]),
            "case {case:?}: expected the summary to report what the create reported, \
             got {summary}"
        );
        for absent in MEMBERS_NOT_IN_A_SUMMARY {
            assert!(
                summary.get(absent).is_none(),
                "case {case:?}: {absent} is not a member of EncoderConfigurationSummary, \
                 got {summary}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the page is the request's own region's view
// ---------------------------------------------------------------------------

/// "In your account, in the AWS region where the API request is processed": a page holds
/// exactly the configurations created through the listing's own region's endpoint.
#[tokio::test]
async fn a_page_reports_only_the_configurations_of_its_own_region() {
    let emulator = RealTime::new();

    // Rows differ in the region and in how many configurations it holds, so neither "the
    // first region" nor "one each" can pass by accident.
    let rows: [(&str, usize); 3] = [("us-east-1", 2), ("eu-west-1", 1), ("ap-southeast-2", 3)];

    let mut expected: Vec<(&str, Vec<String>)> = Vec::new();
    for (region, count) in rows {
        let mut arns = Vec::new();
        for index in 0..count {
            let created = emulator
                .create_in(region, region, &json!({ "name": format!("cfg-{index}") }))
                .await;
            arns.push(member(&created, "arn"));
        }
        expected.push((region, arns));
    }

    for (region, arns) in &expected {
        let listed = emulator.list_in(region, region, &json!({})).await;
        let reported: Vec<String> = summaries(&listed)
            .iter()
            .map(|summary| member(summary, "arn"))
            .collect();
        assert_eq!(
            &reported, arns,
            "case {region:?}: expected only this region's configurations, got {reported:?}"
        );
        for arn in &reported {
            assert!(
                arn.contains(&format!(":{region}:")),
                "case {region:?}: every listed ARN must name this region, got {arn:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the listing is a read
// ---------------------------------------------------------------------------

/// Listing is a read: it changes nothing a later caller sees. Repeated pages over an
/// unchanged store are identical, whatever the bound, and a read of a configuration in
/// between answers the same too.
#[tokio::test]
async fn listing_changes_nothing() {
    let fixture = Fixture::new().await;
    let probe_arn = member(&fixture.created[1], "arn");

    // Rows differ in the request repeated: the whole listing, a one-at-a-time page and a
    // page bound at the published maximum.
    let requests = [
        json!({}),
        json!({ "maxResults": 1 }),
        json!({ "maxResults": MAX_RESULTS_RANGE.1 }),
    ];
    for request in requests {
        let case = format!("request {request}");
        let first = fixture.emulator.list(&case, &request).await;
        let before = fixture.emulator.get(&case, &probe_arn).await;
        let second = fixture.emulator.list(&case, &request).await;
        let after = fixture.emulator.get(&case, &probe_arn).await;
        assert_eq!(
            first, second,
            "case {case:?}: expected the second page to be identical, got {second}"
        );
        assert_eq!(
            before, after,
            "case {case:?}: expected listing to leave the configuration where it was, \
             got {after}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: refusals
// ---------------------------------------------------------------------------

/// `maxResults` is held to the modelled `@range(1..100)`: both published bounds are
/// accepted and a value just past either is refused with the modelled
/// `ValidationException`.
#[tokio::test]
async fn the_listing_holds_the_modeled_max_results_range() {
    let fixture = Fixture::new().await;

    let (minimum, maximum) = MAX_RESULTS_RANGE;
    let rows: Vec<(&str, i64, u16)> = vec![
        ("the published minimum", minimum, 200),
        ("the published maximum", maximum, 200),
        ("one below the minimum", minimum - 1, 400),
        ("one above the maximum", maximum + 1, 400),
        ("far below the minimum", -1, 400),
        ("far above the maximum", 1_000, 400),
    ];
    for (case, max_results, expected_status) in rows {
        let (status, response) = fixture
            .emulator
            .call(
                "ListEncoderConfigurations",
                &json!({ "maxResults": max_results }),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?} (maxResults={max_results}): expected HTTP {expected_status}, \
             got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
            assert!(
                response.get("encoderConfigurations").is_none(),
                "case {case:?}: a refused listing reports no page, got {response}"
            );
        }
    }
}

/// A pagination token this listing did not issue names no position, so it is refused
/// rather than read as "start again" — a token another listing of this same service
/// issued included — while the empty token is the first page and its own token resumes.
#[tokio::test]
async fn the_listing_refuses_a_pagination_token_it_did_not_issue() {
    let fixture = Fixture::new().await;
    let stored = fixture.created.len();

    // A token from the stage listing, which pages over a different store: it names no
    // position here. Two stages, so that listing has a page left over to issue one for.
    for index in 0..2 {
        let (status, response) = fixture
            .emulator
            .call("CreateStage", &json!({ "name": format!("stage-{index}") }))
            .await;
        assert_eq!(
            status, 200,
            "fixture CreateStage: expected HTTP 200, got {status} {response}"
        );
    }
    let (status, stage_page) = fixture
        .emulator
        .call("ListStages", &json!({ "maxResults": 1 }))
        .await;
    assert_eq!(
        status, 200,
        "fixture ListStages must answer HTTP 200, got {status} {stage_page}"
    );
    let foreign_token = stage_page["nextToken"]
        .as_str()
        .expect("a nextToken while stages remain")
        .to_string();

    // Rows differ in how the token is wrong: not base64, base64 of foreign text, base64
    // of another listing's marker, a token issued by another listing of this service —
    // and the empty token, which means the first page.
    let rows: Vec<(&str, String, u16)> = vec![
        ("not base64 at all", "++++".to_string(), 400),
        (
            "base64 of foreign text",
            "c29tZXRoaW5nLWVsc2U=".to_string(),
            400,
        ),
        (
            "base64 of a marker with a non-numeric position",
            "aXZzLXJlYWx0aW1lLXN0b3JhZ2UtY29uZmlndXJhdGlvbi1hZnRlcjpub3Bl".to_string(),
            400,
        ),
        ("a token issued by ListStages", foreign_token, 400),
        (
            "the empty token, which means the first page",
            String::new(),
            200,
        ),
    ];
    for (case, token, expected_status) in rows {
        let (status, response) = fixture
            .emulator
            .call("ListEncoderConfigurations", &json!({ "nextToken": token }))
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {response}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected the modelled ValidationException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                foreign_page_token_message("ListEncoderConfigurations"),
                "case {case:?}: expected this service's one foreign-token sentence, got {}",
                error_message(&response)
            );
        } else {
            assert_eq!(
                summaries(&response).len(),
                stored,
                "case {case:?}: expected the first page of every configuration, \
                 got {response}"
            );
        }
    }

    // Its OWN token resumes where the page ended: the configurations it reports are the
    // ones the first page did not.
    let first = fixture
        .emulator
        .list("its own token", &json!({ "maxResults": 1 }))
        .await;
    let resumed = fixture
        .emulator
        .listed_arns(
            "its own token",
            &json!({ "nextToken": first["nextToken"].as_str().unwrap_or_default() }),
        )
        .await;
    assert_eq!(
        resumed,
        fixture.arns()[1..].to_vec(),
        "case \"its own token\": expected the configurations the first page left, \
         got {resumed:?}"
    );
}
