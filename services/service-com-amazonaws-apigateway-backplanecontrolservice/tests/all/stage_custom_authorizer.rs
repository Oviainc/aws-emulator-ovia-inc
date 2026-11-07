// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `CUSTOM` authorizer of a deployed stage: what the `execute-api`
//! data plane answers for a method whose `authorizationType` is `CUSTOM` and whose
//! `Authorizer` names a Lambda function.
//!
//! THE RULE UNDER TEST: such a method is decided by that function's RETURN VALUE — the IAM
//! policy it returned, applied to the ARN of the method the request addressed — and the
//! result is HELD on the stage for the authorizer's own `authorizerResultTtlInSeconds`,
//! keyed by the value the request carried for its `identitySource`, until
//! `FlushStageAuthorizersCache` drops it. An allowed call runs the method's integration and
//! is answered with what the integration produced; a refused one earns the `403` API Gateway
//! refuses it with; a function that produced no policy at all earns the `500` envelope AWS
//! reserves for an authorizer that errors.
//!
//! The rows span what the rule generalises over:
//!
//! * the DECISION the returned policy takes about the invoked method — an `Allow` naming it,
//!   an explicit `Deny`, and an `Allow` naming another method of the same api;
//! * each way a call earns no decision — an identity source the request does not carry (the
//!   function is not invoked at all), a function that fails with the `Unauthorized` error
//!   API Gateway documents a refusal by, a function that returns no authorizer output, and
//!   an authorizer the function's own resource policy grants no invocation of;
//! * both Lambda authorizer TYPES, `TOKEN` and `REQUEST`, which differ in the event the
//!   function is handed — the rows read the `type` member back out of the authorizer's
//!   published context, so each row asserts which event its function saw;
//! * the CACHING the TTL decides: the same identity twice within the TTL, a second identity,
//!   a flush of the stage's authorizer cache, and an authorizer whose TTL is zero.
//!
//! What makes an invocation OBSERVABLE: the authorizer publishes a per-invocation value in
//! the `context` map of its result, and the method's integration response template reads it
//! back through `$context.authorizer`. A request answered from a HELD result carries the
//! value the invocation that filled it produced; one that invoked the function again carries
//! a new one. So a cache hit is told from a re-invocation by what the client reads, not by
//! anything this suite knows about the store.
//!
//! Only a cross-service suite can pin this: the authorizer and its grant are AWS Lambda's,
//! and the api, the stage, the authorizer record and the method are API Gateway's.

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// The function every authorizer of the api names.
const FUNCTION: &str = "authorizer-handler";

/// The role a created function names. No IAM store is mounted here, so it is a value the
/// create carries rather than a record this suite relies on.
const ROLE: &str = "arn:aws:iam::000000000000:role/lambda-exec";

/// The handler the deployment package declares.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long this suite waits for Lambda's own clock-owed transition (a created function
/// becoming `Active`), and how often it looks.
const WAIT: Duration = Duration::from_secs(20);
const LOOK: Duration = Duration::from_millis(100);

/// The header every authorizer below declares as its identity source.
const IDENTITY_HEADER: &str = "authorization";

/// `Authorizer$authorizerResultTtlInSeconds` the cached rows run under: long enough that the
/// rows asserting reuse are not racing it.
const HELD_TTL: u32 = 300;

/// The status and body an uncustomized `UNAUTHORIZED` gateway response carries, which is
/// what a call the authorizer was not even invoked for is answered with.
const UNAUTHORIZED: (u16, &str) = (401, r#"{"message":"Unauthorized"}"#);

/// The status and body an authorizer that produced no policy earns: the envelope AWS
/// reserves for an authorizer that errors, times out or returns a malformed policy.
const AUTHORIZER_FAILED: (u16, &str) = (500, r#"{"message":"Internal server error"}"#);

/// The sentence a call the returned policy explicitly DENIES is refused with.
const EXPLICIT_DENY: &str = "User is not authorized to access this resource with an explicit deny";

/// The sentence a call no statement of the returned policy allows is refused with.
const NOT_AUTHORIZED: &str = "User is not authorized to access this resource";

/// The ONE authorizer every row is decided by: it reads the token out of whichever event it
/// was handed, chooses what to return from that token, and publishes a value that is
/// different on every invocation — which is what tells a held result from a fresh one.
const HANDLER_SOURCE: &str = r#"
import time


def lambda_handler(event, context):
    kind = event["type"]
    token = event.get("authorizationToken")
    if token is None:
        headers = event.get("headers") or {}
        token = headers.get("Authorization") or ""
    if token == "raise-unauthorized":
        raise Exception("Unauthorized")
    if token == "raise-other":
        raise Exception("the authorizer itself failed")
    if token == "malformed":
        return {"what": "this is not an authorizer output"}
    resource = event["methodArn"]
    effect = "Allow"
    if token.startswith("deny"):
        effect = "Deny"
    if token.startswith("other"):
        resource = resource.rsplit("/", 1)[0] + "/elsewhere"
    return {
        "principalId": "principal-" + token,
        "policyDocument": {
            "Version": "2012-10-17",
            "Statement": [
                {"Action": "execute-api:Invoke", "Effect": effect, "Resource": resource}
            ],
        },
        "context": {"ran": str(time.time_ns()), "kind": kind, "token": token},
    }
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    /// The body as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is not JSON at all.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The text member `name` of a JSON body, empty when it carries none.
    fn member(&self, name: &str) -> String {
        self.json()
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The two services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: the control-plane calls carry a credential
    // scope so the router can name the target service, and the invokes carry no credential at
    // all, as a client of a published invoke URL sends none.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature itself is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answered {
        status: parts.status.as_u16(),
        body,
    }
}

/// One API Gateway control-plane call that has to be accepted with `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await;
    assert_eq!(
        answered.status, expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One AWS Lambda call over its `restJson1` wire, answering its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answered = dispatch(
        router,
        method,
        &format!("lambda.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("lambda")),
        ],
        Bytes::from(payload),
    )
    .await;
    (answered.status, answered.json())
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com`, which is the endpoint
/// `RestApi$disableExecuteApiEndpoint` documents — carrying `headers` and no credential, as
/// a client of a custom-authorized method sends none.
async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Answered {
    dispatch(
        router,
        "GET",
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}{path}"),
        headers,
        Bytes::new(),
    )
    .await
}

/// The same request carrying the identity source every authorizer below declares.
async fn invoke_as(router: &MultiServiceRouter, api: &str, path: &str, token: &str) -> Answered {
    invoke(router, api, path, &[(IDENTITY_HEADER, token)]).await
}

/// A .zip deployment package holding `source` as the module the handler names, written in
/// memory the way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
async fn wait_until_active(router: &MultiServiceRouter) {
    let looks = WAIT.as_millis() / LOOK.as_millis().max(1);
    let mut last = Value::Null;
    for _ in 0..looks {
        let (_, configuration) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{FUNCTION}/configuration"),
            None,
        )
        .await;
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        last = configuration;
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup CreateFunction: the function never became Active, last read {last}");
}

/// The api every row is served by, with its four custom-authorized methods.
struct Deployed {
    /// The api id.
    api: String,
}

/// One method of the api, by the path it sits on: which authorizer decides it, and whether
/// the function's own resource policy grants that authorizer an invocation.
struct Protected {
    /// The path within the stage.
    path: &'static str,
    /// `Authorizer$type`.
    kind: &'static str,
    /// `Authorizer$authorizerResultTtlInSeconds`.
    ttl: u32,
    /// Whether an `AddPermission` names this authorizer as the invoking resource.
    granted: bool,
}

/// The methods the api carries, each on its own path so one deployment serves every row.
const PROTECTED: &[Protected] = &[
    // The `TOKEN` authorizer every decision row is answered by, holding its results.
    Protected {
        path: "/secure",
        kind: "TOKEN",
        ttl: HELD_TTL,
        granted: true,
    },
    // The same function reached as a `REQUEST` authorizer, which sees the whole request.
    Protected {
        path: "/request",
        kind: "REQUEST",
        ttl: HELD_TTL,
        granted: true,
    },
    // "If it equals 0, authorization caching is disabled": every call invokes the function.
    Protected {
        path: "/uncached",
        kind: "TOKEN",
        ttl: 0,
        granted: true,
    },
    // An authorizer the function's resource policy grants no invocation of.
    Protected {
        path: "/ungranted",
        kind: "TOKEN",
        ttl: HELD_TTL,
        granted: false,
    },
];

/// Stand the function and the api up, and deploy the api to [`STAGE`].
async fn deployed(router: &MultiServiceRouter) -> Deployed {
    let (status, function) = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected HTTP 201, got {status} with {function}"
    );
    let function_arn = member("CreateFunction", &function, "FunctionArn");

    let api = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "custom-authorizer" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &api, "id");
    let resources = apigw(
        router,
        "GetResources",
        "GET",
        &format!("/restapis/{api}/resources"),
        None,
        200,
    )
    .await;
    let root = resources
        .get("item")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .map(|item| member("GetResources", item, "id"))
        .expect("the api's root resource");

    for protected in PROTECTED {
        let part = protected.path.trim_start_matches('/');
        let authorizer = apigw(
            router,
            "CreateAuthorizer",
            "POST",
            &format!("/restapis/{api}/authorizers"),
            Some(json!({
                "name": format!("{part}-authorizer"),
                "type": protected.kind,
                "authorizerUri": format!(
                    "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                     {function_arn}/invocations"
                ),
                "identitySource": format!("method.request.header.{IDENTITY_HEADER}"),
                "authorizerResultTtlInSeconds": protected.ttl,
            })),
            201,
        )
        .await;
        let authorizer = member("CreateAuthorizer", &authorizer, "id");
        if protected.granted {
            // "Grants a principal permission to use a function" (`AddPermission`), scoped by
            // `SourceArn` to the AUTHORIZER whose function is invoked — which is what the
            // ungranted row above has none of.
            let (status, granted) = lambda(
                router,
                "POST",
                &format!("/2015-03-31/functions/{FUNCTION}/policy"),
                Some(json!({
                    "StatementId": format!("apigateway-invoke-{part}"),
                    "Action": "lambda:InvokeFunction",
                    "Principal": "apigateway.amazonaws.com",
                    "SourceArn": format!(
                        "arn:aws:execute-api:{REGION}:{account}:{api}/authorizers/{authorizer}",
                        account = engine::emulator_account_id(),
                    ),
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "setup AddPermission({part}): expected HTTP 201, got {status} with {granted}"
            );
        }
        let resource = apigw(
            router,
            "CreateResource",
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": part })),
            201,
        )
        .await;
        let resource = member("CreateResource", &resource, "id");
        apigw(
            router,
            "PutMethod",
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET"),
            Some(json!({ "authorizationType": "CUSTOM", "authorizerId": authorizer })),
            201,
        )
        .await;
        // A `MOCK` integration, so what an ALLOWED call is answered with is the api's own
        // records and nothing else: the decision is the only thing the rows vary.
        apigw(
            router,
            "PutIntegration",
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/integration"),
            Some(json!({
                "type": "MOCK",
                "requestTemplates": { "application/json": r#"{"statusCode": 200}"# },
            })),
            201,
        )
        .await;
        apigw(
            router,
            "PutMethodResponse",
            "PUT",
            &format!("/restapis/{api}/resources/{resource}/methods/GET/responses/200"),
            Some(json!({})),
            201,
        )
        .await;
        // The response template reads back what the AUTHORIZER published for this call,
        // which is how a row tells which invocation decided it.
        apigw(
            router,
            "PutIntegrationResponse",
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource}/methods/GET/integration/responses/200"
            ),
            Some(json!({
                "responseTemplates": {
                    "application/json": "{\"principal\":\"$context.authorizer.principalId\",\
                                          \"ran\":\"$context.authorizer.ran\",\
                                          \"kind\":\"$context.authorizer.kind\",\
                                          \"token\":\"$context.authorizer.token\"}",
                },
            })),
            201,
        )
        .await;
    }

    let deployment = apigw(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "description": "the snapshot the stage serves" })),
        201,
    )
    .await;
    let deployment = member("CreateDeployment", &deployment, "id");
    apigw(
        router,
        "CreateStage",
        "POST",
        &format!("/restapis/{api}/stages"),
        Some(json!({ "stageName": STAGE, "deploymentId": deployment })),
        201,
    )
    .await;

    wait_until_active(router).await;
    Deployed { api }
}

/// RULE: the returned policy decides the call — an `Allow` naming the invoked method runs
/// the integration, an explicit `Deny` and an `Allow` naming another method are each refused
/// `403` with the sentence API Gateway refuses them with, and a call that earns no policy at
/// all is answered the way AWS answers an authorizer that produced none.
#[tokio::test]
async fn the_authorizers_returned_policy_decides_the_invoked_method() {
    let router = emulator();
    let deployed = deployed(&router).await;
    let api = deployed.api.as_str();

    // Rows differ along what the function returns for the token presented, which is the
    // whole of the decision.
    struct Row {
        case: &'static str,
        /// The identity the request presents, or `None` to present none at all.
        token: Option<&'static str>,
        status: u16,
        /// The body the caller must be served, for a row whose body is fixed text.
        body: Option<&'static str>,
        /// The `Message` an authorization refusal states, for a row that is refused.
        refused: Option<&'static str>,
    }
    let rows: &[Row] = &[
        Row {
            case: "an Allow naming the invoked method",
            token: Some("allow-me"),
            status: 200,
            body: None,
            refused: None,
        },
        Row {
            case: "an explicit Deny",
            token: Some("deny-me"),
            status: 403,
            body: None,
            refused: Some(EXPLICIT_DENY),
        },
        Row {
            case: "an Allow naming another method of the api",
            token: Some("other-method"),
            status: 403,
            body: None,
            refused: Some(NOT_AUTHORIZED),
        },
        Row {
            case: "no identity source at all: the function is not invoked",
            token: None,
            status: UNAUTHORIZED.0,
            body: Some(UNAUTHORIZED.1),
            refused: None,
        },
        Row {
            case: "a function failing with the Unauthorized error a refusal is worded by",
            token: Some("raise-unauthorized"),
            status: UNAUTHORIZED.0,
            body: Some(UNAUTHORIZED.1),
            refused: None,
        },
        Row {
            case: "a function that failed some other way",
            token: Some("raise-other"),
            status: AUTHORIZER_FAILED.0,
            body: Some(AUTHORIZER_FAILED.1),
            refused: None,
        },
        Row {
            case: "a function returning no authorizer output",
            token: Some("malformed"),
            status: AUTHORIZER_FAILED.0,
            body: Some(AUTHORIZER_FAILED.1),
            refused: None,
        },
    ];
    for row in rows {
        let answered = match row.token {
            Some(token) => invoke_as(&router, api, "/secure", token).await,
            None => invoke(&router, api, "/secure", &[]).await,
        };
        assert_eq!(
            answered.status,
            row.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            row.case,
            row.status,
            answered.status,
            answered.text()
        );
        if let Some(body) = row.body {
            assert_eq!(
                answered.text(),
                body,
                "case {:?}: expected the body {body:?}, got {:?}",
                row.case,
                answered.text()
            );
        }
        if let Some(refused) = row.refused {
            assert_eq!(
                answered.member("Message"),
                refused,
                "case {:?}: expected the refusal {refused:?}, got {:?}",
                row.case,
                answered.text()
            );
        }
        if row.status == 200 {
            // An allowed call ran the integration, and what the authorizer published for it
            // is what the integration's own template read.
            assert_eq!(
                answered.member("principal"),
                format!("principal-{}", row.token.unwrap_or_default()),
                "case {:?}: expected the published principal, got {:?}",
                row.case,
                answered.text()
            );
            assert!(
                !answered.member("ran").is_empty(),
                "case {:?}: expected the published context to reach the integration, got {:?}",
                row.case,
                answered.text()
            );
        }
    }

    // An authorizer the function's resource policy grants no invocation of is one this api
    // cannot obtain a decision from, however well-formed the request is.
    let ungranted = invoke_as(&router, api, "/ungranted", "allow-me").await;
    assert_eq!(
        ungranted.status,
        AUTHORIZER_FAILED.0,
        "case \"an authorizer the function grants no invocation of\": expected HTTP {}, got {} \
         with {:?}",
        AUTHORIZER_FAILED.0,
        ungranted.status,
        ungranted.text()
    );
}

/// RULE: the result is HELD for `authorizerResultTtlInSeconds`, keyed by the identity source
/// value — the same identity within the window reuses it with the function not invoked
/// again, another identity earns its own invocation, `FlushStageAuthorizersCache` drops what
/// the stage holds, and an authorizer whose TTL is zero holds nothing.
#[tokio::test]
async fn a_held_result_is_reused_per_identity_until_the_ttl_or_a_flush_ends_it() {
    let router = emulator();
    let deployed = deployed(&router).await;
    let api = deployed.api.as_str();

    let answered = invoke_as(&router, api, "/secure", "allow-first").await;
    assert_eq!(
        answered.status,
        200,
        "case \"the first call of an identity\": expected HTTP 200, got {} with {:?}",
        answered.status,
        answered.text()
    );
    let first = answered.member("ran");
    assert!(
        !first.is_empty(),
        "case \"the first call of an identity\": expected a published value, got {:?}",
        answered.text()
    );

    // The same identity again, inside the TTL: the HELD result decides it, so the value the
    // first invocation published is what this call carries too.
    let again = invoke_as(&router, api, "/secure", "allow-first").await;
    assert_eq!(
        again.member("ran"),
        first,
        "case \"the same identity within the TTL\": expected the held result to decide it, got \
         {:?} after {first:?}",
        again.text()
    );

    // Another identity is another entry: its own invocation, and its own published value.
    let other = invoke_as(&router, api, "/secure", "allow-second").await;
    assert_eq!(
        other.status,
        200,
        "case \"another identity\": expected HTTP 200, got {} with {:?}",
        other.status,
        other.text()
    );
    assert_ne!(
        other.member("ran"),
        first,
        "case \"another identity\": expected its own invocation, got {:?}",
        other.text()
    );
    assert_eq!(
        other.member("principal"),
        "principal-allow-second",
        "case \"another identity\": expected its own principal, got {:?}",
        other.text()
    );

    // The same identity, on the method whose authorizer caches nothing: every call invokes
    // the function, so no two of them publish the same value.
    let uncached_first = invoke_as(&router, api, "/uncached", "allow-first").await;
    let uncached_again = invoke_as(&router, api, "/uncached", "allow-first").await;
    assert_eq!(
        uncached_first.status,
        200,
        "case \"an authorizer whose TTL is zero\": expected HTTP 200, got {} with {:?}",
        uncached_first.status,
        uncached_first.text()
    );
    assert_ne!(
        uncached_first.member("ran"),
        uncached_again.member("ran"),
        "case \"an authorizer whose TTL is zero\": expected each call to invoke the function, \
         got {:?} twice",
        uncached_first.text()
    );

    // `FlushStageAuthorizersCache` drops what the stage holds, so the next call of the first
    // identity invokes the function again.
    apigw(
        &router,
        "FlushStageAuthorizersCache",
        "DELETE",
        &format!("/restapis/{api}/stages/{STAGE}/cache/authorizers"),
        None,
        202,
    )
    .await;
    let flushed = invoke_as(&router, api, "/secure", "allow-first").await;
    assert_eq!(
        flushed.status,
        200,
        "case \"after a flush\": expected HTTP 200, got {} with {:?}",
        flushed.status,
        flushed.text()
    );
    assert_ne!(
        flushed.member("ran"),
        first,
        "case \"after a flush\": expected the authorizer to be invoked again, got {:?}",
        flushed.text()
    );
}

/// RULE: the event the function is handed is the one its authorizer TYPE documents — a
/// `TOKEN` authorizer's token, and a `REQUEST` authorizer's whole method request — and the
/// decision is taken from what it returned either way.
#[tokio::test]
async fn each_authorizer_type_is_handed_the_event_it_documents() {
    let router = emulator();
    let deployed = deployed(&router).await;
    let api = deployed.api.as_str();

    // Rows differ along the authorizer type the method's authorizer carries.
    let rows: &[(&str, &str, &str)] = &[
        ("a TOKEN authorizer", "/secure", "TOKEN"),
        ("a REQUEST authorizer", "/request", "REQUEST"),
    ];
    for (case, path, kind) in rows {
        let answered = invoke_as(&router, api, path, "allow-me").await;
        assert_eq!(
            answered.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.member("kind"),
            *kind,
            "case {case:?}: expected the function to be handed a {kind} event, got {:?}",
            answered.text()
        );
        // Either event names the identity the request presented, which is what the decision
        // is taken about.
        assert_eq!(
            answered.member("token"),
            "allow-me",
            "case {case:?}: expected the presented identity to reach the function, got {:?}",
            answered.text()
        );
    }
}
