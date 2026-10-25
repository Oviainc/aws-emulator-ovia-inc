// SPDX-License-Identifier: CC0-1.0

//! How the alias operations refuse a `Name` the modelled `Alias` constraints do not
//! admit, and which names those constraints admit.
//!
//! The rule under test: a value that breaks the constraints its own input member carries
//! is refused the way the framework's boundary constraint walk refuses one — HTTP 400
//! with the error code `ValidationException` and the clause
//! `N validation error(s) detected: Value '<value>' at '<member>' failed to satisfy
//! constraint: <requirement>`, where a `@pattern` requirement quotes the expression
//! WITHOUT the `^`/`$` anchors a whole-value Smithy pattern is wrapped in — rather than
//! with a modelled error for a semantic refusal of a value that satisfies its
//! constraints. That matters here because the walk cannot state this one itself: the
//! `Alias` pattern's negative lookahead is not an expression its regex engine compiles,
//! so an unparseable pattern is treated as satisfied and the value reaches the handler,
//! which has to answer exactly what the walk would have answered.
//!
//! The rule is a property of the REFUSAL, not of one operation or one member, so it is
//! checked on `CreateAlias`, whose `Name` arrives in the request BODY, and on `GetAlias`,
//! whose `Name` is a URI LABEL of a reporting operation — where it is also checked that
//! the refusal comes before the lookup, so a name outside the pattern is not reported as a
//! missing alias. The rows differ along what the rule generalises over: which constraint
//! was broken (`@pattern`, and `@length` one past its maximum, whose clause the walk
//! itself states), the length and the digits of an all-digit name, the character outside
//! the admitted class, and — for the admitted set — the length, the case and the
//! separators of a name the pattern does take.

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

/// The error code AWS answers a broken input constraint with on this service's protocol.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The `Name` requirement as AWS states it: the model's `Alias` pattern
/// (`smithy_query(query="operation", operation="CreateAlias")` reports
/// `Name` targeting `Alias` with `@pattern ^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$`) with the outer
/// `^`/`$` pair dropped and nothing else changed.
const ALIAS_EXPRESSION: &str = r"(?!^[0-9]+$)([a-zA-Z0-9-_]+)";

/// The `@length` maximum the same member carries (`@length(min: 1, max: 128)`).
const ALIAS_MAX_LEN: usize = 128;

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

/// Create one function, with no published version: every row below names the unpublished
/// version, so nothing here depends on a publish.
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

/// One `Name` the model's `Alias` constraints refuse: the case, the name itself and the
/// requirement the refusal states.
struct RefusedName {
    case: &'static str,
    name: String,
    requirement: String,
}

impl RefusedName {
    /// A row refused by the member's `@pattern`.
    fn pattern(case: &'static str, name: impl Into<String>) -> Self {
        Self {
            case,
            name: name.into(),
            requirement: format!(
                "Member must satisfy regular expression pattern: {ALIAS_EXPRESSION}"
            ),
        }
    }

    /// A row refused by the member's `@length` maximum.
    fn too_long(case: &'static str, name: impl Into<String>) -> Self {
        Self {
            case,
            name: name.into(),
            requirement: format!("Member must have length less than or equal to {ALIAS_MAX_LEN}"),
        }
    }

    /// The whole message the refusal states, built from the request the way AWS builds it.
    fn message(&self) -> String {
        format!(
            "1 validation error detected: Value '{}' at 'name' failed to satisfy constraint: {}",
            self.name, self.requirement
        )
    }

    /// Whether this name can also be sent as the URI LABEL `GetAlias` binds `Name` to:
    /// a character that ends the path segment, or one no URI carries unencoded, would
    /// address something else rather than carry the name under test.
    fn fits_a_uri_label(&self) -> bool {
        self.name.is_ascii() && !self.name.contains('/') && !self.name.contains(' ')
    }
}

/// Every row, spanning the class: all-digit names of several lengths and digits (the
/// lookahead the pattern opens with), a character outside the admitted
/// `[a-zA-Z0-9-_]` class, and a name one past the `@length` maximum, whose clause states
/// that bound instead.
fn refused_names() -> Vec<RefusedName> {
    vec![
        RefusedName::pattern("a single digit", "0"),
        RefusedName::pattern("another single digit", "7"),
        RefusedName::pattern("three digits", "593"),
        RefusedName::pattern("digits with a leading zero", "0042"),
        RefusedName::pattern("a long run of digits", "1".repeat(32)),
        RefusedName::pattern("all digits at the length maximum", "9".repeat(ALIAS_MAX_LEN)),
        RefusedName::pattern("a dot", "live.v1"),
        RefusedName::pattern("a space", "live v1"),
        RefusedName::pattern("a slash", "live/v1"),
        RefusedName::pattern("a colon", "live:v1"),
        RefusedName::pattern("the unpublished version label", VERSION_LATEST),
        RefusedName::pattern("a plus sign", "live+v1"),
        RefusedName::pattern("an at sign", "live@v1"),
        RefusedName::pattern("a non-ascii letter", "liveü"),
        RefusedName::too_long("digits one past the length maximum", "9".repeat(ALIAS_MAX_LEN + 1)),
        RefusedName::too_long(
            "an admitted form one past the length maximum",
            format!("a{}", "9".repeat(ALIAS_MAX_LEN)),
        ),
    ]
}

#[tokio::test]
async fn create_alias_refuses_a_name_outside_the_modelled_constraints() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "named").await;

    for row in refused_names() {
        let case = row.case;
        let answer = create_alias(&router, "named", &row.name).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, VALIDATION_EXCEPTION.to_string()),
            "case {case:?}: expected 400 {VALIDATION_EXCEPTION} for {:?}, got {} {}",
            row.name,
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            row.message(),
            "case {case:?}: expected the clause naming the member and its requirement, got {:?}",
            answer.message()
        );
    }

    // A refused create stores nothing, so the refusal is not a report of something that
    // was persisted under a name no alias may carry.
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let record = store
        .functions
        .get("named")
        .expect("the function must be stored");
    assert!(
        record.aliases.is_empty(),
        "a refused CreateAlias must store no alias, got {:?}",
        record.aliases.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn get_alias_refuses_a_name_outside_the_modelled_constraints() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "named").await;
    let created = create_alias(&router, "named", "live").await;
    assert_eq!(
        created.status,
        201,
        "the alias that proves the function is readable must be created, got {} {}",
        created.status,
        created.text()
    );

    for row in refused_names().into_iter().filter(RefusedName::fits_a_uri_label) {
        let case = row.case;
        // The constraint is the member's, so it is refused whether or not the function
        // the request addresses holds any alias at all — and before the lookup, so a
        // name outside it is never reported as a missing resource.
        for reference in ["named", "no-such-function"] {
            let answer = get_alias(&router, reference, &row.name).await;
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, VALIDATION_EXCEPTION.to_string()),
                "case {case:?} on {reference:?}: expected 400 {VALIDATION_EXCEPTION} for {:?}, \
                 got {} {}",
                row.name,
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.message(),
                row.message(),
                "case {case:?} on {reference:?}: expected the clause naming the member and its \
                 requirement, got {:?}",
                answer.message()
            );
        }
    }
}

/// Every name the `Alias` constraints ADMIT, so the refusal above is the constraint's and
/// not a blanket one: one character at the length minimum, either case, a name that
/// merely STARTS with a digit, both separators the class carries, and a name at the
/// length maximum.
fn admitted_names() -> Vec<(&'static str, String)> {
    vec![
        ("one lowercase character, the length minimum", "a".to_string()),
        ("one uppercase character", "A".to_string()),
        ("a digit first, but not all digits", "1a".to_string()),
        ("a digit last", "a1".to_string()),
        ("digits around an underscore", "0_0".to_string()),
        ("a lone hyphen", "-".to_string()),
        ("a lone underscore", "_".to_string()),
        (
            "mixed case with both separators",
            "Live-2024_v2".to_string(),
        ),
        (
            "at the length maximum",
            format!("a{}", "9".repeat(ALIAS_MAX_LEN - 1)),
        ),
    ]
}

#[tokio::test]
async fn an_admitted_alias_name_round_trips() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    create_function(&router, "admitted").await;

    for (case, name) in admitted_names() {
        let created = create_alias(&router, "admitted", &name).await;
        assert_eq!(
            created.status,
            201,
            "case {case:?}: CreateAlias {name:?} must succeed, got {} {}",
            created.status,
            created.text()
        );
        assert_eq!(
            created.string("Name"),
            Some(name.clone()),
            "case {case:?}: CreateAlias must answer the name it stored, got {:?}",
            created.string("Name")
        );

        // Read back through the implemented reader: an alias that was answered but never
        // stored fails here.
        let fetched = get_alias(&router, "admitted", &name).await;
        assert_eq!(
            fetched.status,
            200,
            "case {case:?}: GetAlias {name:?} must report the stored alias, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert_eq!(
            (
                fetched.string("Name"),
                fetched.string("FunctionVersion"),
                fetched.string("AliasArn")
            ),
            (
                Some(name.clone()),
                Some(VERSION_LATEST.to_string()),
                Some(format!("{}:{name}", function_arn("admitted")))
            ),
            "case {case:?}: GetAlias must report the alias as stored, got {}",
            fetched.text()
        );
    }
}
