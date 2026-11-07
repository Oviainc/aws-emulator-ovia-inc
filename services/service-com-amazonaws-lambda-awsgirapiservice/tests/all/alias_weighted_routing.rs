// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TRAFFIC SPLIT an alias performs: `Invoke` through an alias whose
//! `RoutingConfig` names a second version.
//!
//! The rule under test: "You can also map an alias to split invocation requests between two
//! versions. Use the `RoutingConfig` parameter to specify a second version and the percentage
//! of invocation requests that it receives" (`CreateAlias`). So an invocation of such an alias
//! resolves to a version PER INVOCATION, with the share the configuration gives each: the
//! `X-Amz-Executed-Version` of the answers varies across the pair, and the code that ran is
//! the code of the version each answer reports. An alias with no routing configuration
//! invokes the version it names, every time.
//!
//! The rows differ along what the rule generalises over: the weight the second version
//! carries, including each published bound of the member's own range
//! (`smithy_query(query="shape", shape="AdditionalVersionWeights")`: `@range(min: 0.0, max:
//! 1.0)`), and an alias that shifts nothing at all. Each row reads BOTH the version the
//! answer reports and the value the handler returned, because the two agreeing is what says
//! the invocation ran the version it was routed to.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module every package below holds the handler in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How many invocations one row of the distribution table sends through the alias.
///
/// Enough that a split which really draws per invocation reaches both versions: the chance a
/// correct emulator sends all of them to one version of the narrowest split below is under
/// one in ten million, while a split that is not consulted at all fails on the first sample.
const INVOCATIONS_PER_ROW: usize = 60;

/// How many invocations RUN the handler, to tie the version an answer reports to the code it
/// ran. Far fewer than above, because each one is a run of the interpreter rather than a
/// resolution, and the SHARE each version receives is what the dry-run table above measures.
const RUNS_PER_SPLIT: usize = 6;

/// "For the DryRun invocation type, the status code is 204" (`Invoke$StatusCode`).
const DRY_RUN_STATUS: u16 = 204;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &[u8]) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The handler a version reports itself from: each published version carries its own number,
/// so the value an answer returns says which version's code ran.
fn source_reporting(version: u32) -> String {
    format!("def lambda_handler(event, context):\n    return {{'ran': {version}}}\n")
}

/// Create a function with two published versions, each running its own code, and wait out the
/// provisioning window the create documents.
async fn create_two_versions(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Publish": true,
            "Timeout": 5,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&source_reporting(1))),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
    let updated = call(
        router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/code"),
        &serde_json::to_vec(&json!({
            "ZipFile": base64::engine::general_purpose::STANDARD
                .encode(package(&source_reporting(2))),
            "Publish": true,
        }))
        .expect("the update body serializes"),
    )
    .await;
    assert_eq!(
        updated.status,
        200,
        "UpdateFunctionCode on {name:?} must succeed, got {} {}",
        updated.status,
        updated.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// Point the alias `alias` at version 1, shifting `weights` of its invocations elsewhere.
async fn create_alias(
    router: &MultiServiceRouter,
    name: &str,
    alias: &str,
    weights: &BTreeMap<String, f64>,
) {
    let mut body = json!({ "Name": alias, "FunctionVersion": "1" });
    if !weights.is_empty() {
        body["RoutingConfig"] = json!({ "AdditionalVersionWeights": weights });
    }
    let created = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &serde_json::to_vec(&body).expect("the alias body serializes"),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateAlias {alias:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
}

/// One row: the case, the weight the second version carries (none at all for an alias that
/// shifts nothing), and the versions the invocations must be spread over.
struct Row {
    case: &'static str,
    weight: Option<f64>,
    expected: &'static [&'static str],
}

/// A DRY RUN of the alias `reference` names: it resolves the qualifier and runs nothing.
async fn dry_run(router: &MultiServiceRouter, reference: &str) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "http://lambda.us-east-1.amazonaws.com{FUNCTIONS_URI}/{reference}/invocations"
        ))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .header("x-amz-invocation-type", "DryRun")
        .body(bytes::Bytes::from_static(b"{}"))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// An alias splits its invocations between the two versions its routing configuration names:
/// the version each invocation resolves to is drawn per invocation, so the answers are spread
/// over the pair.
///
/// The invocations are dry runs — "Validate parameter values and verify that the user or role
/// has permission to invoke the function" — because the RESOLUTION is what this table is
/// about: a dry run reports the version the qualifier resolved to and runs nothing, so a row
/// can take enough samples to see a share rather than an accident.
/// `an_invocation_runs_the_code_of_the_version_it_was_routed_to` is what ties that version to
/// the code of the run.
#[tokio::test]
async fn a_weighted_alias_spreads_its_invocations_over_both_versions() {
    let rows: &[Row] = &[
        Row {
            case: "no routing configuration at all",
            weight: None,
            expected: &["1"],
        },
        Row {
            case: "half the invocations shifted",
            weight: Some(0.5),
            expected: &["1", "2"],
        },
        Row {
            case: "a quarter of the invocations shifted",
            weight: Some(0.25),
            expected: &["1", "2"],
        },
        Row {
            case: "three quarters of the invocations shifted",
            weight: Some(0.75),
            expected: &["1", "2"],
        },
        Row {
            case: "the whole of the traffic shifted, the top of the member's range",
            weight: Some(1.0),
            expected: &["2"],
        },
        Row {
            case: "nothing shifted, the bottom of the member's range",
            weight: Some(0.0),
            expected: &["1"],
        },
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-weighted";
    create_two_versions(&router, name).await;
    for (index, row) in rows.iter().enumerate() {
        let alias = format!("live{index}");
        let weights: BTreeMap<String, f64> = row
            .weight
            .map(|weight| [("2".to_string(), weight)].into_iter().collect())
            .unwrap_or_default();
        create_alias(&router, name, &alias, &weights).await;
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for _ in 0..INVOCATIONS_PER_ROW {
            let answered = dry_run(&router, &format!("{name}:{alias}")).await;
            assert_eq!(
                answered.status,
                DRY_RUN_STATUS,
                "case {}: expected the dry run to be served, got {} {}",
                row.case,
                answered.status,
                answered.text()
            );
            let executed = answered
                .header("x-amz-executed-version")
                .unwrap_or_default();
            assert!(
                row.expected.contains(&executed.as_str()),
                "case {}: an invocation may only reach {:?}, got {executed:?}",
                row.case,
                row.expected
            );
            *seen.entry(executed).or_default() += 1;
        }
        for expected in row.expected {
            assert!(
                seen.contains_key(*expected),
                "case {}: {INVOCATIONS_PER_ROW} invocations must reach version {expected}, got \
                 {seen:?}",
                row.case
            );
        }
    }
}

/// The version an answer REPORTS is the version whose code the invocation ran: each answer's
/// payload is the one that version's handler returns, whichever of the pair the invocation
/// was routed to.
///
/// That both versions are REACHED over many invocations is the dry-run table's assertion;
/// this one is the tie between the version reported and the code that ran, so it needs only
/// as many runs as there are versions to be routed to.
#[tokio::test]
async fn an_invocation_runs_the_code_of_the_version_it_was_routed_to() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-weighted-run";
    create_two_versions(&router, name).await;
    let weights: BTreeMap<String, f64> = [("2".to_string(), 0.5)].into_iter().collect();
    create_alias(&router, name, "live", &weights).await;
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for attempt in 0..RUNS_PER_SPLIT {
        let answered = call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}:live/invocations"),
            b"{}",
        )
        .await;
        assert_eq!(
            answered.status,
            200,
            "case invocation {attempt}: expected 200, got {} {}",
            answered.status,
            answered.text()
        );
        let executed = answered
            .header("x-amz-executed-version")
            .unwrap_or_default();
        let ran = answered
            .json()
            .get("ran")
            .and_then(Value::as_u64)
            .map(|ran| ran.to_string())
            .unwrap_or_default();
        assert_eq!(
            ran,
            executed,
            "case invocation {attempt}: the answer reports version {executed:?}, so the code of \
             that version must be what ran, got {ran:?} from {}",
            answered.text()
        );
        *seen.entry(executed).or_default() += 1;
    }
    for version in seen.keys() {
        assert!(
            version == "1" || version == "2",
            "case the routed set: an invocation may only run one of the two configured \
             versions, got {version:?}"
        );
    }
}

/// The split is the INVOKE path's alone: reading the alias reports the version it invokes,
/// whatever an invocation of it was routed to, and reading it again reports the same.
#[tokio::test]
async fn reading_a_weighted_alias_reports_the_version_it_invokes() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-weighted-read";
    create_two_versions(&router, name).await;
    let weights: BTreeMap<String, f64> = [("2".to_string(), 0.5)].into_iter().collect();
    create_alias(&router, name, "live", &weights).await;
    for case in ["before any invocation", "after a round of invocations"] {
        let read = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/aliases/live"),
            &[],
        )
        .await;
        assert_eq!(
            read.status,
            200,
            "case {case}: GetAlias must succeed, got {} {}",
            read.status,
            read.text()
        );
        let reported = read.json();
        assert_eq!(
            reported.get("FunctionVersion").and_then(Value::as_str),
            Some("1"),
            "case {case}: the alias invokes the version it was created on, got {reported}"
        );
        assert_eq!(
            reported
                .get("RoutingConfig")
                .and_then(|routing| routing.get("AdditionalVersionWeights"))
                .and_then(|weights| weights.get("2"))
                .and_then(Value::as_f64),
            Some(0.5),
            "case {case}: the alias reports the split it was configured with, got {reported}"
        );
        // Then a round of invocations, which the second pass reads the alias after: the
        // DRY RUNS resolve the alias per invocation exactly as a run does, so the round
        // exercises the draw without spending the shared run slots on it.
        for _ in 0..ROUND_BEFORE_THE_SECOND_READ {
            let answered = dry_run(&router, &format!("{name}:live")).await;
            assert_eq!(
                answered.status,
                DRY_RUN_STATUS,
                "case {case}: expected the dry run to be served, got {} {}",
                answered.status,
                answered.text()
            );
        }
    }
}

/// How many invocations stand between the two reads of the alias below.
const ROUND_BEFORE_THE_SECOND_READ: usize = 8;
