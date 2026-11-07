// SPDX-License-Identifier: CC0-1.0

//! WHICH log group a function's invocations go to, as every surface that answers for it
//! reports it.
//!
//! RULE: a function's log destination is its own `LoggingConfig$LogGroup` — "the name of the
//! Amazon CloudWatch log group the function sends logs to" — and, when it names none, the
//! default group "`/aws/lambda/<function name>`" its own documentation gives. That is ONE
//! reading ([`service_com_amazonaws_lambda_awsgirapiservice::state::configured_log_group`]),
//! so every surface that reports a function's `LoggingConfig` reports the same group as the
//! delivery of a run's log writes to: `CreateFunction`, `GetFunction`,
//! `GetFunctionConfiguration`, `UpdateFunctionConfiguration` and `PublishVersion` cannot
//! disagree about it, and neither can a published version, which carries the configuration
//! frozen at the instant it was published.
//!
//! The rows differ along what the rule generalises over: whether the create named a
//! `LoggingConfig` at all, whether the one it named carries a `LogGroup`, and whether that
//! group is inside the `/aws/lambda/` prefix or outside it. Each row then reads the function
//! back through every surface above, updates the group and reads it again, and reads the
//! version published before that update. (A `LogGroup` that is the empty string names no
//! group either; the model's own `length`/`pattern` constraints refuse one, so that form is
//! pinned where it is decided, in `state::invocation_logs`'s own unit tests.)
//!
//! Nothing here invokes a function: that the records of a run really land in the group these
//! surfaces report is driven end to end by `invocation_log_delivery.rs` beside this file and,
//! across API Gateway and DynamoDB, by
//! `core/router/tests/all/apigateway_proxy_lambda_ddb_pipeline.rs`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// An opaque deployment package, base64-encoded as `FunctionCode$ZipFile` carries one. No
/// handler runs here, so its contents do not matter: it is an empty archive.
const ZIP_FILE: &str = "UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==";

/// The group a function that names none logs to, as its own documentation gives it.
const DEFAULT_PREFIX: &str = "/aws/lambda/";

/// How long past a due instant this suite waits before the call that needs the function
/// settled. The transition is owed by the CLOCK, so nothing here sleeps on the emulator's
/// behalf.
const SLACK: Duration = Duration::from_millis(150);

/// What a dispatched request answered.
struct Answer {
    status: u16,
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
}

/// This service mounted on its own engine, reached the way the production router reaches it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one Lambda `restJson1` request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the body serializes")
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
        body,
    }
}

/// The `LoggingConfig.LogGroup` a body reports, or `None` where it reports no group at all.
fn reported_group(body: &Value) -> Option<String> {
    body.get("LoggingConfig")
        .or_else(|| {
            body.get("Configuration")
                .and_then(|configuration| configuration.get("LoggingConfig"))
        })
        .and_then(|logging| logging.get("LogGroup"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One row: the `LoggingConfig` the create names, and the group the function therefore logs
/// to.
struct Row {
    case: &'static str,
    /// The `LoggingConfig` member of the create, or `None` for a create that names none.
    logging_config: Option<Value>,
    /// The group every surface must report, or `None` for the function's default group.
    expected: Option<&'static str>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create naming no LoggingConfig at all",
            logging_config: None,
            expected: None,
        },
        Row {
            case: "a LoggingConfig that names a format but no group",
            logging_config: Some(json!({ "LogFormat": "JSON" })),
            expected: None,
        },
        Row {
            case: "a LoggingConfig naming another group under the /aws/lambda prefix",
            logging_config: Some(json!({ "LogGroup": "/aws/lambda/shared-group" })),
            expected: Some("/aws/lambda/shared-group"),
        },
        Row {
            case: "a LoggingConfig naming a group outside that prefix entirely",
            logging_config: Some(json!({ "LogGroup": "orders-log-group" })),
            expected: Some("orders-log-group"),
        },
    ]
}

/// The group a row's function logs to, as a name.
fn expected_group(row: &Row, function: &str) -> String {
    row.expected
        .map(str::to_string)
        .unwrap_or_else(|| format!("{DEFAULT_PREFIX}{function}"))
}

#[tokio::test]
async fn every_surface_reports_the_one_log_group_a_functions_invocations_go_to() {
    let router = emulator();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let function = format!("log-group-{index}");
        let mut create = json!({
            "FunctionName": function,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Code": { "ZipFile": ZIP_FILE },
        });
        if let Some(logging) = &row.logging_config {
            create["LoggingConfig"] = logging.clone();
        }
        let created = call(&router, "POST", FUNCTIONS_URI, &create).await;
        assert_eq!(
            created.status,
            201,
            "setup {case:?}: expected HTTP 201 from CreateFunction, got {} with {:?}",
            created.status,
            created.text()
        );

        let wanted = expected_group(row, &function);
        // Every surface that answers for the function's `LoggingConfig`, including the
        // create's own answer.
        let published = call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{function}/versions"),
            &json!({}),
        )
        .await;
        assert_eq!(
            published.status,
            201,
            "setup {case:?}: expected HTTP 201 from PublishVersion, got {} with {:?}",
            published.status,
            published.text()
        );
        let surfaces: Vec<(&str, Answer)> = vec![
            ("CreateFunction", created),
            ("PublishVersion", published),
            (
                "GetFunctionConfiguration",
                call(
                    &router,
                    "GET",
                    &format!("{FUNCTIONS_URI}/{function}/configuration"),
                    &Value::Null,
                )
                .await,
            ),
            (
                "GetFunction",
                call(&router, "GET", &format!("{FUNCTIONS_URI}/{function}"), &Value::Null).await,
            ),
            (
                "GetFunction on the published version",
                call(
                    &router,
                    "GET",
                    &format!("{FUNCTIONS_URI}/{function}?Qualifier=1"),
                    &Value::Null,
                )
                .await,
            ),
        ];
        for (surface, answer) in &surfaces {
            let reported = reported_group(&answer.json());
            assert_eq!(
                reported.as_deref(),
                Some(wanted.as_str()),
                "case {case:?}: expected {surface} to report the log group {wanted:?}, got \
                 {reported:?} in {}",
                answer.text()
            );
        }

        // A function moved to another group reports the new one, and the version published
        // before the move keeps the group it was published with. The move is a write, so it
        // waits for the create's own settling window to fall due.
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
        let moved = format!("/moved/{function}");
        let updated = call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/{function}/configuration"),
            &json!({ "LoggingConfig": { "LogGroup": moved } }),
        )
        .await;
        assert_eq!(
            updated.status,
            200,
            "setup {case:?}: expected HTTP 200 from UpdateFunctionConfiguration, got {} with \
             {:?}",
            updated.status,
            updated.text()
        );
        let reported = reported_group(&updated.json());
        assert_eq!(
            reported.as_deref(),
            Some(moved.as_str()),
            "case {case:?}: expected the update to report the log group {moved:?}, got \
             {reported:?}"
        );
        let frozen = call(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{function}/configuration?Qualifier=1"),
            &Value::Null,
        )
        .await;
        let reported = reported_group(&frozen.json());
        assert_eq!(
            reported.as_deref(),
            Some(wanted.as_str()),
            "case {case:?}: expected the published version to keep the log group {wanted:?}, \
             got {reported:?} in {}",
            frozen.text()
        );
    }
}
