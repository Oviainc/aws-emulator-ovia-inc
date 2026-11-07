// SPDX-License-Identifier: CC0-1.0

//! What an `AWS_PROXY` method's response BODY is when the function declared it
//! base64-encoded.
//!
//! RULE: `RestApi$binaryMediaTypes` is "the list of binary media types supported by the
//! RestApi. By default, the RestApi supports only UTF-8-encoded text payloads"
//! (`smithy_query(query="doc", shape="RestApi", member="binaryMediaTypes")`), so a proxy
//! response whose `isBase64Encoded` is set reaches the client as the BYTES its `body` names
//! only where the response's own media type is one the api declares binary; on an api that
//! declares none of them, the base64 TEXT is the payload and travels unchanged.
//!
//! The rows differ along what the rule generalises over: which media types the api declares
//! binary (none, one exact type, a subtype wildcard, the whole wildcard, another type than
//! the response's), the media type the response itself declares (including one carrying a
//! `charset` parameter), and whether the function declared the body encoded at all. Each row
//! stands its own api up and drives the real wire path — the stage invoke URL through the
//! Lambda function the integration names — so what is asserted is the bytes a client holds.
//!
//! [`emulator`], [`dispatch`], [`apigw`], [`lambda`] and the helpers beside them are this
//! suite's Lambda-backed wire harness, reused by the sibling `proxy_event_repeated_names`
//! suite rather than stood up a second way.

use std::io::Write as _;
use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for and the integration ARNs name.
pub(crate) const REGION: &str = "us-east-1";

/// The stage each api is deployed to.
pub(crate) const STAGE: &str = "live";

/// The function every integration names.
const FUNCTION: &str = "apigw-binary-payload";

/// The module and attribute the deployment package carries its handler as.
pub(crate) const HANDLER: &str = "lambda_function.lambda_handler";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
pub(crate) const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// How long one invocation is given, as the function's own `Timeout`.
pub(crate) const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
const POLL: Duration = Duration::from_millis(25);
const POLLS: usize = 400;

/// The bytes the handler answers with: a PNG signature and four more bytes, which is a
/// payload no UTF-8 text carries.
const BLOB: [u8; 12] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01, 0x02, 0x03];

/// The body the handler answers with when it declares the body NOT encoded.
const PLAIN: &str = "plain-text-body";

/// The handler every row runs: the query string says which media type the response declares
/// and whether its body is declared base64-encoded, so one package serves every row.
const HANDLER_SOURCE: &str = r#"
import base64

BLOB = bytes([0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x01, 0x02, 0x03])


def lambda_handler(event, context):
    query = event.get("queryStringParameters") or {}
    media = query.get("media", "application/json")
    if query.get("encoded") == "no":
        return {
            "statusCode": 200,
            "headers": {"Content-Type": media},
            "body": "plain-text-body",
        }
    return {
        "statusCode": 200,
        "headers": {"Content-Type": media},
        "body": base64.b64encode(BLOB).decode("ascii"),
        "isBase64Encoded": True,
    }
"#;

/// One answered request: its status, its headers and its body verbatim.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) headers: http::HeaderMap,
    pub(crate) body: Bytes,
}

impl Answer {
    pub(crate) fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub(crate) fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// API Gateway and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts them.
pub(crate) fn emulator() -> MultiServiceRouter {
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
pub(crate) fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
pub(crate) async fn dispatch(
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
pub(crate) async fn apigw(
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

/// One Lambda `restJson1` call.
pub(crate) async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
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
    .await
}

/// The member `name` of a JSON object, as text.
pub(crate) fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// A .zip deployment package holding `source` as `lambda_function.py`.
pub(crate) fn package(source: &str) -> Vec<u8> {
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

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`.
pub(crate) async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported.json().get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: Lambda never reported {name} Active");
}

/// One api declaring `binary_media_types` binary, with an `AWS_PROXY` method on its root
/// resource behind `function`, deployed to [`STAGE`]. Answers the api id.
async fn given_api(
    router: &MultiServiceRouter,
    name: &str,
    binary_media_types: &[&str],
    function: &str,
) -> String {
    let created = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": name, "binaryMediaTypes": binary_media_types })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    apigw(
        router,
        "PutMethod",
        "PUT",
        &format!("/restapis/{api}/resources/{root}/methods/GET"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        "PutIntegration",
        "PUT",
        &format!("/restapis/{api}/resources/{root}/methods/GET/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                 {function}/invocations"
            ),
        })),
        201,
    )
    .await;
    apigw(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;
    api
}

/// One row: what the api declares binary, what the response declares, and what the client
/// must therefore hold.
struct Row {
    case: &'static str,
    /// The api's own `binaryMediaTypes`.
    declared: &'static [&'static str],
    /// The media type the function's response declares.
    media: &'static str,
    /// Whether the function declares its body base64-encoded.
    encoded: bool,
    /// Whether the client must hold the DECODED bytes rather than the base64 text.
    decoded: bool,
}

#[tokio::test]
async fn a_base64_proxy_body_is_decoded_only_for_a_media_type_the_api_declares_binary() {
    let router = emulator();

    let function = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        function.status,
        201,
        "setup CreateFunction: expected HTTP 201 from Lambda, got {} with {:?}",
        function.status,
        function.text()
    );
    let function_arn = member("CreateFunction", &function.json(), "FunctionArn");
    wait_until_active(&router, FUNCTION).await;

    // The grant that admits the invocation. It names no `SourceArn`, so it admits the call
    // from every api below rather than one statement per api.
    let granted = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": "apigw-any-api",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
        })),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "setup AddPermission: expected HTTP 201 from Lambda, got {} with {:?}",
        granted.status,
        granted.text()
    );

    let rows = [
        Row {
            case: "the api declares the response's own media type binary",
            declared: &["image/png"],
            media: "image/png",
            encoded: true,
            decoded: true,
        },
        Row {
            case: "the api declares another media type binary",
            declared: &["image/png"],
            media: "application/octet-stream",
            encoded: true,
            decoded: false,
        },
        Row {
            case: "the api declares no binary media type at all",
            declared: &[],
            media: "image/png",
            encoded: true,
            decoded: false,
        },
        Row {
            case: "the api declares every media type binary",
            declared: &["*/*"],
            media: "application/pdf",
            encoded: true,
            decoded: true,
        },
        Row {
            case: "the api declares the response's whole type binary by subtype wildcard",
            declared: &["image/*"],
            media: "image/jpeg",
            encoded: true,
            decoded: true,
        },
        Row {
            case: "the response's media type carries a charset parameter",
            declared: &["text/plain"],
            media: "text/plain; charset=utf-8",
            encoded: true,
            decoded: true,
        },
        Row {
            case: "a body the function does not declare encoded, on a binary media type",
            declared: &["image/png"],
            media: "image/png",
            encoded: false,
            decoded: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let api = given_api(
            &router,
            &format!("binary-media-types-{index}"),
            row.declared,
            &function_arn,
        )
        .await;
        let query = format!(
            "?media={}&encoded={}",
            percent_encoding::utf8_percent_encode(row.media, percent_encoding::NON_ALPHANUMERIC),
            if row.encoded { "yes" } else { "no" }
        );
        let answered = dispatch(
            &router,
            "GET",
            &format!("{api}.execute-api.{REGION}.amazonaws.com"),
            &format!("/{STAGE}/{query}"),
            &[],
            Bytes::new(),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {:?}: expected HTTP 200 from the stage, got {} with {:?}",
            row.case,
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("content-type").as_deref(),
            Some(row.media),
            "case {:?}: expected the media type the function declared, got {:?}",
            row.case,
            answered.header("content-type")
        );
        let expected: Vec<u8> = if !row.encoded {
            PLAIN.as_bytes().to_vec()
        } else if row.decoded {
            BLOB.to_vec()
        } else {
            base64::engine::general_purpose::STANDARD
                .encode(BLOB)
                .into_bytes()
        };
        assert_eq!(
            answered.body.as_ref(),
            expected.as_slice(),
            "case {:?}: expected {:?}, got {:?}",
            row.case,
            String::from_utf8_lossy(&expected),
            String::from_utf8_lossy(&answered.body)
        );
    }
}
