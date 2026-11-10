// SPDX-License-Identifier: CC0-1.0

//! A WEBSOCKET api's routes served by the FUNCTIONS they front, over a real connection: the
//! `$connect` / `$disconnect` lifecycle, the route each frame is dispatched to, and the
//! `REQUEST` authorizer that decides the handshake.
//!
//! The rule every table below is one slice of: **each call of a WebSocket connection is
//! served by performing the integration of the route that call belongs to — the `$connect`
//! route for the opening handshake, the route the api's `RouteSelectionExpression` selects
//! for each frame, the `$disconnect` route for the close — so an `AWS_PROXY` integration's
//! answer is the answer of the function it names, invoked with the event that call is
//! (`requestContext.eventType` `CONNECT` / `MESSAGE` / `DISCONNECT`, the route's own
//! `routeKey`, and the one `connectionId` the handshake minted, with the handshake's query
//! string on the connection request), while the handshake itself completes only on a `2xx`
//! from the connection request and a `REQUEST` authorizer in front of it decides it first:
//! `401` when an identity source is absent (the authorizer not invoked at all), `403` when
//! its function denies, and otherwise the context it returned reaches every later call's
//! event.**
//!
//! Nothing here is a value this emulator fills in: every assertion is on something a
//! function RAN and returned, which is why AWS Lambda is mounted beside this service on one
//! engine, and why the lifecycle routes — whose answers reach no client — record the event
//! they were given in an Amazon SQS queue, through the AWS SDK their runtime supplies, the
//! way a `$connect` handler stores a connection id.
//!
//! The suites that own the steps BEFORE the integration are not restated here: the
//! handshake's own statuses and the `MOCK` reply rule are
//! `websocket_transport_data_plane.rs`, and the identity-source precondition over every
//! spelling it takes is `websocket_connect_authorization.rs`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::http_api_backend_dispatch::given_function;
use crate::websocket_transport_data_plane::{exchange, handshake_at, ClientSocket};

/// The Region every control-plane request below is signed for.
const REGION: &str = "us-east-1";

/// The stage every api below is deployed to.
const STAGE: &str = "prod";

/// The statuses a refused handshake carries: the `403` an authorizer's DENY earns and the
/// `401` an absent identity source earns.
const FORBIDDEN: u16 = 403;
const UNAUTHORIZED: u16 = 401;

/// How long a case waits for a recorded event before failing, and how often it looks: a
/// lifecycle route's own answer reaches no client, so the record it wrote is read by polling
/// the queue.
const RECORD_WAIT: Duration = Duration::from_secs(20);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// This service, AWS Lambda and Amazon SQS on ONE engine, with request authentication off.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTAPIGWV2/20261021/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One control-plane request against THIS service, as its status and parsed body.
async fn control(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let host = format!("apigateway.{REGION}.amazonaws.com");
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("the body serializes")),
        None => bytes::Bytes::new(),
    };
    let request = smithy_server_runtime::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header("authorization", authorization("apigateway"))
        .body(payload)
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One control-plane request that must answer `expected`.
async fn created(
    emulator: &MultiServiceRouter,
    case: &str,
    what: &str,
    uri: &str,
    body: Value,
) -> Value {
    let (status, answered) = control(emulator, "POST", uri, Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: {what} must answer 201, got {status} {answered}"
    );
    answered
}

/// One Amazon SQS call that must answer 200.
async fn sqs(emulator: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let host = format!("sqs.{REGION}.amazonaws.com");
    let request = smithy_server_runtime::http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", &host)
        .header("authorization", authorization("sqs"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the request builds");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: SQS {operation} must answer 200, got {status} {parsed}"
    );
    parsed
}

/// The queue the lifecycle routes record the event they were given in.
async fn given_queue(emulator: &MultiServiceRouter, case: &str) -> String {
    let created = sqs(
        emulator,
        case,
        "CreateQueue",
        json!({ "QueueName": "websocket-route-records" }),
    )
    .await;
    created
        .get("QueueUrl")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateQueue answered no QueueUrl: {created}"))
        .to_string()
}

/// A handler that records the event it was given in the queue and answers `reply`.
///
/// It is the pattern a real `$connect` handler follows — the connection id it is given is
/// what it stores — and it is how a call whose answer reaches no client is observed here.
fn recording_handler(name: &str, queue_url: &str, reply: &str) -> String {
    format!(
        "import json\n\
         import boto3\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   boto3.client(\"sqs\").send_message(\n\
         \x20       QueueUrl={queue:?},\n\
         \x20       MessageBody=json.dumps({{\"fn\": {name:?}, \"event\": event}}),\n\
         \x20   )\n\
         \x20   return {reply}\n",
        queue = queue_url,
        name = name,
        reply = reply,
    )
}

/// A handler that answers the event it was given as its response body, which is how a
/// two-way route's own event is asserted.
fn echoing_handler(name: &str) -> String {
    format!(
        "import json\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   return {{\"statusCode\": 200,\n\
         \x20           \"body\": json.dumps({{\"fn\": {name:?}, \"event\": event}})}}\n",
        name = name,
    )
}

/// A `REQUEST` authorizer's function: it records the event it was given and answers the
/// policy document the handshake's own `token` asks for, with a context for the routes.
fn authorizer_handler(queue_url: &str) -> String {
    format!(
        "import json\n\
         import boto3\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   boto3.client(\"sqs\").send_message(\n\
         \x20       QueueUrl={queue:?},\n\
         \x20       MessageBody=json.dumps({{\"fn\": \"authorizer\", \"event\": event}}),\n\
         \x20   )\n\
         \x20   token = (event.get(\"queryStringParameters\") or {{}}).get(\"token\", \"\")\n\
         \x20   effect = \"Allow\" if token == \"open\" else \"Deny\"\n\
         \x20   return {{\n\
         \x20       \"principalId\": \"connection-principal\",\n\
         \x20       \"policyDocument\": {{\n\
         \x20           \"Version\": \"2012-10-17\",\n\
         \x20           \"Statement\": [{{\n\
         \x20               \"Action\": \"execute-api:Invoke\",\n\
         \x20               \"Effect\": effect,\n\
         \x20               \"Resource\": event[\"methodArn\"],\n\
         \x20           }}],\n\
         \x20       }},\n\
         \x20       \"context\": {{\"tier\": \"gold\", \"token\": token}},\n\
         \x20   }}\n",
        queue = queue_url,
    )
}

/// The same authorizer answering in the OTHER format a Lambda authorizer states a decision
/// in: the boolean, "instead of an IAM policy".
fn simple_authorizer_handler(queue_url: &str) -> String {
    format!(
        "import json\n\
         import boto3\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   boto3.client(\"sqs\").send_message(\n\
         \x20       QueueUrl={queue:?},\n\
         \x20       MessageBody=json.dumps({{\"fn\": \"authorizer\", \"event\": event}}),\n\
         \x20   )\n\
         \x20   token = (event.get(\"queryStringParameters\") or {{}}).get(\"token\", \"\")\n\
         \x20   return {{\n\
         \x20       \"isAuthorized\": token == \"open\",\n\
         \x20       \"context\": {{\"tier\": \"gold\", \"token\": token}},\n\
         \x20   }}\n",
        queue = queue_url,
    )
}

/// One api, created with the socket its `ApiEndpoint` announces.
struct Api {
    api_id: String,
    /// The `host:port` of the api's own socket, read off the announced `ApiEndpoint` so a
    /// case dials the address the service reported.
    authority: String,
}

/// Create a WEBSOCKET api whose frames select a route by their `action` member.
async fn given_api(emulator: &MultiServiceRouter, case: &str) -> Api {
    let created = created(
        emulator,
        case,
        "CreateApi",
        "/v2/apis",
        json!({
            "name": "socket-routes-api",
            "protocolType": "WEBSOCKET",
            "routeSelectionExpression": "$request.body.action"
        }),
    )
    .await;
    let announced = created
        .get("apiEndpoint")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let authority = announced
        .strip_prefix("wss://")
        .unwrap_or_else(|| {
            panic!("case {case:?}: a WEBSOCKET api must announce a wss:// socket, got {announced:?}")
        })
        .to_string();
    Api {
        api_id: created
            .get("apiId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        authority,
    }
}

/// Bind `function_arn` as an `AWS_PROXY` integration of `api_id` and route `route_key` to
/// it, with a `RouteResponse` when `two_way` — "to define a two-way route, create a route
/// response".
async fn given_route(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    route_key: &str,
    function_arn: &str,
    two_way: bool,
    authorizer_id: Option<&str>,
) {
    let integration = created(
        emulator,
        case,
        "CreateIntegration",
        &format!("/v2/apis/{api_id}/integrations"),
        json!({
            "integrationType": "AWS_PROXY",
            "integrationUri": function_arn,
            "integrationMethod": "POST"
        }),
    )
    .await;
    let integration_id = integration
        .get("integrationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut body = json!({
        "routeKey": route_key,
        "target": format!("integrations/{integration_id}")
    });
    if let Some(authorizer_id) = authorizer_id {
        // `CUSTOM` is the `AuthorizationType` "for using a Lambda authorizer".
        body["authorizationType"] = json!("CUSTOM");
        body["authorizerId"] = json!(authorizer_id);
    }
    let route = created(
        emulator,
        case,
        "CreateRoute",
        &format!("/v2/apis/{api_id}/routes"),
        body,
    )
    .await;
    if two_way {
        let route_id = route
            .get("routeId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        created(
            emulator,
            case,
            "CreateRouteResponse",
            &format!("/v2/apis/{api_id}/routes/{route_id}/routeresponses"),
            json!({ "routeResponseKey": "$default" }),
        )
        .await;
    }
}

/// Create the stage every case dials and release a deployment to it.
async fn given_stage(emulator: &MultiServiceRouter, case: &str, api_id: &str) {
    created(
        emulator,
        case,
        "CreateStage",
        &format!("/v2/apis/{api_id}/stages"),
        json!({ "stageName": STAGE }),
    )
    .await;
    created(
        emulator,
        case,
        "CreateDeployment",
        &format!("/v2/apis/{api_id}/deployments"),
        json!({ "stageName": STAGE }),
    )
    .await;
}

/// Create a `REQUEST` authorizer over the handshake's `token` query parameter and answer its
/// `AuthorizerId`.
async fn given_authorizer(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    function_arn: &str,
) -> String {
    // The form `AuthorizerUri` documents: "arn:aws:apigateway:{region}:lambda:path/
    // 2015-03-31/functions/{function-arn}/invocations".
    let uri = format!(
        "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/{function_arn}/invocations"
    );
    let authorizer = created(
        emulator,
        case,
        "CreateAuthorizer",
        &format!("/v2/apis/{api_id}/authorizers"),
        json!({
            "name": "connect-auth",
            "authorizerType": "REQUEST",
            "authorizerUri": uri,
            "identitySource": ["route.request.querystring.token"],
            // No caching, so each handshake of a case is decided by its own invocation.
            "authorizerResultTtlInSeconds": 0
        }),
    )
    .await;
    authorizer
        .get("authorizerId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: CreateAuthorizer answered no AuthorizerId"))
        .to_string()
}

/// The records written to `queue_url` so far, drained from it: each is what one handler
/// recorded, `{"fn": …, "event": …}`.
async fn drained(emulator: &MultiServiceRouter, case: &str, queue_url: &str) -> Vec<Value> {
    let received = sqs(
        emulator,
        case,
        "ReceiveMessage",
        json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    received
        .get("Messages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| message.get("Body").and_then(Value::as_str))
                .filter_map(|body| serde_json::from_str::<Value>(body).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Wait until a record whose `fn` is `name` has been written, and answer the event it
/// carried; panic when none arrives within [`RECORD_WAIT`].
///
/// A function's run is started by this emulator and is not instantaneous, so the wait is for
/// a record the CALL already owes rather than for anything a clock schedules.
async fn recorded_event(
    emulator: &MultiServiceRouter,
    case: &str,
    queue_url: &str,
    name: &str,
) -> Value {
    let deadline = std::time::Instant::now() + RECORD_WAIT;
    let mut seen: Vec<String> = Vec::new();
    while std::time::Instant::now() < deadline {
        for record in drained(emulator, case, queue_url).await {
            let recorded = record
                .get("fn")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if recorded == name {
                return record.get("event").cloned().unwrap_or(Value::Null);
            }
            seen.push(recorded);
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("case {case:?}: expected a record from {name:?}, saw records from {seen:?}");
}

/// The member of `event` at `pointer`, as JSON.
fn member(event: &Value, pointer: &str) -> Value {
    event.pointer(pointer).cloned().unwrap_or(Value::Null)
}

#[tokio::test]
async fn each_call_of_a_connection_is_served_by_its_own_routes_function() {
    let case = "a connection whose $connect, $disconnect and two message routes each front \
                their own function";
    let (emulator, engine) = emulator();
    let queue = given_queue(&emulator, case).await;

    // One function per route key, as a client provisions them: the lifecycle routes record
    // the event they were given, and the message routes answer it.
    let connect = given_function(
        &emulator,
        &engine,
        case,
        "connect-route",
        &recording_handler("$connect", &queue, "{\"statusCode\": 200}"),
    )
    .await;
    let disconnect = given_function(
        &emulator,
        &engine,
        case,
        "disconnect-route",
        &recording_handler("$disconnect", &queue, "{\"statusCode\": 200}"),
    )
    .await;
    let ping = given_function(&emulator, &engine, case, "ping-route", &echoing_handler("ping"))
        .await;
    let sendmsg = given_function(
        &emulator,
        &engine,
        case,
        "sendmsg-route",
        &echoing_handler("sendmsg"),
    )
    .await;

    let api = given_api(&emulator, case).await;
    // No `$default` route: an action no route matches is the api's own refusal.
    for (route_key, function, two_way) in [
        ("$connect", &connect, false),
        ("$disconnect", &disconnect, false),
        ("ping", &ping, true),
        ("sendmsg", &sendmsg, true),
    ] {
        given_route(
            &emulator,
            case,
            &api.api_id,
            route_key,
            function,
            two_way,
            None,
        )
        .await;
    }
    given_stage(&emulator, case, &api.api_id).await;

    // The handshake: completed on the `$connect` function's own 2xx, with the query string
    // the client dialled with.
    let mut socket: ClientSocket = handshake_at(
        case,
        &api.authority,
        &format!("/{STAGE}?token=open&trace=abc"),
    )
    .await
    .unwrap_or_else(|status| {
        panic!("case {case:?}: the handshake must be completed, got {status}")
    });

    let connect_event = recorded_event(&emulator, case, &queue, "$connect").await;
    let connection_id = member(&connect_event, "/requestContext/connectionId");
    for (what, pointer, expected) in [
        (
            "the kind of call",
            "/requestContext/eventType",
            json!("CONNECT"),
        ),
        (
            "the route that served it",
            "/requestContext/routeKey",
            json!("$connect"),
        ),
        ("the stage", "/requestContext/stage", json!(STAGE)),
        (
            "the handshake's query string",
            "/queryStringParameters/token",
            json!("open"),
        ),
        (
            "every parameter of it",
            "/queryStringParameters/trace",
            json!("abc"),
        ),
    ] {
        assert_eq!(
            member(&connect_event, pointer),
            expected,
            "case {case:?}: expected {what} at {pointer} to be {expected}, got \
             {connect_event}"
        );
    }
    assert!(
        connection_id
            .as_str()
            .is_some_and(|id| !id.trim().is_empty()),
        "case {case:?}: the connection request must carry the minted connectionId, got \
         {connect_event}"
    );

    // Each frame: dispatched by the route selection expression to that action's own
    // function, under the SAME connection, and answered with what that function returned.
    //
    // The rows span what the dispatch generalises over: each action with a route of its
    // own, an action no route matches on an api with no `$default` route, a frame naming no
    // action at all, and a frame that is no JSON object.
    let cases: [(&str, &str, Option<&str>); 5] = [
        ("the first action", r#"{"action":"ping"}"#, Some("ping")),
        (
            "another action, which is another function",
            r#"{"action":"sendmsg","text":"hello"}"#,
            Some("sendmsg"),
        ),
        (
            "the first action again, on the same connection",
            r#"{"action":"ping","again":true}"#,
            Some("ping"),
        ),
        ("an action no route matches", r#"{"action":"nope"}"#, None),
        ("a frame that is no JSON object", "not json at all", None),
    ];
    for (row, frame, expected) in cases {
        let answer = exchange(case, &mut socket, frame).await.unwrap_or_else(|| {
            panic!("case {case:?} / {row}: the api must answer this frame, got nothing")
        });
        let answered: Value = serde_json::from_str(&answer).unwrap_or_else(|error| {
            panic!("case {case:?} / {row}: expected a JSON frame, got {answer:?}: {error}")
        });
        match expected {
            Some(name) => {
                assert_eq!(
                    member(&answered, "/fn"),
                    json!(name),
                    "case {case:?} / {row}: expected {name:?}'s own function to answer, got \
                     {answered}"
                );
                for (what, pointer, expected) in [
                    (
                        "the kind of call",
                        "/event/requestContext/eventType",
                        json!("MESSAGE"),
                    ),
                    (
                        "the route that served it",
                        "/event/requestContext/routeKey",
                        json!(name),
                    ),
                    (
                        "the connection it arrived on",
                        "/event/requestContext/connectionId",
                        connection_id.clone(),
                    ),
                    ("the frame itself", "/event/body", json!(frame)),
                ] {
                    assert_eq!(
                        member(&answered, pointer),
                        expected,
                        "case {case:?} / {row}: expected {what} at {pointer} to be \
                         {expected}, got {answered}"
                    );
                }
            }
            None => {
                // An action the api routes nowhere: the gateway's own frame, naming the
                // connection it is about — no function of the api is reached at all.
                assert_eq!(
                    member(&answered, "/message"),
                    json!("Forbidden"),
                    "case {case:?} / {row}: expected the gateway's Forbidden frame, got \
                     {answered}"
                );
                assert_eq!(
                    member(&answered, "/connectionId"),
                    connection_id.clone(),
                    "case {case:?} / {row}: the gateway's frame names the connection, got \
                     {answered}"
                );
            }
        }
    }

    // The close: the `$disconnect` route's own function, with the same connection.
    drop(socket);
    let disconnect_event = recorded_event(&emulator, case, &queue, "$disconnect").await;
    for (what, pointer, expected) in [
        (
            "the kind of call",
            "/requestContext/eventType",
            json!("DISCONNECT"),
        ),
        (
            "the route that served it",
            "/requestContext/routeKey",
            json!("$disconnect"),
        ),
        (
            "the connection that ended",
            "/requestContext/connectionId",
            connection_id.clone(),
        ),
    ] {
        assert_eq!(
            member(&disconnect_event, pointer),
            expected,
            "case {case:?}: expected {what} at {pointer} to be {expected}, got \
             {disconnect_event}"
        );
    }
}

#[tokio::test]
async fn the_connection_requests_own_status_decides_the_handshake() {
    /// What one row expects of the handshake: completed, or refused with this status and
    /// the body the connection request's own function returned.
    type Expected = Result<(), (u16, &'static str)>;
    // The rows span what the connection request can answer: each class of status, and a
    // reply naming none at all. A `2xx` admits the connection; anything else refuses the
    // handshake with that status and the body the function returned.
    let cases: [(&str, &str, Expected); 6] = [
        (
            "a 200, which admits the connection",
            r#"{"statusCode": 200}"#,
            Ok(()),
        ),
        (
            "another success status, which admits it too",
            r#"{"statusCode": 201, "body": "ready"}"#,
            Ok(()),
        ),
        (
            "a 403 with the body the function chose",
            r#"{"statusCode": 403, "body": "denied"}"#,
            Err((403, "denied")),
        ),
        (
            "a 401, which is that status and no other",
            r#"{"statusCode": 401, "body": "who?"}"#,
            Err((401, "who?")),
        ),
        (
            "a 500 the function itself answered",
            r#"{"statusCode": 500, "body": "boom"}"#,
            Err((500, "boom")),
        ),
        (
            "a reply naming no status, which is no decision at all",
            r#"{"ok": true}"#,
            Err((500, "")),
        ),
    ];
    for (row, reply, expected) in cases {
        let case = row;
        let (emulator, engine) = emulator();
        let queue = given_queue(&emulator, case).await;
        let connect = given_function(
            &emulator,
            &engine,
            case,
            "connect-decision",
            &recording_handler("$connect", &queue, reply),
        )
        .await;
        let api = given_api(&emulator, case).await;
        given_route(&emulator, case, &api.api_id, "$connect", &connect, false, None).await;
        given_stage(&emulator, case, &api.api_id).await;

        let answered = handshake_at(case, &api.authority, &format!("/{STAGE}")).await;
        match (expected, answered) {
            (Ok(()), Ok(_socket)) => {}
            (Ok(()), Err(status)) => {
                panic!("case {case:?}: the handshake must be completed, got {status}")
            }
            (Err((expected_status, _body)), Err(status)) => assert_eq!(
                status, expected_status,
                "case {case:?}: expected the handshake refused {expected_status}, got {status}"
            ),
            (Err((expected_status, _body)), Ok(_socket)) => panic!(
                "case {case:?}: expected the handshake refused {expected_status}, got 101"
            ),
        }
        // The function RAN either way: a connection request is served before the handshake
        // is answered, and it is given the connection id the gateway minted for it.
        let connect_event = recorded_event(&emulator, case, &queue, "$connect").await;
        assert!(
            member(&connect_event, "/requestContext/connectionId")
                .as_str()
                .is_some_and(|id| !id.trim().is_empty()),
            "case {case:?}: the connection request must carry a connectionId, got \
             {connect_event}"
        );
    }
}

#[tokio::test]
async fn a_request_authorizer_decides_the_handshake_and_its_context_reaches_the_routes() {
    // The rows span the three answers the authorization owes: the identity source absent
    // (the function not invoked at all), the function denying, and the function allowing —
    // whose context then reaches the message route's own event.
    let cases: [(&str, &str, Option<u16>, bool); 4] = [
        (
            "an authorizer that authorizes, which completes the handshake",
            "?token=open",
            None,
            true,
        ),
        (
            "an authorizer that refuses, which refuses the handshake",
            "?token=shut",
            Some(FORBIDDEN),
            true,
        ),
        (
            "the identity source absent, which refuses it without invoking the function",
            "",
            Some(UNAUTHORIZED),
            false,
        ),
        (
            "the identity source present but empty, which is absent too",
            "?token=",
            Some(UNAUTHORIZED),
            false,
        ),
    ];
    // And each row is driven once per FORMAT its function states the decision in: the IAM
    // policy document a Lambda authorizer must return, and the boolean it "can return …
    // instead of an IAM policy". A WebSocket api's authorizer declares neither
    // `EnableSimpleResponses` nor `AuthorizerPayloadFormatVersion` — both are an HTTP api's —
    // so the answer itself is what says which of the two it is.
    type Handler = fn(&str) -> String;
    let formats: [(&str, Handler, bool); 2] = [
        ("an IAM policy document", authorizer_handler, true),
        ("the boolean of a simple response", simple_authorizer_handler, false),
    ];
    for (format, handler, names_a_principal) in formats {
        for (row, query, refused, invoked) in cases {
            let case = &format!("{row}, stated as {format}");
            let case = case.as_str();
            let (emulator, engine) = emulator();
            let queue = given_queue(&emulator, case).await;
            let authorizer_function = given_function(
                &emulator,
                &engine,
                case,
                "connect-authorizer",
                &handler(&queue),
            )
            .await;
            let connect = given_function(
                &emulator,
                &engine,
                case,
                "connect-route",
                &recording_handler("$connect", &queue, "{\"statusCode\": 200}"),
            )
            .await;
            let ping =
                given_function(&emulator, &engine, case, "ping-route", &echoing_handler("ping"))
                    .await;

            let api = given_api(&emulator, case).await;
            let authorizer_id =
                given_authorizer(&emulator, case, &api.api_id, &authorizer_function).await;
            given_route(
                &emulator,
                case,
                &api.api_id,
                "$connect",
                &connect,
                false,
                Some(&authorizer_id),
            )
            .await;
            given_route(&emulator, case, &api.api_id, "ping", &ping, true, None).await;
            given_stage(&emulator, case, &api.api_id).await;

            let answered = handshake_at(case, &api.authority, &format!("/{STAGE}{query}")).await;
            let mut socket = match (refused, answered) {
                (None, Ok(socket)) => socket,
                (None, Err(status)) => {
                    panic!("case {case:?}: the handshake must be completed, got {status}")
                }
                (Some(expected), Err(status)) => {
                    assert_eq!(
                        status, expected,
                        "case {case:?}: expected the handshake refused {expected}, got {status}"
                    );
                    if !invoked {
                        // "Otherwise, it returns a 401 Unauthorized response without calling
                        // the Lambda function": nothing of the api ran, so nothing recorded.
                        let records = drained(&emulator, case, &queue).await;
                        assert!(
                            records.is_empty(),
                            "case {case:?}: no function may run for a handshake refused \
                             before the authorizer, got {records:?}"
                        );
                    }
                    continue;
                }
                (Some(expected), Ok(_socket)) => {
                    panic!("case {case:?}: expected the handshake refused {expected}, got 101")
                }
            };

            // The authorizer ran, and it was given the connection request's own event.
            let authorizer_event = recorded_event(&emulator, case, &queue, "authorizer").await;
            for (what, pointer, expected) in [
                ("the authorizer kind", "/type", json!("REQUEST")),
                (
                    "the call it decides",
                    "/requestContext/eventType",
                    json!("CONNECT"),
                ),
                (
                    "the route it decides",
                    "/requestContext/routeKey",
                    json!("$connect"),
                ),
            ] {
                assert_eq!(
                    member(&authorizer_event, pointer),
                    expected,
                    "case {case:?}: expected {what} at {pointer} to be {expected}, got \
                     {authorizer_event}"
                );
            }
            assert!(
                member(&authorizer_event, "/methodArn")
                    .as_str()
                    .is_some_and(|arn| arn.contains(&api.api_id)
                        && arn.contains(STAGE)
                        && arn.ends_with("$connect")),
                "case {case:?}: the authorizer event must name the route it decides, got \
                 {authorizer_event}"
            );

            // And what it ALLOWED reaches every later call of the connection.
            let answer = exchange(case, &mut socket, r#"{"action":"ping"}"#)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: the ping route must answer"));
            let answered: Value = serde_json::from_str(&answer).unwrap_or_else(|error| {
                panic!("case {case:?}: expected JSON, got {answer:?}: {error}")
            });
            for (what, pointer, expected) in [
                (
                    "the context the authorizer returned",
                    "/event/requestContext/authorizer/tier",
                    json!("gold"),
                ),
                (
                    "every member of it",
                    "/event/requestContext/authorizer/token",
                    json!("open"),
                ),
                (
                    // A policy-document answer names a principal beside its context; the
                    // boolean names none, so nothing is reported for one.
                    "the principal it named",
                    "/event/requestContext/authorizer/principalId",
                    if names_a_principal {
                        json!("connection-principal")
                    } else {
                        Value::Null
                    },
                ),
            ] {
                assert_eq!(
                    member(&answered, pointer),
                    expected,
                    "case {case:?}: expected {what} at {pointer} to be {expected}, got \
                     {answered}"
                );
            }
        }
    }
}
