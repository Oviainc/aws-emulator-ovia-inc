// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the HTTP TASK (`arn:aws:states:::http:invoke`): the `Task` state
//! whose unit of work is one HTTP request to an endpoint its own parameters name.
//!
//! The rule under test: an HTTP Task MAKES the request its parameters describe — the
//! `Method` to the `ApiEndpoint`, carrying its `Headers`, its `QueryParameters` and its
//! `RequestBody` under the encoding its `Transform` asks for — through the EventBridge
//! connection `Authentication.ConnectionArn` names, and the task's result is THAT
//! endpoint's own response: the status it answered, the reason phrase for that status, the
//! headers it carried and its body. The connection decides whether the request is made at
//! all: one that resolves to no connection, and one whose `ConnectionState` is not a state
//! in which it holds authorization, fail the task before anything is sent — while the
//! parameters a connection it DOES authorize contributes are merged onto the request, the
//! connection's value winning where both name one key and a value it flagged secret
//! travelling not at all. An endpoint's non-2xx answer fails the task under the error name
//! that status gives it (`States.Http.StatusCode.NNN`).
//!
//! Every row drives both services' own wire protocols through the production
//! `MultiServiceRouter` with Step Functions, Lambda and EventBridge mounted on ONE engine
//! — the shape `core/router/src/lib.rs` mounts them in — against a Lambda function URL
//! whose package RUNS and ECHOES the request it was given, and the connection is created
//! through EventBridge's own API. So a response this emulator made up rather than read off
//! the endpoint fails here, as does a request whose members never reached it.
//!
//! The rows differ along what the rule generalises over: the method, what the request
//! carries (a JSON body, a URL-encoded body, query parameters, headers, a path), which
//! status the endpoint answers, what the connection contributes and what state it is in,
//! and each way the parameters can describe no request at all.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

// The deployment package a Python handler is created from, and the reading of a response
// body: both are this test binary's own, stated once in the suite that first needed them.
use crate::lambda_task_control_flow::{package, parsed};
use crate::state_machine_lifecycle::{string, PAST_THE_EXECUTION_DELAY, REGION, ROLE_ARN};

/// The `Resource` that names Step Functions' own HTTP Task.
const HTTP_INVOKE: &str = "arn:aws:states:::http:invoke";

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`), and where a function's
/// URL is created (`POST /2021-10-31/functions/{FunctionName}/url`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The `State` a function reports once it can be invoked (`FunctionConfiguration$State`).
const FUNCTION_ACTIVE: &str = "Active";

/// The runtime and handler the echo package below is written for.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "index.handler";

/// How long one poll of a status the clock owes sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row waits for something the clock owes: a function reaching
/// [`FUNCTION_ACTIVE`], a connection reaching its settled `ConnectionState`.
const PATIENCE: Duration = Duration::from_secs(20);

/// How long a row waits for an execution to leave `RUNNING`.
const TERMINAL_PATIENCE: Duration =
    PAST_THE_EXECUTION_DELAY.saturating_add(Duration::from_secs(40));

/// The execution role the echoing function runs as. No IAM is mounted here, so nothing is
/// known about it and Lambda serves the create.
const LAMBDA_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/LambdaExecutionRole";

/// How one service's requests are addressed on the wire: `smithy_query(query=
/// "service_info")` of each model — the signing name, the `X-Amz-Target` prefix and the
/// `awsJson` version each of Step Functions and EventBridge declares.
const SFN_SIGNING_NAME: &str = "states";
/// See [`SFN_SIGNING_NAME`].
const SFN_TARGET_PREFIX: &str = "AWSStepFunctions";
/// See [`SFN_SIGNING_NAME`].
const EVENTS_SIGNING_NAME: &str = "events";
/// See [`SFN_SIGNING_NAME`].
const EVENTS_TARGET_PREFIX: &str = "AWSEvents";

/// The connection's API-KEY credential: the header name it sends the key under, and the
/// key itself — which lives in the connection's own secret and is what makes a request
/// through that connection an authorized one.
const API_KEY_HEADER: &str = "x-api-key";
/// See [`API_KEY_HEADER`].
const API_KEY_VALUE: &str = "the-key-itself";

/// The user name and password a BASIC connection builds its `Authorization` header from.
const BASIC_USER: &str = "ada";
/// See [`BASIC_USER`].
const BASIC_PASSWORD: &str = "l0velace";

/// The header the connection sends with every invocation through it, and the value it
/// sends under that name.
const CONNECTION_HEADER: &str = "x-connection";
/// See [`CONNECTION_HEADER`].
const CONNECTION_HEADER_VALUE: &str = "from-the-connection";

/// The header the connection declares a SECRET value for: it is held in the connection's
/// secret and no read of the connection reports it, and the invocation still carries it.
const SECRET_HEADER: &str = "x-connection-secret";
/// See [`SECRET_HEADER`].
const SECRET_HEADER_VALUE: &str = "kept-in-the-secret";

/// The path the echoing function answers a 404 for, so a row can observe what a task does
/// with an endpoint's own refusal.
const MISSING_PATH: &str = "/missing";

/// The handler every row's request reaches: it refuses a request carrying no credential
/// at all, and otherwise ECHOES the request it was given — the method, the path, the raw
/// query, the headers and the body — so every assertion below is about what really
/// arrived at the endpoint. One path answers a 404 instead, which is the endpoint refusing
/// rather than this emulator doing so.
const ECHO_SOURCE: &str = r#"
import json


def handler(event, context):
    headers = event.get("headers", {})
    path = event.get("rawPath", "/")
    if not headers.get("x-api-key") and not headers.get("authorization"):
        return {
            "statusCode": 403,
            "headers": {"content-type": "application/json"},
            "body": json.dumps({"message": "this endpoint takes a credential"}),
        }
    if path == "/missing":
        return {
            "statusCode": 404,
            "headers": {"content-type": "application/json"},
            "body": json.dumps({"message": "no such thing"}),
        }
    return {
        "statusCode": 200,
        "headers": {"content-type": "application/json", "x-echo": "1"},
        "body": json.dumps({
            "method": event.get("requestContext", {}).get("http", {}).get("method"),
            "path": path,
            "query": event.get("rawQueryString", ""),
            "headers": headers,
            "body": event.get("body"),
            "base64": event.get("isBase64Encoded"),
        }),
    }
"#;

/// Step Functions, Lambda and EventBridge over ONE engine, so the request a task makes is
/// served by the very function URL the test created and the connection it authenticates
/// through is the one EventBridge stored.
struct Chain {
    router: MultiServiceRouter,
}

impl Chain {
    fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(std::sync::Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(
                std::sync::Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// A SigV4 `Authorization` header whose credential scope names `service`, which is
    /// what routes the request. The signature is never checked: this router runs with
    /// request authentication off.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
             SignedHeaders=host, Signature=0123456789abcdef"
        )
    }

    /// One `awsJson` call, as its status and parsed body.
    async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        version: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", &host)
            .header("content-type", format!("application/x-amz-json-{version}"))
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", Self::authorization(signing_name))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed(response.into_body()))
    }

    /// One Step Functions call, requiring a 200.
    async fn sfn(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self
            .rpc(SFN_SIGNING_NAME, SFN_TARGET_PREFIX, "1.0", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One EventBridge call, as its status and parsed body.
    async fn events(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.rpc(
            EVENTS_SIGNING_NAME,
            EVENTS_TARGET_PREFIX,
            "1.1",
            operation,
            body,
        )
        .await
    }

    /// One `restJson1` call to Lambda's control plane, as its status and parsed body.
    async fn lambda(&self, method: &str, uri: &str, body: &Value) -> (u16, Value) {
        let payload = if body.is_null() {
            Vec::new()
        } else {
            body.to_string().into_bytes()
        };
        let host = format!("lambda.{REGION}.amazonaws.com");
        let request = http::Request::builder()
            .method(method)
            .uri(format!("http://{host}{uri}"))
            .header("host", &host)
            .header("content-type", "application/json")
            .header("authorization", Self::authorization("lambda"))
            .body(Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, parsed(response.into_body()))
    }

    /// The echoing function, its URL created and granted to the public, answering the
    /// endpoint Lambda minted for it.
    async fn given_echo_endpoint(&self, name: &str) -> String {
        let (status, created) = self
            .lambda(
                "POST",
                FUNCTIONS_URI,
                &json!({
                    "FunctionName": name,
                    "Role": LAMBDA_ROLE_ARN,
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Timeout": 20,
                    "Code": { "ZipFile": package(ECHO_SOURCE) },
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "CreateFunction must answer 201, got {status} with {created}"
        );
        let began = std::time::Instant::now();
        loop {
            let (status, configuration) = self
                .lambda(
                    "GET",
                    &format!("{FUNCTIONS_URI}/{name}/configuration"),
                    &Value::Null,
                )
                .await;
            assert_eq!(
                status, 200,
                "GetFunctionConfiguration must resolve the created function, got {status} with \
                 {configuration}"
            );
            if configuration.get("State").and_then(Value::as_str) == Some(FUNCTION_ACTIVE) {
                break;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "the function must reach {FUNCTION_ACTIVE} within {PATIENCE:?}, last saw \
                 {configuration}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
        let (status, url) = self
            .lambda(
                "POST",
                &format!("/2021-10-31/functions/{name}/url"),
                &json!({ "AuthType": "NONE" }),
            )
            .await;
        assert_eq!(
            status, 201,
            "CreateFunctionUrlConfig must answer 201, got {status} with {url}"
        );
        // An `AuthType` of `NONE` bypasses IAM authentication, and the function's own
        // resource-based policy is what admits the anonymous caller — so the endpoint is
        // public only once that grant is written.
        let (status, granted) = self
            .lambda(
                "POST",
                &format!("{FUNCTIONS_URI}/{name}/policy"),
                &json!({
                    "StatementId": "public-url",
                    "Action": "lambda:InvokeFunctionUrl",
                    "Principal": "*",
                    "FunctionUrlAuthType": "NONE",
                }),
            )
            .await;
        assert_eq!(
            status, 201,
            "AddPermission must answer 201, got {status} with {granted}"
        );
        string(&url, "FunctionUrl")
            .trim_end_matches('/')
            .to_string()
    }

    /// A connection created through EventBridge's own API, waited out to the
    /// `ConnectionState` it settles on; answers its ARN.
    ///
    /// `auth` is the `AuthorizationType` and the parameters for it, so one helper serves
    /// every authorization type a row asserts over. Every connection also carries
    /// `InvocationHttpParameters`: one header whose value a read reports and one the
    /// connection flags SECRET, the pair the merge rule and the secret rule are asserted
    /// over.
    async fn given_connection(&self, name: &str, auth: (&str, Value)) -> String {
        let (authorization_type, mut auth_parameters) = auth;
        auth_parameters["InvocationHttpParameters"] = json!({
            "HeaderParameters": [
                {
                    "Key": CONNECTION_HEADER,
                    "Value": CONNECTION_HEADER_VALUE,
                    "IsValueSecret": false,
                },
                {
                    "Key": SECRET_HEADER,
                    "Value": SECRET_HEADER_VALUE,
                    "IsValueSecret": true,
                },
            ],
        });
        let (status, created) = self
            .events(
                "CreateConnection",
                &json!({
                    "Name": name,
                    "AuthorizationType": authorization_type,
                    "AuthParameters": auth_parameters,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "CreateConnection must answer 200, got {status} with {created}"
        );
        let arn = string(&created, "ConnectionArn");
        self.awaited_connection(name, "AUTHORIZED").await;
        arn
    }

    /// The connection `name`, once it reports `state`.
    async fn awaited_connection(&self, name: &str, state: &str) {
        let began = std::time::Instant::now();
        loop {
            let (status, described) = self
                .events("DescribeConnection", &json!({ "Name": name }))
                .await;
            assert_eq!(
                status, 200,
                "DescribeConnection must resolve {name:?}, got {status} with {described}"
            );
            if described.get("ConnectionState").and_then(Value::as_str) == Some(state) {
                return;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "the connection {name:?} must reach {state} within {PATIENCE:?}, last saw \
                 {described}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// A connection that has been DEAUTHORIZED, so it holds no authorization at all;
    /// answers its ARN.
    async fn given_deauthorized_connection(&self, name: &str) -> String {
        let arn = self
            .given_connection(
                name,
                (
                    "API_KEY",
                    json!({
                        "ApiKeyAuthParameters": {
                            "ApiKeyName": API_KEY_HEADER,
                            "ApiKeyValue": API_KEY_VALUE,
                        },
                    }),
                ),
            )
            .await;
        let (status, deauthorized) = self
            .events("DeauthorizeConnection", &json!({ "Name": name }))
            .await;
        assert_eq!(
            status, 200,
            "DeauthorizeConnection must answer 200, got {status} with {deauthorized}"
        );
        self.awaited_connection(name, "DEAUTHORIZED").await;
        arn
    }

    /// Create `definition` as a state machine, start one execution of it, and report that
    /// execution once it has left `RUNNING`.
    async fn ran(&self, case: &str, name: &str, definition: &Value) -> Value {
        let created = self
            .sfn(
                case,
                "CreateStateMachine",
                &json!({
                    "name": name,
                    "definition": definition.to_string(),
                    "roleArn": ROLE_ARN,
                }),
            )
            .await;
        let machine = string(&created, "stateMachineArn");
        let started = self
            .sfn(
                case,
                "StartExecution",
                &json!({ "stateMachineArn": machine, "name": "only", "input": "{}" }),
            )
            .await;
        let execution = string(&started, "executionArn");
        let began = std::time::Instant::now();
        loop {
            let described = self
                .sfn(case, "DescribeExecution", &json!({ "executionArn": execution }))
                .await;
            if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
                return described;
            }
            assert!(
                began.elapsed() < TERMINAL_PATIENCE,
                "case {case:?}: expected a terminal status within {TERMINAL_PATIENCE:?}, last \
                 saw {described}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

/// One `Task` state named `Call` whose work is the HTTP request `parameters` describes.
fn one_http_task(parameters: Value) -> Value {
    json!({
        "StartAt": "Call",
        "States": {
            "Call": {
                "Type": "Task",
                "Resource": HTTP_INVOKE,
                "Parameters": parameters,
                "End": true,
            },
        },
    })
}

/// What an execution must come to for one row.
enum Outcome {
    /// It SUCCEEDED: the task's result must carry this status and reason phrase, the echo
    /// the endpoint answered must carry each of these members, the endpoint must have been
    /// given this document as its request body, and the request must have reached it
    /// carrying each of these headers.
    Succeeded {
        status: u64,
        status_text: &'static str,
        echoed: Vec<(&'static str, Value)>,
        request_body: Option<Value>,
        credential: Vec<(&'static str, String)>,
    },
    /// It FAILED under this error name, with a cause carrying each of these fragments.
    Failed {
        error: String,
        cause_has: Vec<String>,
    },
}

/// One row: what the task asks for, and what the execution must come to.
struct Row {
    case: &'static str,
    parameters: Value,
    outcome: Outcome,
}

/// An HTTP Task makes the request its parameters describe and reports the endpoint's own
/// response; the connection decides whether the request is made at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_http_task_makes_its_request_and_reports_the_endpoints_own_response() {
    let chain = Chain::new();
    let endpoint = chain.given_echo_endpoint("echo").await;
    let connection = chain
        .given_connection(
            "link",
            (
                "API_KEY",
                json!({
                    "ApiKeyAuthParameters": {
                        "ApiKeyName": API_KEY_HEADER,
                        "ApiKeyValue": API_KEY_VALUE,
                    },
                }),
            ),
        )
        .await;
    let basic = chain
        .given_connection(
            "basic-link",
            (
                "BASIC",
                json!({
                    "BasicAuthParameters": {
                        "Username": BASIC_USER,
                        "Password": BASIC_PASSWORD,
                    },
                }),
            ),
        )
        .await;
    let deauthorized = chain.given_deauthorized_connection("stale-link").await;
    let authentication = json!({ "ConnectionArn": connection });
    // The `Authorization` header a BASIC connection builds (RFC 7617), which is what the
    // endpoint must be given for a request authorized through that connection.
    let basic_header = format!(
        "Basic {}",
        ::base64::Engine::encode(
            &::base64::engine::general_purpose::STANDARD,
            format!("{BASIC_USER}:{BASIC_PASSWORD}").as_bytes(),
        )
    );
    // What every request through the API-KEY connection must carry: its key under the
    // header name the connection names, the connection's own non-secret parameter, and
    // the parameter whose value the connection holds in its secret — each of which only
    // the connection can supply.
    let api_key_credential: Vec<(&'static str, String)> = vec![
        (API_KEY_HEADER, API_KEY_VALUE.to_string()),
        (CONNECTION_HEADER, CONNECTION_HEADER_VALUE.to_string()),
        (SECRET_HEADER, SECRET_HEADER_VALUE.to_string()),
    ];
    // The form fields a URL-encoded body is sent as, under the documented default
    // `ArrayFormat` (`INDICES`). A form body arrives at a function URL BASE64-encoded,
    // because its media type is not one of the textual ones the payload format carries
    // verbatim, so the echo is compared with the base64 of exactly these bytes.
    let form_fields = "flag=true&tags%5B0%5D=a&tags%5B1%5D=b";
    let form_body = ::base64::Engine::encode(
        &::base64::engine::general_purpose::STANDARD,
        form_fields.as_bytes(),
    );

    let rows: Vec<Row> = vec![
        Row {
            case: "a POST carrying a JSON body",
            parameters: json!({
                "ApiEndpoint": format!("{endpoint}/orders"),
                "Method": "POST",
                "Authentication": authentication,
                "RequestBody": { "order": 7, "items": ["a", "b"] },
            }),
            outcome: Outcome::Succeeded {
                status: 200,
                status_text: "OK",
                echoed: vec![("method", json!("POST")), ("path", json!("/orders"))],
                request_body: Some(json!({ "order": 7, "items": ["a", "b"] })),
                credential: api_key_credential.clone(),
            },
        },
        Row {
            case: "a GET carrying query parameters, one of them repeated",
            parameters: json!({
                "ApiEndpoint": endpoint.clone(),
                "Method": "GET",
                "Authentication": authentication,
                "QueryParameters": { "flag": "1", "q": ["a b", "c"] },
            }),
            outcome: Outcome::Succeeded {
                status: 200,
                status_text: "OK",
                echoed: vec![
                    ("method", json!("GET")),
                    ("query", json!("flag=1&q=a%20b&q=c")),
                    ("body", Value::Null),
                ],
                request_body: None,
                credential: api_key_credential.clone(),
            },
        },
        Row {
            case: "a PUT whose headers the connection's own parameter overrides",
            parameters: json!({
                "ApiEndpoint": format!("{endpoint}/profile"),
                "Method": "PUT",
                "Authentication": authentication,
                "Headers": {
                    "x-task": "from-the-task",
                    CONNECTION_HEADER: "overridden",
                },
                "RequestBody": { "name": "ada" },
            }),
            outcome: Outcome::Succeeded {
                status: 200,
                status_text: "OK",
                echoed: vec![("method", json!("PUT")), ("path", json!("/profile"))],
                request_body: Some(json!({ "name": "ada" })),
                // The task's own header travels beside the connection's, and the
                // connection's value wins where both name one.
                credential: {
                    let mut headers = api_key_credential.clone();
                    headers.push(("x-task", "from-the-task".to_string()));
                    headers
                },
            },
        },
        Row {
            case: "a POST whose body the Transform asks to URL-encode",
            parameters: json!({
                "ApiEndpoint": format!("{endpoint}/form"),
                "Method": "POST",
                "Authentication": authentication,
                "RequestBody": { "flag": true, "tags": ["a", "b"] },
                "Transform": { "RequestBodyEncoding": "URL_ENCODED" },
            }),
            outcome: Outcome::Succeeded {
                status: 200,
                status_text: "OK",
                echoed: vec![
                    ("path", json!("/form")),
                    ("body", json!(form_body)),
                    ("base64", json!(true)),
                ],
                request_body: None,
                credential: api_key_credential.clone(),
            },
        },
        Row {
            case: "a request authorized through a BASIC connection",
            parameters: json!({
                "ApiEndpoint": format!("{endpoint}/basic"),
                "Method": "GET",
                "Authentication": { "ConnectionArn": basic },
            }),
            outcome: Outcome::Succeeded {
                status: 200,
                status_text: "OK",
                echoed: vec![("path", json!("/basic"))],
                request_body: None,
                credential: vec![("authorization", basic_header.clone())],
            },
        },
        Row {
            case: "an endpoint that answers 404",
            parameters: json!({
                "ApiEndpoint": format!("{endpoint}{MISSING_PATH}"),
                "Method": "GET",
                "Authentication": authentication,
            }),
            outcome: Outcome::Failed {
                error: "States.Http.StatusCode.404".to_string(),
                cause_has: vec!["no such thing".to_string(), "Not Found".to_string()],
            },
        },
        Row {
            case: "a ConnectionArn that resolves to no connection",
            parameters: json!({
                "ApiEndpoint": endpoint.clone(),
                "Method": "GET",
                "Authentication": {
                    "ConnectionArn": format!(
                        "arn:aws:events:{REGION}:000000000000:connection/absent/0000"
                    ),
                },
            }),
            outcome: Outcome::Failed {
                error: "Events.ConnectionResource.ResourceNotFound".to_string(),
                cause_has: vec!["connection/absent".to_string()],
            },
        },
        Row {
            case: "a connection that has been deauthorized",
            parameters: json!({
                "ApiEndpoint": endpoint.clone(),
                "Method": "GET",
                "Authentication": { "ConnectionArn": deauthorized },
            }),
            outcome: Outcome::Failed {
                error: "Events.ConnectionResource.InvalidConnectionState".to_string(),
                cause_has: vec!["DEAUTHORIZED".to_string()],
            },
        },
        Row {
            case: "a ConnectionArn that is not a connection ARN at all",
            parameters: json!({
                "ApiEndpoint": endpoint.clone(),
                "Method": "GET",
                "Authentication": { "ConnectionArn": "link" },
            }),
            outcome: Outcome::Failed {
                error: "Events.ConnectionResource.ResourceNotFound".to_string(),
                cause_has: vec!["ConnectionArn".to_string()],
            },
        },
        Row {
            case: "an ApiEndpoint nothing in this emulator serves",
            parameters: json!({
                "ApiEndpoint": "https://nothing.serves.this.invalid/",
                "Method": "GET",
                "Authentication": authentication,
            }),
            outcome: Outcome::Failed {
                error: "States.Runtime".to_string(),
                cause_has: vec!["nothing in this emulator serves".to_string()],
            },
        },
        Row {
            case: "an ApiEndpoint that is not an absolute URL",
            parameters: json!({
                "ApiEndpoint": "/orders",
                "Method": "GET",
                "Authentication": authentication,
            }),
            outcome: Outcome::Failed {
                error: "States.Runtime".to_string(),
                cause_has: vec!["ApiEndpoint".to_string()],
            },
        },
        Row {
            case: "a Method that is not an HTTP method",
            parameters: json!({
                "ApiEndpoint": endpoint.clone(),
                "Method": "G E T",
                "Authentication": authentication,
            }),
            outcome: Outcome::Failed {
                error: "States.Runtime".to_string(),
                cause_has: vec!["Method".to_string()],
            },
        },
        Row {
            case: "a task naming no Authentication at all",
            parameters: json!({
                "ApiEndpoint": endpoint.clone(),
                "Method": "GET",
            }),
            outcome: Outcome::Failed {
                error: "States.Runtime".to_string(),
                cause_has: vec!["Authentication".to_string()],
            },
        },
    ];

    for (at, row) in rows.iter().enumerate() {
        let case = row.case;
        let described = chain
            .ran(
                case,
                &format!("http-task-{at}"),
                &one_http_task(row.parameters.clone()),
            )
            .await;
        let status = described
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match &row.outcome {
            Outcome::Succeeded {
                status: expected_status,
                status_text,
                echoed,
                request_body,
                credential,
            } => {
                assert_eq!(
                    status, "SUCCEEDED",
                    "case {case:?}: the execution must succeed, got {described}"
                );
                let output: Value = serde_json::from_str(&string(&described, "output"))
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: the output must be a JSON document, got {error}")
                    });
                assert_eq!(
                    output.get("StatusCode").and_then(Value::as_u64),
                    Some(*expected_status),
                    "case {case:?}: expected StatusCode {expected_status}, got {output}"
                );
                assert_eq!(
                    output.get("StatusText").and_then(Value::as_str),
                    Some(*status_text),
                    "case {case:?}: expected StatusText {status_text:?}, got {output}"
                );
                let body = output
                    .get("ResponseBody")
                    .cloned()
                    .unwrap_or(Value::Null);
                for (member, expected) in echoed {
                    let read = body.get(*member).cloned().unwrap_or(Value::Null);
                    assert_eq!(
                        &read, expected,
                        "case {case:?}: the endpoint must have been given {member} {expected}, \
                         got {read} in {body}"
                    );
                }
                // The body the endpoint was given, compared as the DOCUMENT it carries
                // rather than as its text: which order a document's members are written
                // in is no part of what the task sends.
                if let Some(expected) = request_body {
                    let sent: Value = body
                        .get("body")
                        .and_then(Value::as_str)
                        .map(|text| {
                            serde_json::from_str(text).unwrap_or_else(|error| {
                                panic!(
                                    "case {case:?}: the endpoint must have been given a JSON \
                                     body, got {error} for {text:?}"
                                )
                            })
                        })
                        .unwrap_or(Value::Null);
                    assert_eq!(
                        &sent, expected,
                        "case {case:?}: the endpoint must have been given the body {expected}, \
                         got {sent}"
                    );
                }
                // The connection's own authorization and parameters travel with every
                // request it authorizes — including the parameter whose value it holds in
                // its secret — and the connection's value wins over a header of the same
                // name the task asks for. None of them is anything the task's own
                // parameters carry, so a request that reaches the endpoint with them is
                // one the connection authorized.
                let headers = body.get("headers").cloned().unwrap_or(Value::Null);
                for (name, expected) in credential {
                    assert_eq!(
                        headers.get(*name).and_then(Value::as_str),
                        Some(expected.as_str()),
                        "case {case:?}: the endpoint must have been given the header {name:?} \
                         as {expected:?}, got {headers}"
                    );
                }
            }
            Outcome::Failed { error, cause_has } => {
                assert_eq!(
                    status, "FAILED",
                    "case {case:?}: the execution must fail, got {described}"
                );
                let reported = described
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                assert_eq!(
                    reported, error,
                    "case {case:?}: expected the error {error:?}, got {described}"
                );
                let cause = described
                    .get("cause")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                for fragment in cause_has {
                    assert!(
                        cause.contains(fragment.as_str()),
                        "case {case:?}: the cause must name {fragment:?}, got {cause:?}"
                    );
                }
            }
        }
    }
}
