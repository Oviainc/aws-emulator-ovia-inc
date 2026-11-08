// SPDX-License-Identifier: CC0-1.0

//! REPEATED requests to one deployed REST stage, and what each of them leaves behind.
//!
//! The rule under test: **every request to a deployed REST stage is dispatched to the
//! method's integration on its own — a stage holds no per-client state that makes a second
//! identical request different from the first — so N identical requests are answered N
//! times, the handler runs N times, and a write it makes under a value it derives PER
//! REQUEST leaves N distinct items; a stream-enabled table then carries exactly one `INSERT`
//! record per newly created item.** Five crates have to agree for that to hold
//! (CloudFormation expands the macro and provisions the function, API Gateway resolves the
//! stage and performs the proxy integration, Lambda runs the handler, DynamoDB holds the
//! table, DynamoDB Streams serves its stream), so the suite lives here.
//!
//! The rows differ along what the rule generalises over: HOW MANY identical requests are
//! issued (one, two and three), HOW the stage is addressed (the api's own `execute-api`
//! host, and each of the three path addressings a client that cannot resolve that host
//! reaches it by), WHICH per-request value the handler keys its write by (the id API Gateway
//! mints for the request, and the id Lambda mints for the invocation), and WHETHER the
//! requests arrive one after another or all at once.
//!
//! Every request is asserted to be ANSWERED within [`ANSWER_DEADLINE`], so a stage that
//! leaves a request open fails here rather than hanging the suite.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{form_encode, physical_ids_of};

/// The Region every request below is signed for and every ARN names.
const REGION: &str = "us-east-1";

/// The `StackStatus` the setup waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The resource path of the api's one method. The stage name is per row: a request that
/// carries the invoke path ALONE names its api by the stage segment, which only names ONE
/// api while no two apis publish a stage of that name.
const RESOURCE_PATH: &str = "visits";

/// How long one invocation of this handler is given, as the function's own `Timeout`.
const FUNCTION_TIMEOUT: u64 = 30;

/// How long to wait between polls of a clock-owed transition, and how many polls to make.
const POLL: Duration = Duration::from_millis(50);
const POLLS: usize = 400;

/// How long ONE request to the deployed stage is given to be answered. The invocation it
/// drives is bounded by the integration's own `timeoutInMillis`, so a request that is still
/// open after this was never going to be answered at all — which is the symptom this suite
/// pins, so it is asserted rather than waited out.
const ANSWER_DEADLINE: Duration = Duration::from_secs(60);

/// The `eventName` a stream record of a newly created item carries.
const INSERT_EVENT: &str = "INSERT";

/// The endpoint a client on the machine this emulator runs on really dials: a host that
/// names no service of it at all.
const LOCAL_ENDPOINT: &str = "localhost:9297";

/// The handler the stack deploys: it writes ONE item keyed by the value it derives for this
/// request — either the id API Gateway mints for the request or the id Lambda mints for the
/// invocation, whichever the row asked for — and answers that value in a well-formed proxy
/// envelope.
const HANDLER_SOURCE: &str = r#"
import json
import os

import boto3


def handler(event, context):
    if os.environ["KEY_SOURCE"] == "lambda":
        visit = context.aws_request_id
    else:
        visit = event["requestContext"]["requestId"]
    boto3.client("dynamodb").put_item(
        TableName=os.environ["TABLE"],
        Item={"id": {"S": visit}, "path": {"S": event.get("path") or ""}},
    )
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps({"visit": visit}),
    }
"#;

/// One answered request: its status and its body verbatim.
struct Answered {
    status: u16,
    body: Bytes,
}

impl Answered {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The emulator as it is ASSEMBLED, on one engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: a control-plane call carries a credential
    // scope so the router can name the target service, and the invoke of the published stage
    // URL carries none, as a client of one sends none.
    router.set_request_authentication(false);
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

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> Answered {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    dispatch(
        router,
        "POST",
        &format!("cloudformation.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("cloudformation")),
        ],
        Bytes::from(body),
    )
    .await
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

/// One AWS Lambda call over its `restJson1` wire.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answered {
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

/// One `awsJson1_0` call under the `dynamodb` signing name: DynamoDB itself, or DynamoDB
/// Streams, told apart by the target prefix.
async fn ddb_rpc(
    router: &MultiServiceRouter,
    prefix: &str,
    operation: &str,
    body: Value,
) -> Answered {
    dispatch(
        router,
        "POST",
        &format!("dynamodb.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("authorization", &authorization("dynamodb")),
            ("x-amz-target", &format!("{prefix}.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await
}

/// One Amazon DynamoDB call that has to answer 200.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let answered = ddb_rpc(router, "DynamoDB_20120810", operation, body).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// One Amazon DynamoDB Streams call that has to answer 200.
async fn streams(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let answered = ddb_rpc(router, "DynamoDBStreams_20120810", operation, body).await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected HTTP 200 from DynamoDB Streams {operation}, got {} with {:?}",
        answered.status,
        answered.text()
    );
    answered.json()
}

/// A required string member of a JSON object.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// Poll `GetFunctionConfiguration` until Lambda first reports the function `Active`, and
/// answer the configuration it reported then.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) -> Value {
    for _ in 0..POLLS {
        let reported = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        let configuration = reported.json();
        if configuration.get("State").and_then(Value::as_str) == Some("Active") {
            return configuration;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup: Lambda never reported {name} Active");
}

/// Poll `DescribeTable` until DynamoDB first reports the table `ACTIVE`, and answer the
/// description it reported then.
async fn wait_until_table_active(router: &MultiServiceRouter, case: &str, table: &str) -> Value {
    for _ in 0..POLLS {
        let described = dynamodb(router, case, "DescribeTable", json!({ "TableName": table })).await;
        let reported = described
            .get("Table")
            .cloned()
            .unwrap_or(Value::Null);
        if reported.get("TableStatus").and_then(Value::as_str) == Some("ACTIVE") {
            return reported;
        }
        tokio::time::sleep(POLL).await;
    }
    panic!("setup {case:?}: DynamoDB never reported {table} ACTIVE");
}

/// The SAM template the suite deploys: the handler above, named at the table it writes and
/// at the per-request value it keys by, through the environment variables the transform
/// carries through.
fn template(table: &str, key_source: &str) -> String {
    json!({
        "Transform": "AWS::Serverless-2016-10-31",
        "Resources": {
            "Writer": {
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": FUNCTION_TIMEOUT,
                    "InlineCode": HANDLER_SOURCE,
                    "Environment": {
                        "Variables": { "TABLE": table, "KEY_SOURCE": key_source }
                    },
                    "Policies": [{ "DynamoDBCrudPolicy": { "TableName": table } }]
                }
            }
        }
    })
    .to_string()
}

/// Which per-request value the handler of a row keys its write by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeySource {
    /// The id API Gateway mints for the request it is answering.
    ApiGatewayRequestId,
    /// The id Lambda mints for the invocation serving it.
    LambdaRequestId,
}

impl KeySource {
    /// What the handler reads this choice out of its environment as.
    fn as_env(self) -> &'static str {
        match self {
            Self::ApiGatewayRequestId => "apigateway",
            Self::LambdaRequestId => "lambda",
        }
    }
}

/// How a row addresses the deployed stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addressing {
    /// `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}/{path}` — the URI the
    /// api's own endpoint is published at.
    ExecuteApiHost,
    /// The reserved data-plane path a client on this machine reaches the same stage by,
    /// because nothing resolves that host here.
    DataPlanePath,
    /// That same reserved path on the endpoint a client on this machine really dials, whose
    /// host names no service at all.
    LocalDataPlanePath,
    /// The invoke path ALONE on that endpoint: the stage segment is all such a request
    /// carries that names a deployed stage.
    LocalStageOnlyPath,
}

/// Whether a row's requests are issued one after another or all at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrival {
    /// One request at a time, each answered before the next is sent.
    OneAfterAnother,
    /// Every request in flight at once — [`AT_ONCE`] of them, which is what makes a request
    /// that only completes because nothing else is in flight fail here.
    AllAtOnce,
}

/// How many requests an [`Arrival::AllAtOnce`] row issues.
const AT_ONCE: usize = 3;

/// One row: how the stage is addressed, which per-request value the handler keys by, how
/// many identical requests are issued, and whether they arrive one at a time or together.
struct Row {
    case: &'static str,
    addressing: Addressing,
    key_source: KeySource,
    requests: usize,
    arrival: Arrival,
}

const ROWS: &[Row] = &[
    Row {
        case: "one request on the api's own execute-api host",
        addressing: Addressing::ExecuteApiHost,
        key_source: KeySource::ApiGatewayRequestId,
        requests: 1,
        arrival: Arrival::OneAfterAnother,
    },
    Row {
        case: "three identical requests on the api's own execute-api host",
        addressing: Addressing::ExecuteApiHost,
        key_source: KeySource::ApiGatewayRequestId,
        requests: 3,
        arrival: Arrival::OneAfterAnother,
    },
    Row {
        case: "two identical requests on the reserved data-plane path",
        addressing: Addressing::DataPlanePath,
        key_source: KeySource::LambdaRequestId,
        requests: 2,
        arrival: Arrival::OneAfterAnother,
    },
    Row {
        case: "three identical requests on the reserved data-plane path of the local endpoint",
        addressing: Addressing::LocalDataPlanePath,
        key_source: KeySource::LambdaRequestId,
        requests: 3,
        arrival: Arrival::OneAfterAnother,
    },
    Row {
        case: "two identical requests carrying the invoke path alone",
        addressing: Addressing::LocalStageOnlyPath,
        key_source: KeySource::ApiGatewayRequestId,
        requests: 2,
        arrival: Arrival::OneAfterAnother,
    },
    Row {
        case: "three identical requests in flight at once",
        addressing: Addressing::ExecuteApiHost,
        key_source: KeySource::LambdaRequestId,
        requests: AT_ONCE,
        arrival: Arrival::AllAtOnce,
    },
];

/// One request to the stage, answered WITHIN [`ANSWER_DEADLINE`]: a stage that leaves the
/// request open rather than answering it fails here instead of hanging.
async fn answer_within(
    router: &MultiServiceRouter,
    case: &str,
    attempt: usize,
    host: &str,
    path: &str,
) -> Answered {
    let request = dispatch(
        router,
        "GET",
        host,
        path,
        &[("accept", "application/json")],
        Bytes::new(),
    );
    match ::tokio::time::timeout(ANSWER_DEADLINE, request).await {
        Ok(answered) => answered,
        Err(_elapsed) => panic!(
            "case {case:?}: request {attempt} to {host}{path} was never answered within \
             {ANSWER_DEADLINE:?}"
        ),
    }
}

/// Every stream record the table's stream holds, read from the beginning of every shard,
/// polled until the stream holds at least `at_least` of them or the polls run out.
///
/// Polled rather than read once, for the reason every other wait here is: the record a write
/// owes its stream is handed over to the streams service by the writing operation, so a read
/// taken the instant after the last request can be a moment early.
///
/// Once the expected number is there the stream is drained ONCE MORE, after a settle span, and
/// that second reading is what is answered — so a SURPLUS record handed over a moment late is
/// in what the caller asserts the exact count over, rather than slipping in behind the first
/// reading that happened to be long enough.
async fn stream_records(
    router: &MultiServiceRouter,
    case: &str,
    stream_arn: &str,
    at_least: usize,
) -> Vec<Value> {
    let mut held: Vec<Value> = Vec::new();
    for _ in 0..POLLS {
        held = drained_records(router, case, stream_arn).await;
        if held.len() >= at_least {
            ::tokio::time::sleep(POLL).await;
            return drained_records(router, case, stream_arn).await;
        }
        ::tokio::time::sleep(POLL).await;
    }
    held
}

/// One pass over every shard of the stream, from its beginning.
async fn drained_records(router: &MultiServiceRouter, case: &str, stream_arn: &str) -> Vec<Value> {
    let described = streams(
        router,
        case,
        "DescribeStream",
        json!({ "StreamArn": stream_arn }),
    )
    .await;
    let shards = described["StreamDescription"]["Shards"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut held: Vec<Value> = Vec::new();
    for shard in shards {
        let shard_id = shard["ShardId"].as_str().unwrap_or_default().to_string();
        let iterator = streams(
            router,
            case,
            "GetShardIterator",
            json!({
                "StreamArn": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        let mut cursor = iterator["ShardIterator"].as_str().map(str::to_string);
        // The stream is drained a page at a time, bounded: a shard answers its records and
        // then an empty page, which is where this stops.
        for _ in 0..POLLS {
            let Some(at) = cursor.clone() else { break };
            let page = streams(
                router,
                case,
                "GetRecords",
                json!({ "ShardIterator": at }),
            )
            .await;
            let records = page["Records"].as_array().cloned().unwrap_or_default();
            let drained = records.is_empty();
            held.extend(records);
            cursor = page["NextShardIterator"].as_str().map(str::to_string);
            if drained {
                break;
            }
        }
    }
    held
}

// The emulator serves on a MULTI-THREADED runtime (`core/router/src/main.rs`), and an
// invoke drives two nested in-process calls on it (the proxy integration's `Invoke`, and the
// handler's own SDK call back into this router), so the suite runs on that flavour too.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeat_requests_to_a_deployed_stage_each_answer_and_each_leave_one_item_and_one_insert() {
    // ONE emulator for every row: each row builds a table, a function and an api of its own,
    // so what a row observes is that row's alone.
    let mut router = emulator();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;

        // 1. The table the handler writes, with its stream on, and the stage this row's
        // deployment publishes.
        let table = format!("visits-{index}");
        let stage = format!("live{index}");
        dynamodb(
            &router,
            case,
            "CreateTable",
            json!({
                "TableName": table,
                "KeySchema": [{ "AttributeName": "id", "KeyType": "HASH" }],
                "AttributeDefinitions": [{ "AttributeName": "id", "AttributeType": "S" }],
                "BillingMode": "PAY_PER_REQUEST",
                "StreamSpecification": {
                    "StreamEnabled": true,
                    "StreamViewType": "NEW_AND_OLD_IMAGES",
                },
            }),
        )
        .await;
        let described = wait_until_table_active(&router, case, &table).await;
        let stream_arn = member("DescribeTable", &described, "LatestStreamArn");

        // 2. The function that writes it, one item per invocation.
        let stack = format!("repeat-visits-{index}");
        let body = template(&table, row.key_source.as_env());
        let created = cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &stack),
                ("TemplateBody", &body),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Capabilities.member.2", "CAPABILITY_AUTO_EXPAND"),
            ],
        )
        .await;
        assert_eq!(
            created.status, 200,
            "setup {case:?}: expected HTTP 200 from CreateStack, got {} with {:?}",
            created.status,
            created.text()
        );
        let (reached, description) = wait_for_stack_status(&router, &stack, CREATE_COMPLETE).await;
        assert_eq!(
            reached, CREATE_COMPLETE,
            "setup {case:?}: expected the stack to reach {CREATE_COMPLETE}, got {description}"
        );
        let resources =
            cloudformation(&router, "DescribeStackResources", &[("StackName", &stack)]).await;
        let function_name = physical_ids_of(&resources.text())
            .get("Writer")
            .cloned()
            .unwrap_or_else(|| panic!("setup {case:?}: no physical id for Writer"));
        let configuration = wait_until_active(&router, &function_name).await;
        let function_arn = member("GetFunctionConfiguration", &configuration, "FunctionArn");

        // The grant that admits an invocation from API Gateway.
        let granted = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function_name}/policy"),
            Some(json!({
                "StatementId": "apigw-invoke",
                "Action": "lambda:InvokeFunction",
                "Principal": "apigateway.amazonaws.com",
            })),
        )
        .await;
        assert_eq!(
            granted.status, 201,
            "setup {case:?}: expected HTTP 201 from AddPermission, got {} with {:?}",
            granted.status,
            granted.text()
        );

        // 3. The api: GET /visits behind an AWS_PROXY integration on that function, deployed.
        let api_record = apigw(
            &router,
            case,
            "POST",
            "/restapis",
            Some(json!({ "name": format!("repeat-api-{index}") })),
            201,
        )
        .await;
        let api = member("CreateRestApi", &api_record, "id");
        let root = member("CreateRestApi", &api_record, "rootResourceId");
        let resource = apigw(
            &router,
            case,
            "POST",
            &format!("/restapis/{api}/resources/{root}"),
            Some(json!({ "pathPart": RESOURCE_PATH })),
            201,
        )
        .await;
        let resource_id = member("CreateResource", &resource, "id");
        let method = format!("/restapis/{api}/resources/{resource_id}/methods/GET");
        apigw(
            &router,
            case,
            "PUT",
            &method,
            Some(json!({ "authorizationType": "NONE" })),
            201,
        )
        .await;
        apigw(
            &router,
            case,
            "PUT",
            &format!("{method}/integration"),
            Some(json!({
                "type": "AWS_PROXY",
                "httpMethod": "POST",
                "uri": format!(
                    "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions/\
                     {function_arn}/invocations"
                ),
            })),
            201,
        )
        .await;
        apigw(
            &router,
            case,
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "stageName": stage })),
            201,
        )
        .await;

        // 4. The repeated requests: each one is answered, and each answer names its own
        // request. They are issued with the front door AUTHENTICATING, as the emulator's own
        // boot configures it, and carrying no credential, as a client of a method whose
        // `authorizationType` is `NONE` sends none: an invoke of a deployed stage is served
        // whatever credential it carries.
        router.set_request_authentication(true);
        let (host, path) = match row.addressing {
            Addressing::ExecuteApiHost => (
                format!("{api}.execute-api.{REGION}.amazonaws.com"),
                format!("/{stage}/{RESOURCE_PATH}"),
            ),
            Addressing::DataPlanePath => (
                format!("apigateway.{REGION}.amazonaws.com"),
                format!("/_aws/execute-api/{api}/{stage}/{RESOURCE_PATH}"),
            ),
            Addressing::LocalDataPlanePath => (
                LOCAL_ENDPOINT.to_string(),
                format!("/_aws/execute-api/{api}/{stage}/{RESOURCE_PATH}"),
            ),
            Addressing::LocalStageOnlyPath => (
                LOCAL_ENDPOINT.to_string(),
                format!("/{stage}/{RESOURCE_PATH}"),
            ),
        };
        let served = match row.arrival {
            Arrival::OneAfterAnother => {
                let mut one_at_a_time: Vec<Answered> = Vec::new();
                for attempt in 0..row.requests {
                    one_at_a_time.push(answer_within(&router, case, attempt, &host, &path).await);
                }
                one_at_a_time
            }
            // Three in flight at once, on the one router: a stage that answers only while
            // nothing else is in flight never gets past this.
            Arrival::AllAtOnce => {
                assert_eq!(
                    row.requests, AT_ONCE,
                    "case {case:?}: an all-at-once row issues {AT_ONCE} requests, not {}",
                    row.requests
                );
                let (first, second, third) = ::tokio::join!(
                    answer_within(&router, case, 0, &host, &path),
                    answer_within(&router, case, 1, &host, &path),
                    answer_within(&router, case, 2, &host, &path),
                );
                vec![first, second, third]
            }
        };
        let mut answered: Vec<String> = Vec::new();
        for (attempt, served) in served.iter().enumerate() {
            assert_eq!(
                served.status, 200,
                "case {case:?}: request {attempt} of {} must be answered HTTP 200, got {} \
                 with {:?}",
                row.requests,
                served.status,
                served.text()
            );
            let visit = served
                .json()
                .get("visit")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                !visit.is_empty(),
                "case {case:?}: request {attempt} must be answered the handler's own body, \
                 got {:?}",
                served.text()
            );
            answered.push(visit);
        }
        let distinct: BTreeSet<&String> = answered.iter().collect();
        assert_eq!(
            distinct.len(),
            row.requests,
            "case {case:?}: expected {} distinct per-request values, got {answered:?}",
            row.requests
        );
        // The reads below are this suite's own, not a client of the stage's, so they go back
        // to carrying the credential scope that routes them.
        router.set_request_authentication(false);

        // 5. One item per request, each under the value that request derived.
        let scanned = dynamodb(
            &router,
            case,
            "Scan",
            json!({ "TableName": table, "ConsistentRead": true }),
        )
        .await;
        let items = scanned["Items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            row.requests,
            "case {case:?}: expected {} item(s) in {table:?}, one per request, got {items:?}",
            row.requests
        );
        let keys: BTreeSet<String> = items
            .iter()
            .filter_map(|item| item["id"]["S"].as_str())
            .map(str::to_string)
            .collect();
        assert_eq!(
            keys,
            answered.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: expected the items to be keyed by the values the requests were \
             answered with, got {items:?}"
        );

        // 6. One INSERT record per newly created item, and nothing for a write that created
        // none.
        let records = stream_records(&router, case, &stream_arn, row.requests).await;
        let inserts: Vec<&Value> = records
            .iter()
            .filter(|record| record["eventName"].as_str() == Some(INSERT_EVENT))
            .collect();
        assert_eq!(
            inserts.len(),
            row.requests,
            "case {case:?}: expected {} {INSERT_EVENT} record(s) on the table's stream, one \
             per new item, got {records:?}",
            row.requests
        );
        assert_eq!(
            records.len(),
            row.requests,
            "case {case:?}: expected the stream to hold only the {INSERT_EVENT} records of \
             the new items, got {records:?}"
        );
    }
}
