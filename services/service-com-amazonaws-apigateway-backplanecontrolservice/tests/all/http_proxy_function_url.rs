// SPDX-License-Identifier: CC0-1.0

//! A deployed method whose integration is `HTTP_PROXY` naming an endpoint of ANOTHER
//! service of this emulator — a Lambda function URL.
//!
//! RULE: `IntegrationType` gives `HTTP_PROXY` as "for integrating the API method request
//! with an HTTP endpoint, including a third party API, with the client request passed
//! through as-is", so such a method is served by PASSING THE REQUEST ON to the endpoint the
//! `uri` names and answering with that endpoint's own response — its status, its headers and
//! its body, mapped by nothing — whatever that status is. An endpoint NOTHING in this
//! emulator serves answers nothing, so the call is the integration this stage cannot perform
//! instead: `500` through the api's own `API_CONFIGURATION_ERROR` gateway response.
//!
//! The rows differ along what the rule generalises over: the status the backend answers
//! (a success, a `404` and a `500` the handler itself returns, and the `403` the function
//! URL's own admission produces for a URL whose function grants the invocation to nobody),
//! the path and query string the request carries onward, and a `uri` naming an endpoint
//! nothing serves at all. Both services are mounted on this suite's own engine, and the
//! backend's answer is read back through what the HANDLER reported about the request it was
//! handed, so a row that answered without the request reaching the function fails here.

use std::io::Write as _;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// The function whose URL the granted integrations name, and the one whose URL is granted to
/// nobody.
const FUNCTION: &str = "apigw-url-backend";
const UNGRANTED_FUNCTION: &str = "apigw-url-ungranted";

/// The module and attribute the deployment packages carry their handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long one invocation is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
const POLL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The header the client sends and the handler reports back, which is what shows the
/// caller's own headers reached the backend.
const SENT_HEADER: &str = "x-sent";
const SENT_VALUE: &str = "from-the-client";

/// The header the handler answers with, which is what shows the backend's own headers reach
/// the client.
const BACKEND_HEADER: &str = "x-backend";
const BACKEND_VALUE: &str = "seen";

/// The handler behind the function URL: it answers the status the query string names and
/// reports back what it was handed, in the payload-format-2.0 members a function URL event
/// carries (`rawPath`, `rawQueryString`, `requestContext.http.method`, `headers`).
const HANDLER_SOURCE: &str = r#"
import json


def lambda_handler(event, context):
    query = event.get("queryStringParameters") or {}
    status = int(query.get("status", "200"))
    context_http = (event.get("requestContext") or {}).get("http") or {}
    return {
        "statusCode": status,
        "headers": {"content-type": "application/json", "x-backend": "seen"},
        "body": json.dumps(
            {
                "rawPath": event.get("rawPath"),
                "rawQueryString": event.get("rawQueryString"),
                "method": context_http.get("method"),
                "sent": (event.get("headers") or {}).get("x-sent"),
                "apiId": (event.get("headers") or {}).get("x-amzn-apigateway-api-id"),
            }
        ),
    }
"#;

/// One answered request: its status, its headers and its body verbatim.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: Bytes,
}

impl Answer {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// API Gateway and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts them:
/// the request the stage passes on is dispatched back into this same router, so the function
/// URL it reaches is the one Lambda published here.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature is never checked here, as in every other suite of this crate; the
    // credential scope is what routes a request.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
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
) -> Answer {
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
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One API Gateway control-plane call that has to answer `expected`.
async fn apigw(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answer = dispatch(
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
        answer.status, expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One Lambda `restJson1` call that has to answer `expected`.
async fn lambda(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let answer = dispatch(
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
    assert_eq!(
        answer.status, expected,
        "setup {case:?}: expected HTTP {expected} from Lambda, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// A .zip deployment package holding `source` as `lambda_function.py`.
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

/// Create `name` from a package running [`HANDLER_SOURCE`] and wait until Lambda reports it
/// `Active`.
async fn given_function(router: &MultiServiceRouter, name: &str) {
    lambda(
        router,
        &format!("CreateFunction({name})"),
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
        201,
    )
    .await;
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            &format!("GetFunctionConfiguration({name})"),
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
            200,
        )
        .await;
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: Lambda never reported {name} Active");
}

/// `CreateFunctionUrlConfig` on `function`, answering the endpoint Lambda minted for it.
///
/// `granted` grants the public the function-URL invocation, which is what makes an
/// `AuthType=NONE` endpoint public — a URL whose function grants it to nobody admits no
/// anonymous caller.
async fn given_function_url(
    router: &MultiServiceRouter,
    function: &str,
    granted: bool,
) -> String {
    let created = lambda(
        router,
        &format!("CreateFunctionUrlConfig({function})"),
        "POST",
        &format!("/2021-10-31/functions/{function}/url"),
        Some(json!({ "AuthType": "NONE" })),
        201,
    )
    .await;
    if granted {
        lambda(
            router,
            &format!("AddPermission({function})"),
            "POST",
            &format!("/2015-03-31/functions/{function}/policy"),
            Some(json!({
                "StatementId": "public-url",
                "Action": "lambda:InvokeFunctionUrl",
                "Principal": "*",
                "FunctionUrlAuthType": "NONE",
            })),
            201,
        )
        .await;
    }
    member(
        &format!("CreateFunctionUrlConfig({function})"),
        &created,
        "FunctionUrl",
    )
}

/// One resource of the api carrying a `verb` method whose `HTTP_PROXY` integration passes
/// the request on to `uri`, with `request_parameters` mapped onto the integration request.
async fn given_proxy_resource(
    router: &MultiServiceRouter,
    api: &str,
    root: &str,
    path_part: &str,
    verb: &str,
    uri: &str,
    request_parameters: Value,
) {
    let created = apigw(
        router,
        &format!("CreateResource({path_part})"),
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": path_part })),
        201,
    )
    .await;
    let resource = member(&format!("CreateResource({path_part})"), &created, "id");
    apigw(
        router,
        &format!("PutMethod({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/{verb}"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        &format!("PutIntegration({path_part})"),
        "PUT",
        &format!("/restapis/{api}/resources/{resource}/methods/{verb}/integration"),
        Some(json!({
            "type": "HTTP_PROXY",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
            "httpMethod": verb,
            "uri": uri,
            "requestParameters": request_parameters,
        })),
        201,
    )
    .await;
}

/// One row: the request the client makes and the answer the backend earns it.
struct Row {
    case: &'static str,
    /// The path within the stage, query string included.
    path: &'static str,
    /// The status the client must be answered with.
    status: u16,
    /// What the HANDLER must report about the request it was handed: the path and the query
    /// string it arrived with, or `None` for a row whose request reaches no handler.
    handled: Option<(&'static str, &'static str)>,
}

#[tokio::test]
async fn an_http_proxy_method_answers_with_the_function_urls_own_response() {
    let router = emulator();

    given_function(&router, FUNCTION).await;
    given_function(&router, UNGRANTED_FUNCTION).await;
    let endpoint = given_function_url(&router, FUNCTION, true).await;
    let ungranted = given_function_url(&router, UNGRANTED_FUNCTION, false).await;
    assert!(
        endpoint.contains(".lambda-url."),
        "setup CreateFunctionUrlConfig: expected a function URL endpoint, got {endpoint:?}"
    );

    let created = apigw(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "http-proxy-function-url" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    // The endpoint ends in `/`, the path a function URL's own root is addressed by, so the
    // onward path is what follows it.
    let base = endpoint.trim_end_matches('/').to_string();
    given_proxy_resource(
        &router,
        &api,
        &root,
        "echo",
        "GET",
        &format!("{base}/echo"),
        json!({}),
    )
    .await;
    given_proxy_resource(
        &router,
        &api,
        &root,
        "denied",
        "GET",
        &format!("{}/echo", ungranted.trim_end_matches('/')),
        json!({}),
    )
    .await;
    // An endpoint no service of this emulator claims: nothing answers for it, so there is no
    // backend response to pass through.
    given_proxy_resource(
        &router,
        &api,
        &root,
        "nowhere",
        "GET",
        "https://nothing.example.test/echo",
        json!({}),
    )
    .await;
    // The catch-all resource whose greedy path parameter addresses the backend: the `uri`'s
    // own `{proxy}` placeholder is the integration request's path parameter, mapped from the
    // method request's own, and a mapped query-string parameter travels beside the caller's.
    given_proxy_resource(
        &router,
        &api,
        &root,
        "{proxy+}",
        "ANY",
        &format!("{base}/{{proxy}}"),
        json!({
            "integration.request.path.proxy": "method.request.path.proxy",
            "integration.request.querystring.added": "'yes'",
        }),
    )
    .await;
    // The same placeholder with nothing mapped onto it: the address is unknown, so no
    // request is passed on to a guessed-at one.
    given_proxy_resource(
        &router,
        &api,
        &root,
        "unmapped",
        "GET",
        &format!("{base}/{{proxy}}"),
        json!({}),
    )
    .await;
    apigw(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    let rows = [
        Row {
            case: "the handler's own 200, with the path and query string passed on",
            path: "/echo?status=200&tag=a",
            status: 200,
            handled: Some(("/echo", "status=200&tag=a")),
        },
        Row {
            case: "the handler's own 404, passed through as the backend's status",
            path: "/echo?status=404",
            status: 404,
            handled: Some(("/echo", "status=404")),
        },
        Row {
            case: "the handler's own 500, passed through as the backend's status",
            path: "/echo?status=500",
            status: 500,
            handled: Some(("/echo", "status=500")),
        },
        Row {
            case: "a function URL whose function grants the invocation to nobody: its own 403",
            path: "/denied?status=200",
            status: 403,
            handled: None,
        },
        Row {
            case: "a uri naming an endpoint nothing in this emulator serves",
            path: "/nowhere?status=200",
            status: 500,
            handled: None,
        },
        Row {
            case: "a catch-all resource whose greedy path parameter addresses the backend",
            path: "/deep/one/two?status=200",
            status: 200,
            handled: Some(("/deep/one/two", "added=yes&status=200")),
        },
        Row {
            case: "the same path, with the backend's own 404 passed through",
            path: "/deep/three?status=404",
            status: 404,
            handled: Some(("/deep/three", "added=yes&status=404")),
        },
        Row {
            case: "a uri placeholder the integration maps no path parameter onto",
            path: "/unmapped?status=200",
            status: 500,
            handled: None,
        },
    ];

    for row in &rows {
        let answered = dispatch(
            &router,
            "GET",
            &format!("{api}.execute-api.{REGION}.amazonaws.com"),
            &format!("/{STAGE}{}", row.path),
            &[(SENT_HEADER, SENT_VALUE)],
            Bytes::new(),
        )
        .await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            row.case,
            row.status,
            answered.status,
            answered.text()
        );
        let Some((raw_path, raw_query)) = row.handled else {
            assert!(
                answered.header(BACKEND_HEADER).is_none(),
                "case {:?}: expected no backend header, got {:?}",
                row.case,
                answered.header(BACKEND_HEADER)
            );
            continue;
        };
        assert_eq!(
            answered.header(BACKEND_HEADER).as_deref(),
            Some(BACKEND_VALUE),
            "case {:?}: expected the backend's own header, got {:?}",
            row.case,
            answered.header(BACKEND_HEADER)
        );
        // The function URL reports the request IT answered in `x-amzn-requestid`, which is
        // also the header this api reports its OWN request in: the backend's value reaches
        // the client remapped, and the name itself carries the api's.
        assert!(
            answered
                .header("x-amzn-remapped-x-amzn-requestid")
                .is_some_and(|value| !value.is_empty()),
            "case {:?}: expected the backend's request id remapped, got {:?}",
            row.case,
            answered.headers
        );
        assert!(
            answered
                .header("x-amzn-requestid")
                .is_some_and(|value| !value.is_empty()),
            "case {:?}: expected the api's own request id, got {:?}",
            row.case,
            answered.headers
        );
        assert_eq!(
            answered.json(),
            json!({
                "rawPath": raw_path,
                "rawQueryString": raw_query,
                "method": "GET",
                "sent": SENT_VALUE,
                // The api names ITSELF to the endpoint it passes the request on to, in the
                // header API Gateway adds to an integration request.
                "apiId": api.clone(),
            }),
            "case {:?}: expected the handler's report of the request it was handed, got {:?}",
            row.case,
            answered.text()
        );
    }
}
