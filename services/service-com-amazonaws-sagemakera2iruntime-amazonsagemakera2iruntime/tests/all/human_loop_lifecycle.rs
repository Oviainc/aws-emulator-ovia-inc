// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the human-loop lifecycle: `StartHumanLoop`, `DescribeHumanLoop`,
//! `ListHumanLoops`, `StopHumanLoop` and `DeleteHumanLoop`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator serves with.
//!
//! Every case goes through the wire: the only state these tests can observe is what a
//! handler persisted, so a response that was not computed from the store cannot pass.

use bytes::Bytes;
use service_com_amazonaws_sagemakera2iruntime_amazonsagemakera2iruntime::ops::types::{
    HUMAN_LOOP_STATUS_VALUES, SORT_ORDER_VALUES,
};
use service_com_amazonaws_sagemakera2iruntime_amazonsagemakera2iruntime::state::{
    decode_page_token, encode_page_token, HumanLoopStatus, HUMAN_LOOP_PAGE_LIMIT,
    HUMAN_LOOP_QUOTA, STOP_SETTLE_DELAY,
};
use service_com_amazonaws_sagemakera2iruntime_amazonsagemakera2iruntime::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The region every request below is signed for; the ARNs the service mints must carry it.
const REGION: &str = "us-east-1";

/// One emulator holding this service, reached exactly as the production router reaches it:
/// one shared engine, one `EmulatedService`.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A flow-definition ARN in the shape the model's `@pattern` requires.
fn flow_definition_arn(name: &str) -> String {
    format!(
        "arn:aws:sagemaker:{REGION}:{}:flow-definition/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The human-loop ARN the service must mint for `name`.
fn expected_human_loop_arn(name: &str) -> String {
    format!(
        "arn:aws:sagemaker:{REGION}:{}:human-loop/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// One HTTP reply, kept as text so a failing assertion can print exactly what came back.
struct Reply {
    status: u16,
    body: String,
}

impl Reply {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// The restJson1 error code, from the error envelope's `__type` discriminator.
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn string_at(&self, member: &str) -> String {
        self.json()
            .get(member)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A numeric member. restJson1 carries a body timestamp as epoch seconds, so this is how
    /// `CreationTime` arrives.
    fn number_at(&self, member: &str) -> Option<f64> {
        self.json().get(member).and_then(serde_json::Value::as_f64)
    }
}

impl std::fmt::Debug for Reply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} {}", self.status, self.body)
    }
}

/// Send one signed request to the emulator. The SigV4 credential scope is what routes the
/// request to this service and what names the region its ARNs carry.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &str) -> Reply {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/sagemaker/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    Reply {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

async fn start(router: &MultiServiceRouter, body: &serde_json::Value) -> Reply {
    call(router, "POST", "/human-loops", &body.to_string()).await
}

async fn describe(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "GET", &format!("/human-loops/{name}"), "").await
}

async fn delete(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "DELETE", &format!("/human-loops/{name}"), "").await
}

async fn stop(router: &MultiServiceRouter, name: &str) -> Reply {
    call(
        router,
        "POST",
        "/human-loops/stop",
        &serde_json::json!({ "HumanLoopName": name }).to_string(),
    )
    .await
}

async fn list(router: &MultiServiceRouter, query: &str) -> Reply {
    call(router, "GET", &format!("/human-loops?{query}"), "").await
}

/// The `HumanLoopName`s a `ListHumanLoops` reply carries, in the order it returned them.
fn summary_names(reply: &Reply) -> Vec<String> {
    reply
        .json()
        .get("HumanLoopSummaries")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("HumanLoopName")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

fn next_token(reply: &Reply) -> Option<String> {
    reply
        .json()
        .get("NextToken")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

/// A request body for `StartHumanLoop`.
fn start_body(
    name: &str,
    flow: &str,
    input_content: &str,
    classifiers: &[&str],
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "HumanLoopName": name,
        "FlowDefinitionArn": flow,
        "HumanLoopInput": { "InputContent": input_content },
    });
    if !classifiers.is_empty() {
        body["DataAttributes"] = serde_json::json!({ "ContentClassifiers": classifiers });
    }
    body
}

/// Separate two creations in time so `ListHumanLoops` has a creation order to sort on.
/// The service records millisecond resolution, so one millisecond apart is enough.
async fn advance_the_clock() {
    tokio::time::sleep(Duration::from_millis(2)).await;
}

/// Seconds since the Unix epoch, now.
fn now_epoch_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the Unix epoch")
        .as_secs() as i64
}

/// An RFC 3339 `date-time`, the format an `@httpQuery`-bound timestamp takes, for
/// `offset_secs` away from now.
fn timestamp_from_now(offset_secs: i64) -> String {
    aws_smithy_types::DateTime::from_secs(now_epoch_secs() + offset_secs)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a representable timestamp")
}

/// A response's epoch-seconds timestamp as the RFC 3339 `date-time` a query-bound filter
/// takes, so a `CreationTimeAfter` / `CreationTimeBefore` bound can be the stored creation
/// time itself. restJson1 carries a body timestamp as epoch seconds and an `@httpQuery` one
/// as RFC 3339, which is the only reason this conversion exists.
fn as_query_timestamp(epoch_secs: f64) -> String {
    let millis = (epoch_secs * 1000.0).round();
    aws_smithy_types::DateTime::from_secs_f64(millis / 1000.0)
        .fmt(aws_smithy_types::date_time::Format::DateTime)
        .expect("a representable timestamp")
}

/// One row of the start/describe table: what varies between rows is the shape of the name,
/// the size of the input content and the set of content classifiers.
struct StartCase {
    case: &'static str,
    name: String,
    input_content: String,
    classifiers: &'static [&'static str],
}

/// A started human loop is PERSISTED, and `DescribeHumanLoop` reports the stored record.
///
/// The rule under test: every member `DescribeHumanLoop` returns comes from the record
/// `StartHumanLoop` wrote, and the ARN is minted from the region the request was signed for
/// and the loop's own name. The table varies the inputs the rule generalises over.
#[tokio::test]
async fn a_started_human_loop_is_persisted_and_described_from_state() {
    let router = emulator();
    let flow = flow_definition_arn("round-trip-flow");

    let cases = vec![
        StartCase {
            case: "single-character name",
            name: "a".to_string(),
            input_content: "{}".to_string(),
            classifiers: &[],
        },
        StartCase {
            case: "digits only",
            name: "90210".to_string(),
            input_content: r#"{"k":"v"}"#.to_string(),
            classifiers: &[],
        },
        StartCase {
            case: "hyphenated name",
            name: "demo-loop-1".to_string(),
            input_content: r#"{"nested":{"list":[1,2,3]}}"#.to_string(),
            // Both values of the model's `ContentClassifier` enum.
            classifiers: &[
                "FreeOfPersonallyIdentifiableInformation",
                "FreeOfAdultContent",
            ],
        },
        StartCase {
            case: "maximum-length name (63 characters, the model's @length max)",
            name: "a".repeat(63),
            input_content: "{}".to_string(),
            classifiers: &["FreeOfAdultContent"],
        },
        StartCase {
            case: "empty input content (the model's @length min is 0)",
            name: "empty-input".to_string(),
            input_content: String::new(),
            classifiers: &[],
        },
        StartCase {
            case: "large input content",
            name: "large-input".to_string(),
            input_content: format!(r#"{{"blob":"{}"}}"#, "x".repeat(200_000)),
            classifiers: &[],
        },
    ];

    for case in &cases {
        let body = start_body(&case.name, &flow, &case.input_content, case.classifiers);
        let started = start(&router, &body).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected HTTP 200 from StartHumanLoop, got {started:?}",
            case.case
        );
        assert_eq!(
            started.string_at("HumanLoopArn"),
            expected_human_loop_arn(&case.name),
            "case {:?}: StartHumanLoop returned the wrong HumanLoopArn, got {started:?}",
            case.case
        );

        let described = describe(&router, &case.name).await;
        assert_eq!(
            described.status, 200,
            "case {:?}: expected HTTP 200 from DescribeHumanLoop, got {described:?}",
            case.case
        );
        for (member, expected) in [
            ("HumanLoopName", case.name.clone()),
            ("HumanLoopArn", expected_human_loop_arn(&case.name)),
            ("FlowDefinitionArn", flow.clone()),
            (
                "HumanLoopStatus",
                HumanLoopStatus::InProgress.as_wire().to_string(),
            ),
        ] {
            assert_eq!(
                described.string_at(member),
                expected,
                "case {:?}: DescribeHumanLoop reported the wrong {member}, got {described:?}",
                case.case
            );
        }
        // CreationTime is a real clock reading, not a canned value: restJson1 carries it as
        // epoch seconds, and it must sit within a minute of this test's own clock.
        let creation_time = described.number_at("CreationTime");
        let now = now_epoch_secs() as f64;
        assert!(
            creation_time.is_some_and(|created| (created - now).abs() < 60.0),
            "case {:?}: CreationTime must be a timestamp near {now}, got {creation_time:?}",
            case.case
        );
        // A loop nothing has reviewed carries no output object and no failure.
        for absent in ["HumanLoopOutput", "FailureReason", "FailureCode"] {
            assert!(
                described.json().get(absent).is_none(),
                "case {:?}: {absent} must be absent on an InProgress loop, got {described:?}",
                case.case
            );
        }
    }

    // Every row is in the store at once, under its own name.
    let listed = list(&router, &format!("FlowDefinitionArn={flow}")).await;
    let mut names = summary_names(&listed);
    names.sort();
    let mut expected: Vec<String> = cases.iter().map(|case| case.name.clone()).collect();
    expected.sort();
    assert_eq!(
        names, expected,
        "ListHumanLoops must enumerate every persisted loop, got {listed:?}"
    );
}

/// Re-starting a name is idempotent for identical input data and a conflict otherwise.
///
/// The rule under test comes from the model's own `ConflictException`: "the same name as
/// another active human loop but ... different input data". So any difference in the input
/// data is refused with HTTP 409, and no difference at all returns the stored ARN without
/// creating a second loop. The table varies WHICH part of the input data differs.
#[tokio::test]
async fn restarting_a_name_conflicts_only_when_the_input_data_differs() {
    let router = emulator();
    let flow = flow_definition_arn("conflict-flow");
    let other_flow = flow_definition_arn("other-flow");
    let name = "conflict-loop";
    let content = r#"{"original":true}"#;
    let classifiers: &[&str] = &["FreeOfAdultContent"];

    let first = start(&router, &start_body(name, &flow, content, classifiers)).await;
    assert_eq!(
        first.status, 200,
        "the first StartHumanLoop must succeed, got {first:?}"
    );
    let arn = first.string_at("HumanLoopArn");

    let cases: Vec<(&str, serde_json::Value, u16)> = vec![
        (
            "identical input data",
            start_body(name, &flow, content, classifiers),
            200,
        ),
        (
            "different input content",
            start_body(name, &flow, r#"{"original":false}"#, classifiers),
            409,
        ),
        (
            "different flow definition",
            start_body(name, &other_flow, content, classifiers),
            409,
        ),
        (
            "no data attributes",
            start_body(name, &flow, content, &[]),
            409,
        ),
        (
            "a different content classifier",
            start_body(
                name,
                &flow,
                content,
                &["FreeOfPersonallyIdentifiableInformation"],
            ),
            409,
        ),
        (
            "an extra content classifier",
            start_body(
                name,
                &flow,
                content,
                &["FreeOfAdultContent", "FreeOfPersonallyIdentifiableInformation"],
            ),
            409,
        ),
    ];

    for (case, body, expected_status) in &cases {
        let reply = start(&router, body).await;
        assert_eq!(
            reply.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} from the re-start, got {reply:?}"
        );
        if *expected_status == 200 {
            assert_eq!(
                reply.string_at("HumanLoopArn"),
                arn,
                "case {case:?}: an idempotent re-start must return the stored ARN, got {reply:?}"
            );
        } else {
            assert_eq!(
                reply.error_code(),
                "ConflictException",
                "case {case:?}: expected a ConflictException, got {reply:?}"
            );
        }
    }

    // Not one of those calls changed the stored record, and none created a second loop.
    let described = describe(&router, name).await;
    assert_eq!(
        described.string_at("FlowDefinitionArn"),
        flow,
        "a refused re-start must not overwrite the stored loop, got {described:?}"
    );
    let listed = list(&router, &format!("FlowDefinitionArn={flow}")).await;
    assert_eq!(
        summary_names(&listed),
        vec![name.to_string()],
        "a refused re-start must not create a second loop, got {listed:?}"
    );
}

/// `ListHumanLoops` filters on the flow definition and on the creation-time window.
///
/// The rule under test: a summary is returned only when its stored `FlowDefinitionArn` equals
/// the one asked for and its stored `CreationTime` lies within `[CreationTimeAfter,
/// CreationTimeBefore]`. The table spans both bounds, each side open and closed, including
/// bounds taken from a stored creation time itself.
#[tokio::test]
async fn list_filters_by_flow_definition_and_creation_time_window() {
    let router = emulator();
    let flow = flow_definition_arn("window-flow");
    let other_flow = flow_definition_arn("window-other-flow");

    let names = ["win-1", "win-2", "win-3"];
    let mut creation_times = Vec::new();
    for name in names {
        let reply = start(&router, &start_body(name, &flow, "{}", &[])).await;
        assert_eq!(reply.status, 200, "StartHumanLoop failed: {reply:?}");
        let created = describe(&router, name)
            .await
            .number_at("CreationTime")
            .expect("CreationTime is @required");
        creation_times.push(as_query_timestamp(created));
        advance_the_clock().await;
    }
    let decoy = start(&router, &start_body("win-decoy", &other_flow, "{}", &[])).await;
    assert_eq!(decoy.status, 200, "StartHumanLoop failed: {decoy:?}");

    let cases: Vec<(&str, String, Vec<&str>)> = vec![
        (
            "no window",
            format!("FlowDefinitionArn={flow}"),
            vec!["win-3", "win-2", "win-1"],
        ),
        (
            "another flow definition",
            format!("FlowDefinitionArn={other_flow}"),
            vec!["win-decoy"],
        ),
        (
            "a flow definition with no loops",
            format!("FlowDefinitionArn={}", flow_definition_arn("window-empty")),
            vec![],
        ),
        (
            "CreationTimeAfter an hour ago",
            format!(
                "FlowDefinitionArn={flow}&CreationTimeAfter={}",
                timestamp_from_now(-3600)
            ),
            vec!["win-3", "win-2", "win-1"],
        ),
        (
            "CreationTimeAfter an hour from now",
            format!(
                "FlowDefinitionArn={flow}&CreationTimeAfter={}",
                timestamp_from_now(3600)
            ),
            vec![],
        ),
        (
            "CreationTimeBefore an hour from now",
            format!(
                "FlowDefinitionArn={flow}&CreationTimeBefore={}",
                timestamp_from_now(3600)
            ),
            vec!["win-3", "win-2", "win-1"],
        ),
        (
            "CreationTimeBefore an hour ago",
            format!(
                "FlowDefinitionArn={flow}&CreationTimeBefore={}",
                timestamp_from_now(-3600)
            ),
            vec![],
        ),
        (
            "CreationTimeAfter the second loop's own creation time",
            format!(
                "FlowDefinitionArn={flow}&CreationTimeAfter={}",
                creation_times[1]
            ),
            vec!["win-3", "win-2"],
        ),
        (
            "CreationTimeBefore the second loop's own creation time",
            format!(
                "FlowDefinitionArn={flow}&CreationTimeBefore={}",
                creation_times[1]
            ),
            vec!["win-2", "win-1"],
        ),
        (
            "a window closed on both sides",
            format!(
                "FlowDefinitionArn={flow}&CreationTimeAfter={}&CreationTimeBefore={}",
                creation_times[0], creation_times[2]
            ),
            vec!["win-3", "win-2", "win-1"],
        ),
    ];

    for (case, query, expected) in &cases {
        let reply = list(&router, query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected HTTP 200 from ListHumanLoops, got {reply:?}"
        );
        assert_eq!(
            summary_names(&reply),
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "case {case:?}: wrong summaries, got {reply:?}"
        );
    }
}

/// `ListHumanLoops` orders on `CreationTime`, newest first unless told otherwise, and every
/// summary member is read from the stored record.
#[tokio::test]
async fn list_sorts_on_creation_time_and_defaults_to_descending() {
    let router = emulator();
    let flow = flow_definition_arn("sort-flow");

    let created = ["sort-1", "sort-2", "sort-3", "sort-4"];
    for name in created {
        let reply = start(&router, &start_body(name, &flow, "{}", &[])).await;
        assert_eq!(reply.status, 200, "StartHumanLoop failed: {reply:?}");
        advance_the_clock().await;
    }
    let oldest_first: Vec<String> = created.iter().map(ToString::to_string).collect();
    let newest_first: Vec<String> = oldest_first.iter().rev().cloned().collect();

    // The order values come from the model's `SortOrder` enum; `None` is the default.
    let cases: Vec<(&str, Option<&str>, &Vec<String>)> = vec![
        ("no SortOrder (A2I defaults to Descending)", None, &newest_first),
        ("SortOrder=Descending", Some("Descending"), &newest_first),
        ("SortOrder=Ascending", Some("Ascending"), &oldest_first),
    ];
    for (case, order, expected) in &cases {
        let query = match order {
            Some(order) => format!("FlowDefinitionArn={flow}&SortOrder={order}"),
            None => format!("FlowDefinitionArn={flow}"),
        };
        let reply = list(&router, &query).await;
        assert_eq!(
            &summary_names(&reply),
            *expected,
            "case {case:?}: wrong creation-time order, got {reply:?}"
        );
    }
    assert!(
        SORT_ORDER_VALUES.len() == 2
            && SORT_ORDER_VALUES.contains(&"Ascending")
            && SORT_ORDER_VALUES.contains(&"Descending"),
        "the model's SortOrder values changed, so this table no longer spans them: \
         {SORT_ORDER_VALUES:?}"
    );

    // Every summary member is the stored one.
    let reply = list(&router, &format!("FlowDefinitionArn={flow}")).await;
    let summaries = reply.json();
    let summaries = summaries
        .get("HumanLoopSummaries")
        .and_then(serde_json::Value::as_array)
        .expect("HumanLoopSummaries is @required")
        .clone();
    for summary in &summaries {
        let name = summary
            .get("HumanLoopName")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let described = describe(&router, name).await;
        for member in ["HumanLoopName", "HumanLoopStatus", "FlowDefinitionArn"] {
            assert_eq!(
                summary.get(member).and_then(serde_json::Value::as_str),
                Some(described.string_at(member).as_str()),
                "case {name:?}: the summary's {member} must be the stored one, got {summary}"
            );
        }
        assert_eq!(
            summary.get("CreationTime").and_then(serde_json::Value::as_f64),
            described.number_at("CreationTime"),
            "case {name:?}: the summary's CreationTime must be the stored one, got {summary}"
        );
    }
}

/// Paging a list returns every loop exactly once, in the same order, for ANY page size.
///
/// The page sizes below cover the whole space for this population — every size from one to
/// one past the number of loops — plus the model's `MaxResults` bounds (1 and 100).
#[tokio::test]
async fn paging_a_list_walks_every_loop_exactly_once() {
    let router = emulator();
    let flow = flow_definition_arn("page-flow");

    let created = ["page-1", "page-2", "page-3", "page-4", "page-5"];
    for name in created {
        let reply = start(&router, &start_body(name, &flow, "{}", &[])).await;
        assert_eq!(reply.status, 200, "StartHumanLoop failed: {reply:?}");
        advance_the_clock().await;
    }

    let unpaged = list(&router, &format!("FlowDefinitionArn={flow}")).await;
    let whole = summary_names(&unpaged);
    assert_eq!(whole.len(), created.len(), "setup failed: {unpaged:?}");
    assert_eq!(
        next_token(&unpaged),
        None,
        "fewer loops than the default page size, so no NextToken: {unpaged:?}"
    );

    for page_size in [1_usize, 2, 3, 4, 5, 6, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case page_size={page_size}: pagination did not terminate after {pages} pages"
            );
            let query = match &token {
                Some(token) => format!(
                    "FlowDefinitionArn={flow}&MaxResults={page_size}&NextToken={token}"
                ),
                None => format!("FlowDefinitionArn={flow}&MaxResults={page_size}"),
            };
            let reply = list(&router, &query).await;
            assert_eq!(
                reply.status, 200,
                "case page_size={page_size}: expected HTTP 200, got {reply:?}"
            );
            let names = summary_names(&reply);
            assert!(
                names.len() <= page_size,
                "case page_size={page_size}: a page held {} summaries, got {reply:?}",
                names.len()
            );
            walked.extend(names);
            token = next_token(&reply);
            let remaining = whole.len() - walked.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case page_size={page_size}: NextToken must be present exactly while pages \
                 remain ({remaining} left), got {reply:?}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case page_size={page_size}: paging must walk every loop exactly once, in order"
        );
    }
}

/// A pagination token round-trips its position, and a token this service did not issue is
/// refused rather than silently treated as the first page.
#[tokio::test]
async fn a_pagination_token_round_trips_and_a_forged_one_is_refused() {
    let keys = [
        (0_i64, "a".to_string()),
        (1_771_000_000_000, "demo-loop-1".to_string()),
        (-1_000, "before-the-epoch".to_string()),
        (i64::MAX, "a".repeat(63)),
    ];
    for key in &keys {
        let token = encode_page_token(key);
        assert_eq!(
            decode_page_token(&token).as_ref(),
            Some(key),
            "case {key:?}: the token must decode back to its own position, got {token:?}"
        );
    }

    let router = emulator();
    let flow = flow_definition_arn("token-flow");
    let started = start(&router, &start_body("token-loop", &flow, "{}", &[])).await;
    assert_eq!(started.status, 200, "StartHumanLoop failed: {started:?}");

    for forged in ["not-hex-at-all", "6e6f742d612d746f6b656e", "zz"] {
        let reply = list(
            &router,
            &format!("FlowDefinitionArn={flow}&NextToken={forged}"),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {forged:?}: a forged NextToken must be refused, got {reply:?}"
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {forged:?}: expected a ValidationException, got {reply:?}"
        );
    }
}

/// `StopHumanLoop` moves the stored loop to `Stopping`, and the clock settles it on
/// `Stopped`; a loop that already reached a terminal status keeps it.
#[tokio::test]
async fn stopping_a_loop_settles_it_on_stopped() {
    let router = emulator();
    let flow = flow_definition_arn("stop-flow");
    let name = "stop-loop";

    let started = start(&router, &start_body(name, &flow, "{}", &[])).await;
    assert_eq!(started.status, 200, "StartHumanLoop failed: {started:?}");

    let stopped = stop(&router, name).await;
    assert_eq!(
        stopped.status, 200,
        "expected HTTP 200 from StopHumanLoop, got {stopped:?}"
    );

    // The stop is asynchronous, so a caller acting straight after it still sees the
    // transient status — in Describe and in List alike.
    let described = describe(&router, name).await;
    assert_eq!(
        described.string_at("HumanLoopStatus"),
        HumanLoopStatus::Stopping.as_wire(),
        "case immediately-after-stop: got {described:?}"
    );
    let listed = list(&router, &format!("FlowDefinitionArn={flow}")).await;
    assert_eq!(
        listed
            .json()
            .pointer("/HumanLoopSummaries/0/HumanLoopStatus")
            .and_then(serde_json::Value::as_str),
        Some(HumanLoopStatus::Stopping.as_wire()),
        "case immediately-after-stop: the summary must carry the same status, got {listed:?}"
    );

    // Past the due instant the transition has been made: nothing slept for it server-side,
    // the clock made it due.
    tokio::time::sleep(STOP_SETTLE_DELAY + Duration::from_millis(150)).await;
    let described = describe(&router, name).await;
    assert_eq!(
        described.string_at("HumanLoopStatus"),
        HumanLoopStatus::Stopped.as_wire(),
        "case after-the-settle-delay: got {described:?}"
    );

    // Stopping again is a no-op: a settled status is never moved back.
    let again = stop(&router, name).await;
    assert_eq!(
        again.status, 200,
        "expected HTTP 200 from a repeated StopHumanLoop, got {again:?}"
    );
    let described = describe(&router, name).await;
    assert_eq!(
        described.string_at("HumanLoopStatus"),
        HumanLoopStatus::Stopped.as_wire(),
        "case repeated-stop: got {described:?}"
    );
}

/// `DeleteHumanLoop` removes the record, so every later read of that name is a 404 and the
/// name is free again.
#[tokio::test]
async fn deleting_a_loop_removes_it_from_state() {
    let router = emulator();
    let flow = flow_definition_arn("delete-flow");

    for name in ["del-1", "del-2"] {
        let reply = start(&router, &start_body(name, &flow, "{}", &[])).await;
        assert_eq!(reply.status, 200, "StartHumanLoop failed: {reply:?}");
        advance_the_clock().await;
    }

    let deleted = delete(&router, "del-1").await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteHumanLoop, got {deleted:?}"
    );

    let described = describe(&router, "del-1").await;
    assert_eq!(
        described.status, 404,
        "a deleted loop must not be described, got {described:?}"
    );
    assert_eq!(
        described.error_code(),
        "ResourceNotFoundException",
        "expected a ResourceNotFoundException, got {described:?}"
    );

    let listed = list(&router, &format!("FlowDefinitionArn={flow}")).await;
    assert_eq!(
        summary_names(&listed),
        vec!["del-2".to_string()],
        "a deleted loop must not be listed, got {listed:?}"
    );

    // The name is free: starting it again creates a new loop rather than conflicting.
    let restarted = start(&router, &start_body("del-1", &flow, "{}", &[])).await;
    assert_eq!(
        restarted.status, 200,
        "a deleted name must be reusable, got {restarted:?}"
    );
}

/// Every operation that names a loop answers `ResourceNotFoundException` (HTTP 404) for a
/// name no record exists under — whether it was never started or has been deleted.
#[tokio::test]
async fn an_unknown_loop_name_is_a_resource_not_found_everywhere() {
    let router = emulator();
    let flow = flow_definition_arn("missing-flow");

    let started = start(&router, &start_body("was-here", &flow, "{}", &[])).await;
    assert_eq!(started.status, 200, "StartHumanLoop failed: {started:?}");
    let deleted = delete(&router, "was-here").await;
    assert_eq!(deleted.status, 200, "DeleteHumanLoop failed: {deleted:?}");

    for name in ["never-existed", "was-here"] {
        for operation in ["DescribeHumanLoop", "StopHumanLoop", "DeleteHumanLoop"] {
            let reply = match operation {
                "DescribeHumanLoop" => describe(&router, name).await,
                "StopHumanLoop" => stop(&router, name).await,
                _ => delete(&router, name).await,
            };
            assert_eq!(
                reply.status, 404,
                "case {operation}/{name:?}: expected HTTP 404, got {reply:?}"
            );
            assert_eq!(
                reply.error_code(),
                "ResourceNotFoundException",
                "case {operation}/{name:?}: expected a ResourceNotFoundException, got {reply:?}"
            );
        }
    }
}

/// A request outside the model's constraints is refused with `ValidationException` (HTTP
/// 400), and a request on the constraint's own bound is accepted.
///
/// The constraints come from the model: `HumanLoopName` `@length(1..63)` /
/// `@pattern(^[a-z0-9](-*[a-z0-9])*$)`, `FlowDefinitionArn`'s ARN `@pattern`, `MaxResults`
/// `@range(1..100)` and the `SortOrder` enum. Each range is tested on both published bounds
/// and one step past each.
#[tokio::test]
async fn input_outside_the_model_constraints_is_refused() {
    let router = emulator();
    let flow = flow_definition_arn("validation-flow");

    // Something to list, so a refusal cannot be mistaken for an empty store.
    let started = start(&router, &start_body("valid-loop", &flow, "{}", &[])).await;
    assert_eq!(started.status, 200, "StartHumanLoop failed: {started:?}");

    enum Call {
        Start(serde_json::Value),
        Describe(String),
        Delete(String),
        Stop(String),
        List(String),
    }

    let cases: Vec<(&str, Call, u16)> = vec![
        (
            "HumanLoopName with an underscore (outside the pattern)",
            Call::Start(start_body("Bad_Name", &flow, "{}", &[])),
            400,
        ),
        (
            "HumanLoopName with a leading hyphen (outside the pattern)",
            Call::Start(start_body("-leading", &flow, "{}", &[])),
            400,
        ),
        (
            "empty HumanLoopName (one short of the @length min)",
            Call::Start(start_body("", &flow, "{}", &[])),
            400,
        ),
        (
            "HumanLoopName of 1 character (the @length min)",
            Call::Start(start_body("z", &flow, "{}", &[])),
            200,
        ),
        (
            "HumanLoopName of 63 characters (the @length max)",
            Call::Start(start_body(&"b".repeat(63), &flow, "{}", &[])),
            200,
        ),
        (
            "HumanLoopName of 64 characters (one past the @length max)",
            Call::Start(start_body(&"c".repeat(64), &flow, "{}", &[])),
            400,
        ),
        (
            "FlowDefinitionArn that is not an ARN",
            Call::Start(start_body("arn-check", "not-an-arn", "{}", &[])),
            400,
        ),
        (
            "FlowDefinitionArn naming another resource type",
            Call::Start(start_body(
                "arn-check",
                &format!(
                    "arn:aws:sagemaker:{REGION}:{}:human-loop/x",
                    engine::EMULATOR_ACCOUNT_ID
                ),
                "{}",
                &[],
            )),
            400,
        ),
        (
            "a content classifier outside the enum",
            Call::Start(start_body("classifier-check", &flow, "{}", &["FreeOfSense"])),
            400,
        ),
        (
            "DescribeHumanLoop on a name outside the pattern",
            Call::Describe("Bad_Name".to_string()),
            400,
        ),
        (
            "DeleteHumanLoop on a name outside the pattern",
            Call::Delete("Bad_Name".to_string()),
            400,
        ),
        (
            "StopHumanLoop on a name outside the pattern",
            Call::Stop("Bad_Name".to_string()),
            400,
        ),
        (
            "MaxResults=0 (one short of the @range min)",
            Call::List(format!("FlowDefinitionArn={flow}&MaxResults=0")),
            400,
        ),
        (
            "MaxResults=1 (the @range min)",
            Call::List(format!("FlowDefinitionArn={flow}&MaxResults=1")),
            200,
        ),
        (
            "MaxResults=100 (the @range max)",
            Call::List(format!("FlowDefinitionArn={flow}&MaxResults=100")),
            200,
        ),
        (
            "MaxResults=101 (one past the @range max)",
            Call::List(format!("FlowDefinitionArn={flow}&MaxResults=101")),
            400,
        ),
        (
            "SortOrder outside the enum",
            Call::List(format!("FlowDefinitionArn={flow}&SortOrder=Sideways")),
            400,
        ),
        (
            "ListHumanLoops on a FlowDefinitionArn that is not an ARN",
            Call::List("FlowDefinitionArn=not-an-arn".to_string()),
            400,
        ),
    ];

    for (case, call, expected_status) in &cases {
        let reply = match call {
            Call::Start(body) => start(&router, body).await,
            Call::Describe(name) => describe(&router, name).await,
            Call::Delete(name) => delete(&router, name).await,
            Call::Stop(name) => stop(&router, name).await,
            Call::List(query) => list(&router, query).await,
        };
        assert_eq!(
            reply.status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {reply:?}"
        );
        if *expected_status == 400 {
            assert_eq!(
                reply.error_code(),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {reply:?}"
            );
        }
    }
}

/// The status values this service stores are exactly the model's `HumanLoopStatus` enum.
///
/// Source: `smithy_query(query="shape", shape="HumanLoopStatus")`, which the crate surfaces
/// as `ops::types::HUMAN_LOOP_STATUS_VALUES`.
#[test]
fn the_stored_statuses_are_the_models_human_loop_statuses() {
    let mut stored: Vec<&str> = HumanLoopStatus::ALL
        .iter()
        .map(|status| status.as_wire())
        .collect();
    stored.sort_unstable();
    let mut modelled: Vec<&str> = HUMAN_LOOP_STATUS_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        stored, modelled,
        "the stored statuses must be exactly the model's, got {stored:?}"
    );
}

/// A list with no `MaxResults` serves one bounded page, not the whole store.
///
/// The bound is the model's `MaxResults` maximum (`@range(min: 1, max: 100)`), surfaced as
/// `state::HUMAN_LOOP_PAGE_LIMIT`, and the rest of the matches stay reachable through the
/// `NextToken` that page carries.
#[tokio::test]
async fn a_list_without_max_results_is_bounded_to_one_page() {
    let router = emulator();
    let flow = flow_definition_arn("bounded-flow");

    let expected: Vec<String> = (0..=HUMAN_LOOP_PAGE_LIMIT)
        .map(|index| format!("bulk-{index:03}"))
        .collect();
    for name in &expected {
        let reply = start(&router, &start_body(name, &flow, "{}", &[])).await;
        assert_eq!(reply.status, 200, "StartHumanLoop failed: {reply:?}");
    }

    let first = list(&router, &format!("FlowDefinitionArn={flow}")).await;
    assert_eq!(
        summary_names(&first).len(),
        HUMAN_LOOP_PAGE_LIMIT,
        "an absent MaxResults must serve exactly one page of {HUMAN_LOOP_PAGE_LIMIT}, \
         got {first:?}"
    );
    let token = next_token(&first).expect("a page remains, so a NextToken must be returned");

    let second = list(
        &router,
        &format!("FlowDefinitionArn={flow}&NextToken={token}"),
    )
    .await;
    assert_eq!(
        summary_names(&second).len(),
        1,
        "the remaining match must be on the next page, got {second:?}"
    );
    assert_eq!(
        next_token(&second),
        None,
        "no page remains after the last one, got {second:?}"
    );

    let mut walked = summary_names(&first);
    walked.extend(summary_names(&second));
    walked.sort();
    let mut all = expected;
    all.sort();
    assert_eq!(
        walked, all,
        "the two pages together must hold every loop exactly once"
    );
}

/// The store is bounded: once it holds the quota, a new name is refused with the modelled
/// `ServiceQuotaExceededException` (HTTP 402), and deleting a loop frees the slot again.
#[tokio::test]
async fn starting_a_loop_past_the_store_quota_is_refused() {
    let router = emulator();
    let flow = flow_definition_arn("quota-flow");

    for index in 0..HUMAN_LOOP_QUOTA {
        let name = format!("quota-{index:04}");
        let reply = start(&router, &start_body(&name, &flow, "{}", &[])).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?}: a loop within the quota must be accepted, got {reply:?}"
        );
    }

    let refused = start(&router, &start_body("quota-overflow", &flow, "{}", &[])).await;
    assert_eq!(
        refused.status, 402,
        "a loop past the quota must be refused, got {refused:?}"
    );
    assert_eq!(
        refused.error_code(),
        "ServiceQuotaExceededException",
        "expected a ServiceQuotaExceededException, got {refused:?}"
    );
    // The refusal stored nothing.
    let described = describe(&router, "quota-overflow").await;
    assert_eq!(
        described.status, 404,
        "a refused start must store no record, got {described:?}"
    );
    // A loop already stored is still idempotently re-startable at the quota.
    let idempotent = start(&router, &start_body("quota-0000", &flow, "{}", &[])).await;
    assert_eq!(
        idempotent.status, 200,
        "a re-start of a stored name needs no new slot, got {idempotent:?}"
    );

    let deleted = delete(&router, "quota-0000").await;
    assert_eq!(deleted.status, 200, "DeleteHumanLoop failed: {deleted:?}");
    let accepted = start(&router, &start_body("quota-overflow", &flow, "{}", &[])).await;
    assert_eq!(
        accepted.status, 200,
        "deleting a loop must free a slot, got {accepted:?}"
    );
}
