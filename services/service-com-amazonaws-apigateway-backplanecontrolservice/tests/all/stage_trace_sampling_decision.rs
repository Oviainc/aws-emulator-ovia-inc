// SPDX-License-Identifier: CC0-1.0

//! What a deployed REST stage does with the TRACE CONTEXT a caller supplied: API Gateway,
//! AWS Lambda, Amazon DynamoDB and AWS X-Ray on ONE engine, driven over the stage's own
//! invoke URL.
//!
//! The rule under test: **a tracing stage SERVES every request it routes whatever trace
//! context the caller supplied — continuing the chain the caller named where it named one in
//! X-Ray's own form, starting one of its own where it named none or named text that is not a
//! trace id, and naming the span of its own call out as the `Parent` — and the sampling
//! decision governs only what is RECORDED, never whether the request's work happens: an
//! unsampled request is served and does its work, and nothing is recorded for its chain, so
//! that chain is held by neither the trace read (`BatchGetTraces`) nor the trace search
//! (`GetTraceSummaries`).**
//!
//! A malformed context is therefore never fatal to a request, and `Sampled=0` is never an
//! excuse not to serve one: both arms answer `200` with the handler's own body and both leave
//! the item the handler wrote behind.
//!
//! The rows differ along what the rule generalises over: the FORM of the caller's header (a
//! chain of the documented form, a chain naming the caller's own parent span, a root that is
//! not a trace id, no header at all) crossed with the DECISION beside it (`Sampled=1`,
//! `Sampled=0`, and no decision at all). Each row asserts all three halves — the response the
//! client was served, the item the integration's work left in DynamoDB, and what X-Ray holds
//! and indexes for the chain the handler ran in.
//!
//! Only a cross-service suite can pin this: the stamping and the stage's own segment are API
//! Gateway's, the context the handler reads out of its environment is AWS Lambda's, the item
//! the work left is Amazon DynamoDB's, and the chain is read back where a client reads it,
//! through AWS X-Ray's own `BatchGetTraces` and `GetTraceSummaries`.
//!
//! The wire harness ([`dispatch`], [`apigw`], [`lambda`], [`package`]) is the sibling
//! `proxy_binary_media_types` suite's and the read of a held chain is
//! [`crate::stage_trace_context::documents_of`], both reused rather than stood up a second
//! way; what this suite adds is its own engine — the one that also mounts DynamoDB, because
//! the work an unsampled request still does is what it has to read back — and the two halves
//! no other suite asserts: that the work HAPPENED, and what the trace SEARCH indexes. The
//! whole table is driven through ONE test function, so this suite adds one concurrent
//! interpreter to its target rather than seven.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::proxy_binary_media_types::{
    apigw, authorization, dispatch, lambda, member, package, wait_until_active, Answer, HANDLER,
    REGION, ROLE,
};
use crate::stage_trace_context::documents_of;

/// The stage every row's request is addressed to. It is the one stage, and it traces: what
/// tells the rows apart is the caller's header, not the stage.
const STAGE: &str = "traced";

/// The resource path the one method of the api is bound at.
const RESOURCE_PATH: &str = "x";

/// The function the method is integrated with, and the table its work writes.
const FUNCTION: &str = "trace-sampling-probe";
const TABLE: &str = "trace-sampling-work";

/// The `origin` of the segment a REST API stage records, as X-Ray names an API Gateway stage.
const STAGE_ORIGIN: &str = "AWS::ApiGateway::Stage";

/// How far the epoch a MINTED trace id names may sit from this suite's own clock. The id's
/// time part is "the time of the original request, in Unix epoch time, in 8 hexadecimal
/// digits", so the instant it names is the one the stage was serving — allowed a wide margin
/// here because what is under test is that the id names THIS request's time rather than any
/// particular second of it.
const MINTED_WITHIN_SECONDS: u64 = 300;

/// How wide a window the trace search below is made over, either side of the run: wide enough
/// to cover every row, and well inside the bound `GetTraceSummaries` publishes for a window
/// and for how far ahead of the clock one may end.
const WINDOW_SECONDS: u64 = 600;

/// How many pages of trace summaries this suite will walk before calling the paging broken.
const PAGE_LIMIT: usize = 32;

/// The handler every row runs: it reports the trace context its own environment carries, and
/// it DOES THE WORK — one item per row, keyed by the row's own arm — so the item left behind
/// is evidence the request was served whatever the decision on its chain was.
const HANDLER_SOURCE: &str = r#"
import json
import os

import boto3


def lambda_handler(event, context):
    raw = os.environ.get("_X_AMZN_TRACE_ID") or ""
    fields = {}
    for field in raw.split(";"):
        name, _, value = field.partition("=")
        if name:
            fields[name] = value
    arm = (event.get("queryStringParameters") or {}).get("k") or "none"
    boto3.client("dynamodb").put_item(
        TableName=os.environ["TABLE"],
        Item={
            "pk": {"S": arm},
            "root": {"S": fields.get("Root") or ""},
            "parent": {"S": fields.get("Parent") or ""},
            "sampled": {"S": fields.get("Sampled") or ""},
        },
    )
    return {
        "statusCode": 200,
        "headers": {"Content-Type": "application/json"},
        "body": json.dumps(
            {
                "raw_trace_header": raw,
                "root": fields.get("Root"),
                "parent": fields.get("Parent"),
                "sampled": fields.get("Sampled"),
                "arm": arm,
            }
        ),
    }
"#;

/// The four services this rule spans, over ONE engine, as `core/router/src/lib.rs` mounts
/// them: the invocation the stage makes, the item its work writes and the segments both hops
/// record are served by the very Lambda, DynamoDB and X-Ray this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the services, not SigV4: a control-plane call carries a credential
    // scope so the router can name the target service, and the invoke of the published stage
    // URL carries none, as a client of one sends none.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(
        engine,
    ));
    router
}

/// One Amazon DynamoDB call over its `awsJson1_0` wire.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let answered = dispatch(
        router,
        "POST",
        &format!("dynamodb.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("authorization", &authorization("dynamodb")),
            ("x-amz-target", &format!("DynamoDB_20120810.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await;
    (answered.status, answered.json())
}

/// One AWS X-Ray call over its `restJson1` wire.
async fn xray(router: &MultiServiceRouter, path: &str, body: Value) -> Answer {
    dispatch(
        router,
        "POST",
        &format!("xray.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("xray")),
        ],
        Bytes::from(body.to_string()),
    )
    .await
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com` — carrying no credential at all, as a
/// client of a public method sends none.
async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Answer {
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

/// Every chain the trace SEARCH reports over `[start, end)`, walking its pages — the
/// operation a client finds a trace by its window with (`POST /TraceSummaries`).
async fn indexed_chains(router: &MultiServiceRouter, start: u64, end: u64) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut token: Option<String> = None;
    for _ in 0..PAGE_LIMIT {
        let mut body = json!({ "StartTime": start, "EndTime": end });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let answered = xray(router, "/TraceSummaries", body).await;
        assert_eq!(
            answered.status,
            200,
            "expected HTTP 200 from GetTraceSummaries, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let page = answered.json();
        for summary in page
            .get("TraceSummaries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(id) = summary.get("Id").and_then(Value::as_str) {
                found.insert(id.to_string());
            }
        }
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return found;
        }
    }
    panic!("GetTraceSummaries kept issuing a NextToken past {PAGE_LIMIT} pages");
}

/// The instant a trace id names: its time part is "the time of the original request, in Unix
/// epoch time, in 8 hexadecimal digits".
fn minted_at(trace_id: &str) -> Option<u64> {
    u64::from_str_radix(trace_id.split('-').nth(1)?, 16).ok()
}

/// This suite's own clock, in epoch seconds.
fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

/// The table every row's work writes its item into, waited out to `ACTIVE` ("you can perform
/// read and write operations only on an ACTIVE table").
async fn given_table(router: &MultiServiceRouter) {
    let (status, created) = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup CreateTable: expected HTTP 200, got {status} with {created}"
    );
    for _ in 0..400 {
        let (_, described) = dynamodb(router, "DescribeTable", json!({ "TableName": TABLE })).await;
        if described.pointer("/Table/TableStatus").and_then(Value::as_str) == Some("ACTIVE") {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("setup CreateTable: the table never became ACTIVE");
}

/// The `TracingConfig` `Active` function every row's request reaches, with the grant that
/// admits an invocation from API Gateway, waited out to `Active`. Answers its ARN.
async fn given_function(router: &MultiServiceRouter) -> String {
    let created = lambda(
        router,
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 30,
            "TracingConfig": { "Mode": "Active" },
            "Environment": { "Variables": { "TABLE": TABLE } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        })),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup CreateFunction: expected HTTP 201, got {} with {:?}",
        created.status,
        created.text()
    );
    let arn = member("CreateFunction", &created.json(), "FunctionArn");
    let granted = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        Some(json!({
            "StatementId": "apigw-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "apigateway.amazonaws.com",
        })),
    )
    .await;
    assert_eq!(
        granted.status,
        201,
        "setup AddPermission: expected HTTP 201, got {} with {:?}",
        granted.status,
        granted.text()
    );
    wait_until_active(router, FUNCTION).await;
    arn
}

/// The api every row's request reaches: one `GET /{RESOURCE_PATH}` method behind an
/// `AWS_PROXY` integration onto `function_arn`, deployed to [`STAGE`], which traces.
async fn given_api(router: &MultiServiceRouter, function_arn: &str) -> String {
    let created = apigw(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "trace-sampling-api" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    let resource = apigw(
        router,
        "CreateResource",
        "POST",
        &format!("/restapis/{api}/resources/{root}"),
        Some(json!({ "pathPart": RESOURCE_PATH })),
        201,
    )
    .await;
    let resource_id = member("CreateResource", &resource, "id");
    let method = format!("/restapis/{api}/resources/{resource_id}/methods/GET");
    apigw(
        router,
        "PutMethod",
        "PUT",
        &method,
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw(
        router,
        "PutIntegration",
        "PUT",
        &format!("{method}/integration"),
        Some(json!({
            "type": "AWS_PROXY",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports.
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
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;
    // "Specifies whether active tracing with X-ray is enabled for the Stage"
    // (`smithy_query(query="doc", shape="Stage", member="tracingEnabled")`): the one record
    // that decides whether the stage records anything at all.
    apigw(
        router,
        "UpdateStage",
        "PATCH",
        &format!("/restapis/{api}/stages/{STAGE}"),
        Some(json!({
            "patchOperations": [
                { "op": "replace", "path": "/tracingEnabled", "value": "true" }
            ]
        })),
        200,
    )
    .await;
    api
}

/// A chain of X-Ray's own form (`1-<8 hex>-<24 hex>`), and a span id of its own form, that a
/// caller supplies. One chain per row that supplies one, so no two rows share a chain.
const SAMPLED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272711";
const UNSAMPLED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272712";
const UNDECIDED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272713";
const PARENTED_CHAIN: &str = "1-5759e988-bd862e3fe1be46a994272714";
const CALLER_SPAN: &str = "53995c3f42cd8ad8";

/// One row: what the caller sends, and what must follow for it.
struct Row {
    case: &'static str,
    /// The arm the row's work is keyed by, which is also the `k` its request carries.
    arm: &'static str,
    /// The `X-Amzn-Trace-Id` the caller sends, or `None` for a caller that sends none.
    sent: Option<&'static str>,
    /// The chain the handler must see, or `None` where the stage starts one of its own.
    chain: Option<&'static str>,
    /// The sampling decision the handler must see. Where the caller took none, it is the
    /// stage's own — a stage that records its own work records this request — so every row
    /// whose header carries no `Sampled` expects the recorded decision.
    decision: &'static str,
    /// Whether ANYTHING is recorded for the chain this request ran in.
    recorded: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a chain the caller named, sampled",
        arm: "sampled-chain",
        sent: Some("Root=1-5759e988-bd862e3fe1be46a994272711;Sampled=1"),
        chain: Some(SAMPLED_CHAIN),
        decision: "1",
        recorded: true,
    },
    Row {
        case: "a chain the caller named, UNSAMPLED: the work still happens, nothing is \
               recorded",
        arm: "unsampled-chain",
        sent: Some("Root=1-5759e988-bd862e3fe1be46a994272712;Sampled=0"),
        chain: Some(UNSAMPLED_CHAIN),
        decision: "0",
        recorded: false,
    },
    Row {
        case: "a chain the caller named with no decision: the stage takes its own",
        arm: "undecided-chain",
        sent: Some("Root=1-5759e988-bd862e3fe1be46a994272713"),
        chain: Some(UNDECIDED_CHAIN),
        decision: "1",
        recorded: true,
    },
    Row {
        case: "a chain naming the caller's own parent span, unsampled",
        arm: "unsampled-parented-chain",
        sent: Some(
            "Root=1-5759e988-bd862e3fe1be46a994272714;Parent=53995c3f42cd8ad8;Sampled=0",
        ),
        chain: Some(PARENTED_CHAIN),
        decision: "0",
        recorded: false,
    },
    Row {
        case: "no header at all: the stage starts a chain and records it",
        arm: "no-header",
        sent: None,
        chain: None,
        decision: "1",
        recorded: true,
    },
    Row {
        case: "a root that is not a trace id, sampled: replaced rather than fatal",
        arm: "malformed-root-sampled",
        sent: Some("Root=not-a-trace-id;Sampled=1"),
        chain: None,
        decision: "1",
        recorded: true,
    },
    Row {
        case: "a root that is not a trace id, unsampled: replaced, and still not recorded",
        arm: "malformed-root-unsampled",
        sent: Some("Root=not-a-trace-id;Sampled=0"),
        chain: None,
        decision: "0",
        recorded: false,
    },
];

/// The item the row's work left behind, or `None` where nothing is stored under its arm —
/// which is what work that never happened leaves.
async fn work_of(router: &MultiServiceRouter, case: &str, arm: &str) -> Option<Value> {
    let (status, read) = dynamodb(
        router,
        "GetItem",
        json!({ "TableName": TABLE, "Key": { "pk": { "S": arm } }, "ConsistentRead": true }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetItem for {arm:?}, got {status} with {read}"
    );
    read.get("Item").cloned()
}

#[tokio::test]
async fn the_sampling_decision_governs_what_is_recorded_not_whether_the_request_is_served() {
    let router = emulator();
    given_table(&router).await;
    let function_arn = given_function(&router).await;
    let api = given_api(&router, &function_arn).await;

    let window_start = now_epoch_seconds().saturating_sub(WINDOW_SECONDS);
    let mut observed: Vec<(&str, String, bool)> = Vec::new();

    for row in ROWS {
        let case = row.case;
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(sent) = row.sent {
            headers.push(("x-amzn-trace-id", sent));
        }
        let served = invoke(
            &router,
            &api,
            &format!("/{RESOURCE_PATH}?k={}", row.arm),
            &headers,
        )
        .await;
        // Served, whatever the caller supplied: neither a malformed context nor a decision
        // not to record it is an excuse to answer anything but the handler's own envelope.
        assert_eq!(
            served.status,
            200,
            "case {case:?}: expected HTTP 200 from the stage, got {} with {:?}",
            served.status,
            served.text()
        );
        let reported = served.json();
        assert_eq!(
            reported.get("arm").and_then(Value::as_str),
            Some(row.arm),
            "case {case:?}: expected the handler's own body for this arm, got {reported}"
        );

        // The CHAIN: continued where the caller named one of the documented form, and started
        // by the stage where it named none or named text that is not a trace id.
        let root = reported
            .get("root")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            ::service_common::xray_trace::is_trace_id(&root),
            "case {case:?}: expected the handler to see a trace id as its Root, got {reported}"
        );
        match row.chain {
            Some(continued) => assert_eq!(
                root, continued,
                "case {case:?}: expected the caller's chain continued, got {reported}"
            ),
            None => {
                assert!(
                    ROWS.iter().all(|other| other.chain != Some(root.as_str())),
                    "case {case:?}: expected a chain the stage started rather than one \
                     another row named, got {reported}"
                );
                let named = minted_at(&root).unwrap_or_default();
                let now = now_epoch_seconds();
                assert!(
                    now.abs_diff(named) <= MINTED_WITHIN_SECONDS,
                    "case {case:?}: expected a minted chain naming this request's own time \
                     within {MINTED_WITHIN_SECONDS}s of {now}, got {named} in {reported}"
                );
            }
        }

        // The PARENT: the span of the stage's own call out, minted for this call, never the
        // span the caller named as the parent of the stage's work.
        let parent = reported
            .get("parent")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            ::service_common::xray_trace::is_span_id(&parent),
            "case {case:?}: expected a minted span id as the handler's Parent, got {reported}"
        );
        assert_ne!(
            parent, CALLER_SPAN,
            "case {case:?}: expected the stage's own span rather than the caller's, got \
             {reported}"
        );

        // The DECISION: the caller's where it took one, and the tracing stage's where it did
        // not — and it travels to the integration as written.
        assert_eq!(
            reported.get("sampled").and_then(Value::as_str),
            Some(row.decision),
            "case {case:?}: expected the decision {:?} to reach the handler, got {reported}",
            row.decision
        );

        // The WORK: it happened for every row, whatever the decision, and it ran in the
        // context the response reported.
        let held = work_of(&router, case, row.arm).await;
        let item = held.unwrap_or(Value::Null);
        assert_eq!(
            item.pointer("/root/S").and_then(Value::as_str),
            Some(root.as_str()),
            "case {case:?}: expected the integration's work to have happened in the chain \
             the response reported, got {item}"
        );
        assert_eq!(
            item.pointer("/sampled/S").and_then(Value::as_str),
            Some(row.decision),
            "case {case:?}: expected the work to have happened under the decision {:?}, got \
             {item}",
            row.decision
        );

        // What X-RAY HOLDS for that chain: the recorded rows have the stage's own segment,
        // and an unrecorded one has no document at all.
        let documents = documents_of(&router, &root).await;
        let origins: Vec<&str> = documents
            .iter()
            .filter_map(|document| document.get("origin").and_then(Value::as_str))
            .collect();
        if row.recorded {
            assert!(
                origins.contains(&STAGE_ORIGIN),
                "case {case:?}: expected a {STAGE_ORIGIN} segment under {root}, got \
                 {documents:?}"
            );
        } else {
            assert!(
                documents.is_empty(),
                "case {case:?}: expected nothing recorded under {root}, got {documents:?}"
            );
        }
        observed.push((case, root, row.recorded));
    }

    // Every chain is DISTINCT: one request is one chain, so no two rows report the same one.
    let chains: BTreeSet<&str> = observed.iter().map(|(_, root, _)| root.as_str()).collect();
    assert_eq!(
        chains.len(),
        observed.len(),
        "expected one chain per request, got {observed:?}"
    );

    // And what the trace SEARCH indexes over a window covering the whole run: the recorded
    // chains, and none of the unrecorded ones — which is what tells a client that is not
    // sampled from one that never ran.
    let indexed = indexed_chains(&router, window_start, now_epoch_seconds() + WINDOW_SECONDS).await;
    for (case, root, recorded) in &observed {
        assert_eq!(
            indexed.contains(root),
            *recorded,
            "case {case:?}: expected {root} to be indexed by GetTraceSummaries={recorded}, \
             got {indexed:?}"
        );
    }
}
