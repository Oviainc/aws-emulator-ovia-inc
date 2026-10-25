// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE `GetAlias` refuses a name the function holds no alias of with.
//!
//! The rule under test: a refusal that an ALIAS does not exist names the alias by
//! the fully-qualified ARN the request asked about — the function's own
//! unqualified ARN with the alias name as its qualifier — after this service's
//! fixed `Cannot find alias arn: ` prefix, so a caller recovers the ARN it asked
//! for by stripping that prefix. The ARN is wholly request-derived (`FunctionName`
//! and `Name` are the operation's two URI labels), so the same alias asked for
//! through any documented `FunctionName` form is refused with the same sentence.
//!
//! The rows differ along what the rule generalises over: the alias name (its
//! modelled length bounds, either separator, mixed case), the documented
//! `FunctionName` form the request addressed the function by (name, function ARN,
//! partial ARN), and whether the function holds other aliases. The refusal for a
//! missing FUNCTION is checked too, because it is a different resource and keeps
//! its own sentence — a caller has to be able to tell "no such function" from
//! "that function has no such alias".

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The unpublished version label, from the model's `Version` pattern
/// (`^(\$LATEST|[0-9]+)$`).
const VERSION_LATEST: &str = "$LATEST";

/// The `@length` maximum the model puts on `Alias` (`@length(min: 1, max: 128)`),
/// from `smithy_query(query="operation", operation="GetAlias")`.
const ALIAS_MAX_LEN: usize = 128;

/// The error code the model declares for a resource `GetAlias` cannot find, at
/// HTTP 404.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

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

    /// A `String` member of the JSON body.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header), with any
    /// `namespace#` prefix dropped.
    fn error_code(&self) -> String {
        let raw = self
            .string("__type")
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string());
        raw.rsplit('#').next().unwrap_or(&raw).to_string()
    }

    /// The `message` of an error body, under either casing the JSON envelopes use.
    fn message(&self) -> String {
        self.string("message")
            .or_else(|| self.string("Message"))
            .unwrap_or_else(|| self.text())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
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

/// The unqualified ARN a function of `name` carries.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// Create one function, with no published version: every row below names an alias
/// of the unpublished version, so nothing here depends on a publish.
async fn create_function(router: &MultiServiceRouter, name: &str) {
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
            "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
}

/// `CreateAlias` of `name` on the function `reference` names.
async fn create_alias(router: &MultiServiceRouter, reference: &str, name: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/aliases"),
        &json!({ "Name": name, "FunctionVersion": VERSION_LATEST }),
    )
    .await
}

/// `GetAlias` of `name` on the function `reference` names.
async fn get_alias(router: &MultiServiceRouter, reference: &str, name: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{reference}/aliases/{name}"),
        &Value::Null,
    )
    .await
}

/// Alias names the modelled `Alias` constraints admit, spanning the class the
/// sentence generalises over: the length minimum, both separators the pattern
/// carries, mixed case with digits, and the length maximum.
fn missing_alias_names() -> Vec<(&'static str, String)> {
    vec![
        ("one character, the length minimum", "g".to_string()),
        ("a hyphenated name", "ghost-alias".to_string()),
        ("an underscored name", "ghost_alias".to_string()),
        ("mixed case with digits", "Ghost2024v2".to_string()),
        (
            "at the length maximum",
            format!("g{}", "h".repeat(ALIAS_MAX_LEN - 1)),
        ),
    ]
}

#[tokio::test]
async fn an_alias_the_function_does_not_hold_is_refused_by_its_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "ghosted").await;
    // One alias that DOES exist, so each refusal below is about the name asked
    // for rather than about a function with no aliases at all.
    let created = create_alias(&router, "ghosted", "live").await;
    assert_eq!(
        created.status,
        201,
        "the alias that proves the function is readable must be created, got {} {}",
        created.status,
        created.text()
    );

    for (form, alias) in missing_alias_names() {
        // Rows differ along the documented `FunctionName` form: the ARN in the
        // sentence is the function's own, however the request addressed it.
        for reference in [
            "ghosted".to_string(),
            function_arn("ghosted"),
            format!("{ACCOUNT}:function:ghosted"),
        ] {
            let case = format!("{form} addressed by {reference}");
            let expected = format!(
                "Cannot find alias arn: {}:{alias}",
                function_arn("ghosted")
            );
            let answer = get_alias(&router, &reference, &alias).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (404, RESOURCE_NOT_FOUND.to_string()),
                "case {case:?}: expected 404 {RESOURCE_NOT_FOUND}, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );

            // The read changed nothing: the ghost alias was not created by being
            // asked for, and the real one is still there.
            let again = get_alias(&router, &reference, &alias).await;
            assert_eq!(
                (again.status, again.message()),
                (404, expected.clone()),
                "case {case:?}: a repeated read must answer the same refusal, got {} {}",
                again.status,
                again.text()
            );
            let live = get_alias(&router, &reference, "live").await;
            assert_eq!(
                (live.status, live.string("Name")),
                (200, Some("live".to_string())),
                "case {case:?}: the alias that exists must still be reported, got {} {}",
                live.status,
                live.text()
            );
        }
    }

    // Nothing the refusals asked for was stored: the function holds exactly the
    // one alias that was created.
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let record = store
        .functions
        .get("ghosted")
        .expect("the function must be stored");
    assert_eq!(
        record.aliases.keys().cloned().collect::<Vec<_>>(),
        vec!["live".to_string()],
        "a refused GetAlias must store no alias, got {:?}",
        record.aliases.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_function_with_no_aliases_refuses_every_alias_by_its_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "aliasless").await;

    for (form, alias) in missing_alias_names() {
        let expected = format!(
            "Cannot find alias arn: {}:{alias}",
            function_arn("aliasless")
        );
        let answer = get_alias(&router, "aliasless", &alias).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, RESOURCE_NOT_FOUND.to_string()),
            "case {form:?}: expected 404 {RESOURCE_NOT_FOUND}, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {form:?}: expected {expected:?}, got {:?}",
            answer.message()
        );
    }
}

/// RULE: the missing resource is the one the sentence names — a FUNCTION that does
/// not exist keeps the function-shaped refusal, so a caller can tell the two apart.
#[tokio::test]
async fn a_missing_function_keeps_its_own_refusal() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (form, alias) in missing_alias_names() {
        let answer = get_alias(&router, "never-created", &alias).await;
        let expected = format!("Function not found: {}", function_arn("never-created"));
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, RESOURCE_NOT_FOUND.to_string()),
            "case {form:?}: expected 404 {RESOURCE_NOT_FOUND}, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {form:?}: a missing function must be named as the function, got {:?}",
            answer.message()
        );
    }
}
