// SPDX-License-Identifier: CC0-1.0

//! The refusal a running handler's own SDK call reads back, and the class it is thrown as — end
//! to end over the wire, with Lambda and AWS Secrets Manager on ONE engine and a `nodejs*`
//! handler importing the bundled `@aws-sdk/client-secrets-manager` by the name the SDK
//! publishes that service's client package under.
//!
//! Two rules meet here, and the first is why this suite exists at all:
//!
//! * **A service is reached by either spelling of its SDK id.** An SDK client is built from
//!   that id with the words run together (`boto3.client("secretsmanager")`), and the AWS SDK
//!   for JavaScript publishes the same service's client package with them hyphenated
//!   (`@aws-sdk/client-secrets-manager`), whose token is what a handler importing it hands the
//!   runtime. Both spell one service, so a call naming either reaches it; before the fix this
//!   suite accompanies, the hyphenated token was answered "this emulator makes no call to
//!   secrets-manager" and no handler written against that package could reach the service at
//!   all.
//! * **A refusal is the service's own.** What the call throws is an instance of the error class
//!   the service's model names for the shape the answer carried, with that answer's status in
//!   `$metadata` and its message — not a stand-in built by the runtime. The row states no code
//!   of its own: what the handler reported is held to what the SAME request answers on the
//!   wire.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, and the one the handler's client is built
/// with.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `Runtime` the function carries: a managed Node release that bundles the SDK.
const RUNTIME: &str = "nodejs22.x";

/// How long past a due instant this suite waits before calling.
const SLACK: Duration = Duration::from_millis(150);

/// The class every error class of this client package extends, which is what a handler catches
/// to catch any refusal of the service.
const BASE_EXCEPTION: &str = "SecretsManagerServiceException";

/// Lambda and AWS Secrets Manager over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's call is dispatched back into this same router, so the refusal it catches
/// is the one this suite reads off the wire.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// What one Secrets Manager call answered on the wire: its status, the error shape it named in
/// the header, and its parsed body.
async fn secrets(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Option<String>, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("secretsmanager.{REGION}.amazonaws.com"))
        .header("authorization", authorization("secretsmanager"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    let error_type = parts
        .headers
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    (parts.status.as_u16(), error_type, parsed)
}

/// One Lambda `restJson1` call, as its status, headers and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parts.headers, parsed)
}

/// A .zip deployment package holding `source` as `index.mjs` — one ES module, and nothing else:
/// no `node_modules`, so the `@aws-sdk/*` specifier in it is the runtime's to resolve.
fn package(source: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("index.mjs", options)
            .expect("the entry starts");
        writer
            .write_all(source.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// The handler this suite runs: it sends the commands the event lists and reports, per probe,
/// what the SDK gave it — the answer's own members for a call the service served, and for a
/// refusal the class, the name, the message, the status and which of the package's classes the
/// error is an instance of.
///
/// Both probes are made in the ONE run, because what the rules are about is what the client did
/// with one answer, which does not depend on the run it was built in.
const HANDLER_SOURCE: &str = r#"
import {
    SecretsManagerClient,
    CreateSecretCommand,
    GetSecretValueCommand,
    ResourceNotFoundException,
    SecretsManagerServiceException,
} from "@aws-sdk/client-secrets-manager";

const COMMANDS = {
    CreateSecret: CreateSecretCommand,
    GetSecretValue: GetSecretValueCommand,
};

export const handler = async (event) => {
    const client = new SecretsManagerClient({ region: event.region });
    const probes = {};
    for (const probe of event.probes) {
        const Command = COMMANDS[probe.operation];
        try {
            const answered = await client.send(new Command(probe.input));
            probes[probe.case] = {
                threw: false,
                status: answered.$metadata.httpStatusCode,
                members: Object.keys(answered)
                    .filter((name) => name !== "$metadata")
                    .sort(),
            };
        } catch (error) {
            probes[probe.case] = {
                threw: true,
                ctor: error.constructor.name,
                name: error.name,
                message: error.message,
                isResourceNotFound: error instanceof ResourceNotFoundException,
                isServiceException: error instanceof SecretsManagerServiceException,
                isError: error instanceof Error,
                status: error.$metadata === undefined
                    ? null
                    : error.$metadata.httpStatusCode,
            };
        }
    }
    return { probes };
};
"#;

/// The secret the served probe creates.
const SECRET: &str = "reachable-through-the-package-token";

/// RULE: a handler reaches the service through the token its client package is published
/// under, and what a refused call throws is the service's own answer — the class its model
/// names for the shape, carrying that answer's message and status.
#[tokio::test]
async fn a_handler_reaches_the_service_by_its_package_token_and_reads_its_own_refusal() {
    let router = router();
    let name = "sdk-refusal-classes";
    let (status, _headers, created) = lambda(
        &router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": "index.handler",
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    // The two probes: one the service SERVES, which is what proves the hyphenated package token
    // reached it, and one it REFUSES, which is what the class and the message are read from.
    let served = "a call the service serves";
    let refused = "a call the service refuses";
    let (status, headers, payload) = lambda(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        json!({
            "region": REGION,
            "probes": [
                {
                    "case": served,
                    "operation": "CreateSecret",
                    "input": { "Name": SECRET, "SecretString": "held" },
                },
                {
                    "case": refused,
                    "operation": "GetSecretValue",
                    "input": { "SecretId": "no-such-secret-here" },
                },
            ],
        })
        .to_string()
        .as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok());
    assert!(
        signal.is_none(),
        "expected the run to carry no function error, got {signal:?} with {payload}"
    );

    // The served call: the service answered it, so the package token resolved to the service
    // and the answer carries the operation's own output members.
    let answered = &payload["probes"][served];
    assert_eq!(
        answered["threw"],
        json!(false),
        "case {served:?}: expected the call to be served, got {answered}"
    );
    assert_eq!(
        answered["status"].as_u64(),
        Some(200),
        "case {served:?}: expected the SDK to report HTTP 200, got {answered}"
    );
    assert!(
        answered["members"]
            .as_array()
            .is_some_and(|members| members.iter().any(|member| member == "ARN")),
        "case {served:?}: expected the create's own output members, got {answered}"
    );

    // The refused call, held to what the SAME request answers on the wire: the shape it named,
    // its message and its status, so this row states no code and no sentence of its own.
    let (wire_status, wire_type, wire_body) = secrets(
        &router,
        "GetSecretValue",
        &json!({ "SecretId": "no-such-secret-here" }),
    )
    .await;
    let shape = wire_type
        .or_else(|| {
            wire_body
                .get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        !shape.is_empty(),
        "case {refused:?}: expected the wire answer to name an error shape, got {wire_body}"
    );
    let thrown = &payload["probes"][refused];
    assert_eq!(
        thrown["threw"],
        json!(true),
        "case {refused:?}: expected the call to be refused, got {thrown}"
    );
    for (what, reported, expected) in [
        ("the class", thrown["ctor"].as_str(), Some(shape.as_str())),
        ("the name", thrown["name"].as_str(), Some(shape.as_str())),
        (
            "the message",
            thrown["message"].as_str(),
            // The member an AWS JSON answer carries the sentence in, whichever casing this
            // service's answer spells it with.
            wire_body
                .get("message")
                .or_else(|| wire_body.get("Message"))
                .and_then(Value::as_str),
        ),
    ] {
        assert_eq!(
            reported, expected,
            "case {refused:?}: expected {what} to be the wire answer's {expected:?}, got \
             {reported:?} ({thrown})"
        );
    }
    assert_eq!(
        thrown["status"].as_u64(),
        Some(u64::from(wire_status)),
        "case {refused:?}: expected the metadata to carry the answer's status {wire_status}, \
         got {thrown}"
    );
    // The class the model names for that shape, which extends the package's base exception —
    // so a handler catching either catches this refusal, and it is a real `Error`.
    assert_eq!(
        thrown["isResourceNotFound"],
        json!(true),
        "case {refused:?}: expected the class the model names for {shape:?} to catch it, got \
         {thrown}"
    );
    assert_eq!(
        thrown["isServiceException"],
        json!(true),
        "case {refused:?}: expected {BASE_EXCEPTION} to catch it, got {thrown}"
    );
    assert_eq!(
        thrown["isError"],
        json!(true),
        "case {refused:?}: expected a real Error, got {thrown}"
    );
}
