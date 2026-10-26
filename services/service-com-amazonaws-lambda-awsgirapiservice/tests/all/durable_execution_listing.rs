// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListDurableExecutionsByFunction`: "Returns a list of durable
//! executions for a specified Lambda function. You can filter the results by execution
//! name, status, and start time range. This API supports pagination for large result
//! sets."
//!
//! The rule under test: the operation reports, for the one function its `FunctionName`
//! resolves to, exactly the durable executions that function holds — each entry built
//! from the stored record — narrowed by every filter the request names (`Qualifier`,
//! `DurableExecutionName`, `Statuses`, `StartedAfter`, `StartedBefore`), ordered by the
//! instant each execution started (newest first unless `ReverseOrder` asks for
//! chronological order), and cut into pages of at most `MaxItems` entries whose
//! `NextMarker` is present only while entries remain.
//!
//! Every suite drives real HTTP requests through the router, so the assertions are about
//! the wire behaviour (status, body) rather than any handler's internals. The rows differ
//! along what the rule generalises over: which filter is named and with what value, how
//! many executions match, the direction asked for, the page size (one, several, every
//! entry, more than there are, the documented default and each published bound) and the
//! marker a page continues from.
//!
//! No modelled operation starts a durable execution yet — `Invoke` minting one is its own
//! open issue, `20261026T070343Z-events-lambda-sqs-lambda-invoke-on-a-durable-execution-function-returns`
//! — so the executions under test are started through the service's own state, which is
//! where that handler registers them.
//!
//! The clock-driven half (a running execution whose `ExecutionTimeout` has fallen due) is
//! exercised by moving the stored due instant into the past, so nothing here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, EXECUTION_STATUS_FAILED, EXECUTION_STATUS_RUNNING,
    EXECUTION_STATUS_STOPPED, EXECUTION_STATUS_SUCCEEDED, EXECUTION_STATUS_TIMED_OUT,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `ListDurableExecutionsByFunction` is bound
/// (`GET /2025-12-01/functions/{FunctionName}/durable-executions`).
const DURABLE_EXECUTIONS_URI: &str = "/2025-12-01/functions";

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `lambda` in `us-east-1`. Nothing in it is verified: this
/// harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names, which every minted ARN carries.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// A customer managed key ARN satisfying the model's `KMSKeyArn` pattern, which the
/// function under test names in its `DurableConfig`.
const KMS_KEY: &str = "arn:aws:kms:us-east-1:000000000000:key/durable-executions";

/// The `ExecutionTimeout` the function under test configures: "The maximum time (in
/// seconds) that a durable execution can run before timing out", inside the member's own
/// `@range(min: 1, max: 31622400)`.
const EXECUTION_TIMEOUT_SECONDS: u64 = 3_600;

/// The unpublished version label, as the model's `Version` pattern spells it.
const VERSION_LATEST: &str = "$LATEST";

/// The executions one page reports when the caller names no `MaxItems`, which the
/// member's own documentation states: "Maximum number of executions to return (1-1000).
/// Default is 100."
const DEFAULT_PAGE_SIZE: usize = 100;

/// The bounds the model puts on this operation's `MaxItems` (`@range(min: 0, max:
/// 1000)`), from `smithy_query(query="operation",
/// operation="ListDurableExecutionsByFunction")`.
const MAX_ITEMS_MIN: i64 = 0;
const MAX_ITEMS_MAX: i64 = 1_000;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| self.text())
    }

    /// The `DurableExecutions` the answer reported, in the order it reported them.
    fn executions(&self) -> Vec<Value> {
        self.json()
            .get("DurableExecutions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The names of the executions the answer reported, in order.
    fn names(&self) -> Vec<String> {
        self.executions()
            .iter()
            .map(|execution| {
                execution
                    .get("DurableExecutionName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect()
    }

    /// The `NextMarker` the answer carries, when it carries one.
    fn next_marker(&self) -> Option<String> {
        self.json()
            .get("NextMarker")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `ListDurableExecutionsByFunction` on the function `reference` names, with `query`
/// appended (`""` names no filter at all).
async fn list(router: &MultiServiceRouter, reference: &str, query: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{DURABLE_EXECUTIONS_URI}/{reference}/durable-executions{query}"),
        &Value::Null,
    )
    .await
}

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// `at` as the ISO-8601 instant a timestamp query parameter carries.
fn iso8601(at: SystemTime) -> String {
    DateTime::from(at)
        .fmt(Format::DateTime)
        .expect("a test instant must render as ISO-8601")
}

/// Create a function configured for durable executions, publishing version 1 so a
/// qualified reference resolves too, and bring it to `Active`.
async fn create_durable_function(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Publish": true,
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
            "DurableConfig": {
                "KMSKeyArn": KMS_KEY,
                "ExecutionTimeout": EXECUTION_TIMEOUT_SECONDS,
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {name}: CreateFunction must succeed, got {} {}",
        created.status,
        created.text()
    );
    make_activation_due(engine);
}

/// Move every stored function's activation instant into the past, so the next request
/// materialises the `Pending` -> `Active` move the clock owes. This is what waiting would
/// do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// Start one durable execution of `function`, through the state the `Invoke` handler
/// registers one in, answering the ARN it is addressed by.
fn start_execution(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    name: &str,
    version: &str,
    started: SystemTime,
) -> String {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let record = store
        .functions
        .get_mut(function)
        .unwrap_or_else(|| panic!("case {name}: the store must hold the function {function}"));
    record
        .start_durable_execution(name, version, started)
        .unwrap_or_else(|| panic!("case {name}: starting the execution must answer its ARN"))
}

/// Settle one started execution into a terminal status that ended at `ended`, which is
/// the state a callback, a failure, a stop or a timeout leaves it in.
fn settle_execution(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    name: &str,
    status: &str,
    ended: SystemTime,
) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let record = store
        .functions
        .get_mut(function)
        .unwrap_or_else(|| panic!("case {name}: the store must hold the function {function}"));
    let execution = record
        .durable_executions
        .get_mut(name)
        .unwrap_or_else(|| panic!("case {name}: the function must hold the execution"));
    execution.status = status.to_string();
    execution.ended = Some(ended);
    execution.timeout_due = None;
}

/// The whole-second instant the fixture's offsets are measured from, far enough in the
/// past that every seeded execution has already started and near enough that none of
/// their `ExecutionTimeout`s has fallen due.
fn base_instant() -> SystemTime {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock must be past the epoch")
        .as_secs();
    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds - 600)
}

/// One execution the fixture starts: its name, the version it runs under, how many
/// seconds after [`base_instant`] it started, and the status it carries.
struct Seeded {
    name: &'static str,
    version: &'static str,
    offset: u64,
    status: &'static str,
}

/// The fixture's executions: both versions of the function, every one of the model's
/// `ExecutionStatus` values, and distinct start instants so one order is defined.
fn seeded_executions() -> Vec<Seeded> {
    vec![
        Seeded {
            name: "alpha",
            version: VERSION_LATEST,
            offset: 0,
            status: EXECUTION_STATUS_RUNNING,
        },
        Seeded {
            name: "bravo",
            version: VERSION_LATEST,
            offset: 60,
            status: EXECUTION_STATUS_SUCCEEDED,
        },
        Seeded {
            name: "charlie_1",
            version: "1",
            offset: 120,
            status: EXECUTION_STATUS_FAILED,
        },
        Seeded {
            name: "delta-2",
            version: "1",
            offset: 180,
            status: EXECUTION_STATUS_STOPPED,
        },
        Seeded {
            name: "echo",
            version: VERSION_LATEST,
            offset: 240,
            status: EXECUTION_STATUS_TIMED_OUT,
        },
    ]
}

/// The function the fixture seeds, with every execution of [`seeded_executions`] started
/// against it.
async fn fixture(
    name: &str,
) -> (
    Arc<engine::InMemoryEngine>,
    MultiServiceRouter,
    SystemTime,
    Vec<String>,
) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_durable_function(&router, &engine, name).await;
    let base = base_instant();
    let mut arns = Vec::new();
    for seeded in seeded_executions() {
        let started = base + Duration::from_secs(seeded.offset);
        arns.push(start_execution(
            &engine,
            name,
            seeded.name,
            seeded.version,
            started,
        ));
        if seeded.status != EXECUTION_STATUS_RUNNING {
            settle_execution(
                &engine,
                name,
                seeded.name,
                seeded.status,
                started + Duration::from_secs(30),
            );
        }
    }
    (engine, router, base, arns)
}

/// The order every execution of the fixture is reported in by default: "Set to true to
/// return results in chronological order (oldest first). Default is false", so the
/// newest-started comes first.
fn newest_first() -> Vec<String> {
    let mut names: Vec<(u64, String)> = seeded_executions()
        .into_iter()
        .map(|seeded| (seeded.offset, seeded.name.to_string()))
        .collect();
    names.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));
    names.into_iter().map(|(_, name)| name).collect()
}

#[tokio::test]
async fn a_listed_execution_reports_the_record_the_store_holds() {
    let (engine, router, base, arns) = fixture("durable").await;

    let answer = list(&router, "durable", "?DurableExecutionName=alpha").await;
    assert_eq!(
        answer.status,
        200,
        "the listing must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    let executions = answer.executions();
    assert_eq!(
        executions.len(),
        1,
        "case alpha: expected the one execution of that name, got {executions:?}"
    );
    let reported = &executions[0];
    let expected_arn = &arns[0];
    assert_eq!(
        reported.get("DurableExecutionArn").and_then(Value::as_str),
        Some(expected_arn.as_str()),
        "case alpha: expected the stored ARN {expected_arn}, got {reported:?}"
    );
    assert!(
        expected_arn.starts_with(&format!(
            "{}:{VERSION_LATEST}/durable-execution/alpha/",
            function_arn("durable")
        )),
        "case alpha: expected the ARN form the model's DurableExecutionArn pattern \
         admits, got {expected_arn}"
    );
    assert_eq!(
        reported.get("FunctionArn").and_then(Value::as_str),
        Some(function_arn("durable").as_str()),
        "case alpha: expected the ARN of the version the execution runs under, got \
         {reported:?}"
    );
    assert_eq!(
        reported.get("Status").and_then(Value::as_str),
        Some(EXECUTION_STATUS_RUNNING),
        "case alpha: expected the stored status, got {reported:?}"
    );
    assert_eq!(
        reported.get("StartTimestamp").and_then(Value::as_f64),
        Some(DateTime::from(base).as_secs_f64()),
        "case alpha: expected the instant the execution started ({}), got {reported:?}",
        iso8601(base)
    );
    assert!(
        reported.get("EndTimestamp").is_none_or(Value::is_null),
        "case alpha: a running execution has not ended, got {reported:?}"
    );
    assert_eq!(
        reported.get("KMSKeyArn").and_then(Value::as_str),
        Some(KMS_KEY),
        "case alpha: expected the key the function's DurableConfig names, got {reported:?}"
    );

    // A settled execution reports the instant it ended, which is the record's.
    let settled = list(&router, "durable", "?DurableExecutionName=bravo").await;
    let bravo = settled.executions();
    assert_eq!(
        bravo.len(),
        1,
        "case bravo: expected the one execution of that name, got {bravo:?}"
    );
    let ended = base + Duration::from_secs(60 + 30);
    assert_eq!(
        bravo[0].get("EndTimestamp").and_then(Value::as_f64),
        Some(DateTime::from(ended).as_secs_f64()),
        "case bravo: expected the instant the execution ended ({}), got {:?}",
        iso8601(ended),
        bravo[0]
    );
    assert_eq!(
        bravo[0].get("Status").and_then(Value::as_str),
        Some(EXECUTION_STATUS_SUCCEEDED),
        "case bravo: expected the stored status, got {:?}",
        bravo[0]
    );

    // A read changes nothing a later caller sees: the same request answers the same
    // page, and the execution name identifies one execution, so starting it again adds
    // none.
    let again = list(&router, "durable", "?DurableExecutionName=alpha").await;
    assert_eq!(
        again.json(),
        answer.json(),
        "case alpha: a second read must report the same page, got {}",
        again.text()
    );
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let restarted = {
        let mut store = state.lock();
        let record = store
            .functions
            .get_mut("durable")
            .expect("the store must hold the function");
        record.start_durable_execution("alpha", VERSION_LATEST, base)
    };
    assert!(
        restarted.is_none(),
        "case alpha: an execution name is unique within the function, so a second start \
         must not replace it, got {restarted:?}"
    );
}

/// One listing the fixture's executions are narrowed by: how the function is addressed,
/// the query, and the names expected in the order they must be reported.
struct Filtered {
    case: &'static str,
    reference: String,
    query: String,
    expected: Vec<&'static str>,
}

/// The filter table: no filter at all, each filter the operation models, a filter that
/// matches nothing, two filters together, and both documented directions.
fn filtered_cases(base: SystemTime) -> Vec<Filtered> {
    let at_120 = iso8601(base + Duration::from_secs(120));
    let window_open = iso8601(base + Duration::from_secs(30));
    let window_close = iso8601(base + Duration::from_secs(200));
    vec![
        Filtered {
            case: "no filter at all",
            reference: "durable".to_string(),
            query: String::new(),
            expected: vec!["echo", "delta-2", "charlie_1", "bravo", "alpha"],
        },
        Filtered {
            case: "the name of one execution",
            reference: "durable".to_string(),
            query: "?DurableExecutionName=delta-2".to_string(),
            expected: vec!["delta-2"],
        },
        Filtered {
            case: "a name no execution carries",
            reference: "durable".to_string(),
            query: "?DurableExecutionName=zulu".to_string(),
            expected: Vec::new(),
        },
        Filtered {
            case: "one status",
            reference: "durable".to_string(),
            query: format!("?Statuses={EXECUTION_STATUS_RUNNING}"),
            expected: vec!["alpha"],
        },
        Filtered {
            case: "two statuses",
            reference: "durable".to_string(),
            query: format!(
                "?Statuses={EXECUTION_STATUS_SUCCEEDED}&Statuses={EXECUTION_STATUS_FAILED}"
            ),
            expected: vec!["charlie_1", "bravo"],
        },
        Filtered {
            case: "a status and a name that name different executions",
            reference: "durable".to_string(),
            query: format!("?DurableExecutionName=echo&Statuses={EXECUTION_STATUS_RUNNING}"),
            expected: Vec::new(),
        },
        Filtered {
            case: "a qualifier naming a published version",
            reference: "durable".to_string(),
            query: "?Qualifier=1".to_string(),
            expected: vec!["delta-2", "charlie_1"],
        },
        Filtered {
            case: "a qualifier naming the unpublished version",
            reference: "durable".to_string(),
            query: format!("?Qualifier={VERSION_LATEST}"),
            expected: vec!["echo", "bravo", "alpha"],
        },
        Filtered {
            case: "a function name that carries the qualifier itself",
            reference: "durable:1".to_string(),
            query: String::new(),
            expected: vec!["delta-2", "charlie_1"],
        },
        Filtered {
            case: "executions started after an instant",
            reference: "durable".to_string(),
            query: format!("?StartedAfter={at_120}"),
            expected: vec!["echo", "delta-2"],
        },
        Filtered {
            case: "executions started before an instant",
            reference: "durable".to_string(),
            query: format!("?StartedBefore={at_120}"),
            expected: vec!["bravo", "alpha"],
        },
        Filtered {
            case: "a start-time window",
            reference: "durable".to_string(),
            query: format!("?StartedAfter={window_open}&StartedBefore={window_close}"),
            expected: vec!["delta-2", "charlie_1", "bravo"],
        },
        Filtered {
            case: "a start-time window and a status",
            reference: "durable".to_string(),
            query: format!(
                "?StartedAfter={window_open}&StartedBefore={window_close}\
                 &Statuses={EXECUTION_STATUS_FAILED}"
            ),
            expected: vec!["charlie_1"],
        },
        Filtered {
            case: "chronological order",
            reference: "durable".to_string(),
            query: "?ReverseOrder=true".to_string(),
            expected: vec!["alpha", "bravo", "charlie_1", "delta-2", "echo"],
        },
        Filtered {
            case: "the default order, named",
            reference: "durable".to_string(),
            query: "?ReverseOrder=false".to_string(),
            expected: vec!["echo", "delta-2", "charlie_1", "bravo", "alpha"],
        },
    ]
}

#[tokio::test]
async fn every_filter_narrows_the_listing_to_the_executions_it_names() {
    let (_engine, router, base, _arns) = fixture("durable").await;

    for case in filtered_cases(base) {
        let answer = list(&router, &case.reference, &case.query).await;
        assert_eq!(
            answer.status,
            200,
            "case {}: expected 200, got {} {}",
            case.case,
            answer.status,
            answer.text()
        );
        let names = answer.names();
        let expected: Vec<String> = case.expected.iter().map(|name| (*name).to_string()).collect();
        assert_eq!(
            names, expected,
            "case {}: expected {expected:?}, got {names:?}",
            case.case
        );
        assert!(
            answer.next_marker().is_none(),
            "case {}: every match fits one page, so no NextMarker is reported, got {:?}",
            case.case,
            answer.next_marker()
        );
    }
}

#[tokio::test]
async fn max_items_and_marker_page_the_whole_order_without_repeating_an_entry() {
    let (_engine, router, _base, _arns) = fixture("durable").await;
    let expected = newest_first();

    // One entry at a time, several, exactly as many as there are, more than there are,
    // and each published bound of `MaxItems`.
    for size in [1usize, 2, 3, 5, 10, 1_000] {
        let mut collected: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &marker {
                Some(marker) => format!("?MaxItems={size}&Marker={marker}"),
                None => format!("?MaxItems={size}"),
            };
            let answer = list(&router, "durable", &query).await;
            assert_eq!(
                answer.status,
                200,
                "case MaxItems {size} page {pages}: expected 200, got {} {}",
                answer.status,
                answer.text()
            );
            let names = answer.names();
            assert!(
                names.len() <= size,
                "case MaxItems {size} page {pages}: expected at most {size} entries, got \
                 {names:?}"
            );
            collected.extend(names.clone());
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case MaxItems {size}: paging must terminate, got {collected:?}"
            );
            match answer.next_marker() {
                Some(next) => {
                    assert_eq!(
                        names.len(),
                        size.min(expected.len()),
                        "case MaxItems {size} page {pages}: a page that reports a NextMarker \
                         is full, got {names:?}"
                    );
                    marker = Some(next);
                }
                None => break,
            }
        }
        assert_eq!(
            collected, expected,
            "case MaxItems {size}: expected the whole order exactly once, got {collected:?}"
        );
    }
}

#[tokio::test]
async fn a_caller_that_names_no_page_size_is_answered_the_documented_default() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_durable_function(&router, &engine, "wide").await;
    let base = base_instant();
    // One past the documented default, so the default page is full and one entry
    // remains for the page the marker continues.
    let total = DEFAULT_PAGE_SIZE + 1;
    for index in 0..total {
        let offset = u64::try_from(index).unwrap_or_default();
        start_execution(
            &engine,
            "wide",
            &format!("execution-{index}"),
            VERSION_LATEST,
            base + Duration::from_secs(offset),
        );
    }

    let first = list(&router, "wide", "").await;
    assert_eq!(
        first.status,
        200,
        "the listing must succeed, got {} {}",
        first.status,
        first.text()
    );
    assert_eq!(
        first.names().len(),
        DEFAULT_PAGE_SIZE,
        "case no MaxItems: expected the documented default of {DEFAULT_PAGE_SIZE} \
         executions, got {}",
        first.names().len()
    );
    let marker = first.next_marker().unwrap_or_else(|| {
        panic!(
            "case no MaxItems: {total} executions do not fit one default page, so a \
             NextMarker is reported, got {}",
            first.text()
        )
    });

    let second = list(&router, "wide", &format!("?Marker={marker}")).await;
    assert_eq!(
        second.names(),
        vec!["execution-0".to_string()],
        "case no MaxItems page 2: expected the one execution left, got {:?}",
        second.names()
    );
    assert!(
        second.next_marker().is_none(),
        "case no MaxItems page 2: nothing remains, so no NextMarker is reported, got {:?}",
        second.next_marker()
    );

    // The published maximum reports every execution in one page.
    let whole = list(&router, "wide", &format!("?MaxItems={MAX_ITEMS_MAX}")).await;
    assert_eq!(
        whole.names().len(),
        total,
        "case MaxItems {MAX_ITEMS_MAX}: expected every one of the {total} executions, got {}",
        whole.names().len()
    );
    assert!(
        whole.next_marker().is_none(),
        "case MaxItems {MAX_ITEMS_MAX}: nothing remains, so no NextMarker is reported, got {:?}",
        whole.next_marker()
    );
}

/// One request the operation refuses, and the modelled failure it refuses it with.
struct Refusal {
    case: &'static str,
    reference: String,
    query: String,
    status: u16,
    code: &'static str,
}

/// The refusal table: a function this emulator does not hold, however it is addressed; a
/// qualifier naming a version it does not have; a `FunctionName` and a `Qualifier` naming
/// two different versions; and a marker naming no execution in the order.
fn refusal_cases() -> Vec<Refusal> {
    vec![
        Refusal {
            case: "a function that does not exist",
            reference: "absent".to_string(),
            query: String::new(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "an ARN of a function that does not exist",
            reference: function_arn("absent"),
            query: String::new(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "a version the function does not have",
            reference: "durable".to_string(),
            query: "?Qualifier=7".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "an alias the function does not have",
            reference: "durable".to_string(),
            query: "?Qualifier=live".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "a name and a qualifier naming two different versions",
            reference: "durable:1".to_string(),
            query: format!("?Qualifier={VERSION_LATEST}"),
            status: 400,
            code: "InvalidParameterValueException",
        },
        Refusal {
            case: "a marker naming no execution in the order",
            reference: "durable".to_string(),
            query: "?Marker=not-an-execution-arn".to_string(),
            status: 400,
            code: "InvalidParameterValueException",
        },
        Refusal {
            case: "a marker whose execution id names no execution",
            reference: "durable".to_string(),
            query: format!(
                "?Statuses={EXECUTION_STATUS_RUNNING}&Marker={}:1/durable-execution/delta-2/1",
                function_arn("durable")
            ),
            status: 400,
            code: "InvalidParameterValueException",
        },
    ]
}

#[tokio::test]
async fn the_listing_refuses_what_it_cannot_answer() {
    let (_engine, router, _base, _arns) = fixture("durable").await;

    for case in refusal_cases() {
        let answer = list(&router, &case.reference, &case.query).await;
        assert_eq!(
            answer.status, case.status,
            "case {}: expected {}, got {} {}",
            case.case,
            case.status,
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(case.code),
            "case {}: expected {}, got {}",
            case.case,
            case.code,
            answer.error_code()
        );
    }

    // `MaxItems` is modelled `@range(min: 0, max: 1000)`: a page size outside it is
    // refused rather than paged over.
    for max_items in [MAX_ITEMS_MIN - 1, MAX_ITEMS_MAX + 1] {
        let answer = list(&router, "durable", &format!("?MaxItems={max_items}")).await;
        assert_eq!(
            answer.status,
            400,
            "case MaxItems {max_items}: expected 400, got {} {}",
            answer.status,
            answer.text()
        );
    }
}

#[tokio::test]
async fn a_running_execution_is_reported_timed_out_once_its_timeout_falls_due() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_durable_function(&router, &engine, "durable").await;
    let base = base_instant();
    start_execution(&engine, "durable", "alpha", VERSION_LATEST, base);

    let running = list(&router, "durable", "").await;
    assert_eq!(
        running.names(),
        vec!["alpha".to_string()],
        "expected the started execution, got {}",
        running.text()
    );
    assert_eq!(
        running.executions()[0].get("Status").and_then(Value::as_str),
        Some(EXECUTION_STATUS_RUNNING),
        "an execution inside its ExecutionTimeout is still running, got {:?}",
        running.executions()[0]
    );

    // What waiting out the configured ExecutionTimeout would do, without waiting: "the
    // maximum time (in seconds) that a durable execution can run before timing out". The
    // instant is a whole second, so the timestamp the wire reports is exact.
    let due = base + Duration::from_secs(300);
    {
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let record = store
            .functions
            .get_mut("durable")
            .expect("the store must hold the function");
        let execution = record
            .durable_executions
            .get_mut("alpha")
            .expect("the function must hold the execution");
        execution.timeout_due = Some(due);
    }

    let timed_out = list(&router, "durable", "").await;
    let reported = timed_out.executions();
    assert_eq!(
        reported.len(),
        1,
        "expected the one execution, got {}",
        timed_out.text()
    );
    assert_eq!(
        reported[0].get("Status").and_then(Value::as_str),
        Some(EXECUTION_STATUS_TIMED_OUT),
        "an execution past its ExecutionTimeout timed out, got {:?}",
        reported[0]
    );
    assert_eq!(
        reported[0].get("EndTimestamp").and_then(Value::as_f64),
        Some(DateTime::from(due).as_secs_f64()),
        "expected the execution to have ended when its timeout fell due ({}), got {:?}",
        iso8601(due),
        reported[0]
    );
    assert_eq!(
        reported[0].get("StartTimestamp").and_then(Value::as_f64),
        Some(DateTime::from(base).as_secs_f64()),
        "expected the start instant to be unchanged ({}), got {:?}",
        iso8601(base),
        reported[0]
    );

    // The transition the clock owed is applied once and stays applied: a later read
    // reports the same thing, and filtering on the new status finds it.
    let again = list(&router, "durable", &format!("?Statuses={EXECUTION_STATUS_TIMED_OUT}")).await;
    assert_eq!(
        again.executions(),
        reported,
        "a second read must report the same execution, got {}",
        again.text()
    );
}
