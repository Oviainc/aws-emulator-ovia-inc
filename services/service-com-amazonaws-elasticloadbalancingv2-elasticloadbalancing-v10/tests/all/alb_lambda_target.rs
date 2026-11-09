// SPDX-License-Identifier: CC0-1.0

//! A `forward` to a `lambda` TARGET GROUP, end to end over the wire: what a client that
//! sends traffic to the name a load balancer publishes gets back when the group behind the
//! listener registers a Lambda function.
//!
//! The rule under test: **a request an Application Load Balancer forwards to a `lambda`
//! target group is served by INVOKING the function the registration names, and the client is
//! answered from the object that function returned only while that object satisfies the
//! response contract — a readable status code, headers and a body of the shapes the contract
//! gives them, within the response size a Lambda target may answer with. A function that
//! breaks the contract, or whose run errored, is answered with the load balancer's own HTTP
//! 502 and its payload is never proxied; the function still RAN, and neither the
//! registration nor its health changes.**
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router to
//! the published `DNSName`, carrying no AWS credential, exactly as a client sends it — and
//! the function is a real deployment package created through Lambda's OWN API and RUN by the
//! Lambda emulator mounted on this suite's engine. Nothing here is served from a record a
//! test invented.
//!
//! That the function ran is asserted where nothing of this service can fake it: the handler
//! reports every invocation to an Amazon SQS queue, through its own SDK, before it decides
//! what to return — so the queue's contents after a row are a consequence of the handler
//! having been reached. A `502` row that never invoked anything leaves the queue empty and
//! fails.
//!
//! The rows differ along what the rule generalises over: the CLASS of contract violation (no
//! status code, a status outside the HTTP range, a payload that is no response object at all,
//! a member of the wrong shape, a response past the size limit, a run that raised), the shape
//! of a well-formed answer (a text body, no body at all, a base64-encoded body, a status
//! other than 200), and the request the event is built from (the verb, the path, a repeated
//! query parameter, a body). Two further cases bracket the table with the GRANT the
//! invocation is made under: the function's own resource-based policy, removed and added
//! again.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::state::ComAmazonawsElasticloadbalancingv2ElasticloadbalancingV10State as ElbState;
use smithy_server_runtime::MultiServiceRouter;

use crate::alb_data_plane::{create_load_balancer, settle, Answered};
use crate::provisioning_loop::{action, call, members, p, REGION};

/// The function every row runs, and the module and attribute its package carries the handler
/// as.
const FUNCTION: &str = "alb-lambda-target";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The runtime the package is created for.
const RUNTIME: &str = "python3.12";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern. No IAM is
/// mounted here, so nothing is known about it and nothing is refused for it.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The queue the handler reports each invocation to.
const SIDE_CHANNEL: &str = "alb-lambda-side-channel";

/// The body the well-formed answer carries, which is the function's OWN and must never be
/// what a refused response is answered with.
const TARGET_BODY: &str = "TARGET-OK";

/// How long a row waits for work the clock owes before it is a failure, and how long between
/// two looks while waiting.
const BUDGET: Duration = Duration::from_secs(20);
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// How long a case that expects the side channel to stay EMPTY waits before concluding that
/// nothing was invoked: several looks, none of which may find a report.
const QUIET: Duration = Duration::from_millis(250);

/// The handler every row runs. It reports the invocation to the side channel FIRST, so a row
/// that is answered without the handler having run leaves the queue empty, and then decides
/// what to return from the path the event carries — one package serves every row, which is
/// what the rule generalises over.
///
/// The oversized body is built from the limit the load balancer holds a Lambda target's
/// response to, so the row exercises the bound rather than a number guessed at.
const HANDLER_SOURCE: &str = r#"
import base64
import json
import os

import boto3

QUEUE_URL = os.environ["QUEUE_URL"]


def lambda_handler(event, context):
    path = (event.get("path") or "/").lstrip("/")
    boto3.client("sqs").send_message(QueueUrl=QUEUE_URL, MessageBody=path or "/")
    if path == "raise":
        raise RuntimeError("the handler refused to answer")
    if path == "nostatus":
        return {"statusDescription": "200 OK", "body": "TARGET-OK"}
    if path == "nonjson":
        return "not an alb response object at all"
    if path == "badstatus":
        return {"statusCode": 1200, "body": "TARGET-OK"}
    if path == "badheaders":
        return {"statusCode": 200, "headers": ["content-type", "text/plain"]}
    if path == "badbody":
        return {"statusCode": 200, "body": {"not": "text"}}
    if path == "toolarge":
        return {"statusCode": 200, "body": "x" * (1024 * 1024 + 1)}
    if path == "nobody":
        return {"statusCode": 204, "statusDescription": "204 No Content"}
    if path == "created":
        return {
            "statusCode": 201,
            "headers": {"content-type": "application/octet-stream"},
            "body": base64.b64encode(b"TARGET-OK").decode("ascii"),
            "isBase64Encoded": True,
        }
    if path == "echo":
        return {
            "statusCode": 200,
            "headers": {"content-type": "application/json"},
            "body": json.dumps(
                {
                    "httpMethod": event.get("httpMethod"),
                    "path": event.get("path"),
                    "query": event.get("queryStringParameters"),
                    "body": event.get("body"),
                    "isBase64Encoded": event.get("isBase64Encoded"),
                    "proto": (event.get("headers") or {}).get("x-forwarded-proto"),
                    "port": (event.get("headers") or {}).get("x-forwarded-port"),
                    "targetGroupArn": (
                        ((event.get("requestContext") or {}).get("elb") or {})
                        .get("targetGroupArn")
                    ),
                }
            ),
        }
    return {
        "statusCode": 200,
        "statusDescription": "200 OK",
        "headers": {"content-type": "text/plain"},
        "body": "TARGET-OK",
        "isBase64Encoded": False,
    }
"#;

/// A .zip deployment package holding [`HANDLER_SOURCE`] as `lambda_function.py`, written in
/// memory the way a caller uploads one.
fn package() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        writer
            .start_file("lambda_function.py", options)
            .expect("the entry starts");
        writer
            .write_all(HANDLER_SOURCE.as_bytes())
            .expect("the entry's bytes");
        writer.finish().expect("a finished archive");
    }
    buffer.into_inner()
}

/// This service, Lambda and Amazon SQS over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the function the load balancer forwards to is run by that Lambda, and the
/// handler's own SDK call is dispatched back into this same router, so the queue it reports
/// to is the queue this suite reads.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// One request to the name the load balancer published, carrying no credential, as a client
/// of that name sends it.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    target: &str,
    body: &str,
) -> Answered {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://{host}{target}"))
        .header("host", host)
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("the data-plane request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
        })
        .collect();
    let body = String::from_utf8_lossy(&response.into_body()).into_owned();
    Answered {
        status,
        headers,
        body,
    }
}

/// One Lambda `restJson1` call.
async fn lambda(router: &MultiServiceRouter, method: &str, path: &str, body: Value) -> (u16, Value) {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(&body).expect("the request body serializes")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://lambda.{REGION}.amazonaws.com{path}"))
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/lambda/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One Amazon SQS call (`POST /` with `X-Amz-Target: AmazonSQS.<Operation>`), the way a
/// client makes it.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/sqs/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Every message the side channel holds right now, deleted as it is read, so the next row
/// starts from an empty queue.
///
/// One wait for the FIRST message, then one more look, so a row that reported two
/// invocations is seen to have reported two.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    budget: Duration,
) -> Vec<String> {
    let mut seen = Vec::new();
    let deadline = Instant::now() + budget;
    loop {
        let (status, received) = sqs(
            router,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ReceiveMessage to answer 200, got {status} {received}"
        );
        let batch = received["Messages"].as_array().cloned().unwrap_or_default();
        for message in &batch {
            seen.push(
                message["Body"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
            let handle = message["ReceiptHandle"].as_str().unwrap_or_default();
            let (status, deleted) = sqs(
                router,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteMessage to answer 200, got {status} {deleted}"
            );
        }
        if !seen.is_empty() && batch.is_empty() {
            return seen;
        }
        if Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// What one row asks of the load balancer, and what the records earn it.
struct Row {
    case: &'static str,
    verb: &'static str,
    /// The path, query string included, within the load balancer's own endpoint.
    target: &'static str,
    /// The request body the client sends.
    body: &'static str,
    /// The status the load balancer must answer with.
    status: u16,
    /// The body it must answer with, when the row pins one. A refused response pins the
    /// empty body the load balancer answers its own statuses with.
    expect_body: Option<&'static str>,
    /// The media type the answer must declare, when the row pins one.
    content_type: Option<&'static str>,
}

/// Every row's answer must come from the records, and every row must have invoked the
/// function exactly once.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a well-formed response: the function's own status, media type and body",
            verb: "GET",
            target: "/ok",
            body: "",
            status: 200,
            expect_body: Some(TARGET_BODY),
            content_type: Some("text/plain"),
        },
        Row {
            case: "a response naming no body, which the contract allows",
            verb: "GET",
            target: "/nobody",
            body: "",
            status: 204,
            expect_body: Some(""),
            content_type: None,
        },
        Row {
            case: "a base64-encoded body under another status: the bytes it names",
            verb: "GET",
            target: "/created",
            body: "",
            status: 201,
            expect_body: Some(TARGET_BODY),
            content_type: Some("application/octet-stream"),
        },
        Row {
            case: "a response naming no statusCode",
            verb: "GET",
            target: "/nostatus",
            body: "",
            status: 502,
            expect_body: Some(""),
            content_type: None,
        },
        Row {
            case: "a payload that is no response object at all",
            verb: "GET",
            target: "/nonjson",
            body: "",
            status: 502,
            expect_body: Some(""),
            content_type: None,
        },
        Row {
            case: "a statusCode outside the HTTP range",
            verb: "GET",
            target: "/badstatus",
            body: "",
            status: 502,
            expect_body: Some(""),
            content_type: None,
        },
        Row {
            case: "headers that are not an object of header values",
            verb: "GET",
            target: "/badheaders",
            body: "",
            status: 502,
            expect_body: Some(""),
            content_type: None,
        },
        Row {
            case: "a body that is not text",
            verb: "GET",
            target: "/badbody",
            body: "",
            status: 502,
            expect_body: Some(""),
            content_type: None,
        },
        Row {
            case: "a response past the size a Lambda target may answer with",
            verb: "GET",
            target: "/toolarge",
            body: "",
            status: 502,
            expect_body: Some(""),
            content_type: None,
        },
        Row {
            case: "a run that raised",
            verb: "GET",
            target: "/raise",
            body: "",
            status: 502,
            expect_body: Some(""),
            content_type: None,
        },
        Row {
            case: "the event the function was handed, echoed back",
            verb: "POST",
            target: "/echo?shelf=front&shelf=back",
            body: "a body the client sent",
            status: 200,
            expect_body: None,
            content_type: Some("application/json"),
        },
        Row {
            case: "a well-formed request after every refused one: the target still serves",
            verb: "GET",
            target: "/ok",
            body: "",
            status: 200,
            expect_body: Some(TARGET_BODY),
            content_type: Some("text/plain"),
        },
    ]
}

#[tokio::test]
async fn a_forward_to_a_lambda_target_group_is_served_by_the_function_it_registers() {
    let (router, engine) = emulator();

    // The side channel the handler reports every invocation to.
    let (status, queue) = sqs(&router, "CreateQueue", &json!({ "QueueName": SIDE_CHANNEL })).await;
    assert_eq!(
        status, 200,
        "setup CreateQueue: expected 200, got {status} {queue}"
    );
    let queue_url = queue["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateQueue: no QueueUrl in {queue}"))
        .to_string();

    // The function the load balancer will forward to, created through Lambda's own API.
    let (status, created) = lambda(
        &router,
        "POST",
        "/2015-03-31/functions",
        json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": 30,
            "Environment": { "Variables": { "QUEUE_URL": queue_url } },
            "Code": { "ZipFile": aws_smithy_types::base64::encode(package()) },
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup CreateFunction: expected 201, got {status} {created}"
    );
    let function_arn = created["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup CreateFunction: no FunctionArn in {created}"))
        .to_string();
    // The activation span Lambda owes the function, placed rather than slept for — the same
    // reading every suite that invokes a freshly created function uses.
    {
        let state = engine
            .state::<service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState>();
        let mut store = state.lock();
        let past = SystemTime::now() - Duration::from_secs(1);
        for record in store.functions.values_mut() {
            record.active_due = past;
        }
    }

    // The load balancer, the `lambda` target group, the registration of the function and the
    // HTTP:80 listener that forwards to the group: every one of them through this service's
    // own API.
    let (lb_arn, dns_name) = create_load_balancer(&router, "lambda-alb").await;

    let mut form = action("CreateTargetGroup");
    form.push(p("Name", "lambda-tg"));
    form.push(p("TargetType", "lambda"));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateTargetGroup: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let tg_arn = reply
        .text("TargetGroupArn")
        .unwrap_or_else(|| panic!("setup CreateTargetGroup: no TargetGroupArn in {}", reply.body));

    let mut form = action("RegisterTargets");
    form.push(p("TargetGroupArn", &tg_arn));
    form.push(p("Targets.member.1.Id", &function_arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup RegisterTargets: expected 200, got {} body {}",
        reply.status, reply.body
    );

    // The grant that admits the load balancer's invocation: the function's own
    // resource-based policy, narrowed by `aws:SourceArn` to the target group created below.
    // Written through Lambda's own `AddPermission`, as a client grants it.
    let statement = "elbv2-invoke";
    let (status, granted) = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        json!({
            "StatementId": statement,
            "Action": "lambda:InvokeFunction",
            "Principal": "elasticloadbalancing.amazonaws.com",
            "SourceArn": tg_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup AddPermission: expected 201, got {status} {granted}"
    );

    let mut form = action("CreateListener");
    form.push(p("LoadBalancerArn", &lb_arn));
    form.push(p("Protocol", "HTTP"));
    form.push(p("Port", "80"));
    form.push(p("DefaultActions.member.1.Type", "forward"));
    form.push(p("DefaultActions.member.1.TargetGroupArn", &tg_arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "setup CreateListener: expected 200, got {} body {}",
        reply.status, reply.body
    );

    // The move to `active` the clock owes the load balancer, placed rather than slept for.
    settle(&engine, &lb_arn, SystemTime::now() - Duration::from_secs(1));

    for row in rows() {
        let answered = send(&router, row.verb, &dns_name, row.target, row.body).await;
        assert_eq!(
            answered.status, row.status,
            "case {:?}: expected HTTP {}, got {} with body {:?}",
            row.case, row.status, answered.status, answered.body
        );
        if let Some(expected) = row.expect_body {
            assert_eq!(
                answered.body, expected,
                "case {:?}: expected body {expected:?}, got {:?}",
                row.case, answered.body
            );
        }
        let declared = answered.header("content-type").map(str::to_string);
        assert_eq!(
            declared.as_deref(),
            row.content_type,
            "case {:?}: expected content-type {:?}, got {declared:?}",
            row.case,
            row.content_type
        );
        // A refused response is the LOAD BALANCER's answer: neither the function's own body
        // nor the payload it returned reaches the client.
        if row.status == 502 {
            assert!(
                !answered.body.contains(TARGET_BODY) && !answered.body.contains('x'),
                "case {:?}: the refused payload must not be proxied, got body {:?}",
                row.case,
                answered.body
            );
        }
        // The function RAN, exactly once, whatever the load balancer answered.
        let mode = row
            .target
            .trim_start_matches('/')
            .split('?')
            .next()
            .unwrap_or_default();
        let reported = reported(&router, row.case, &queue_url, BUDGET).await;
        assert_eq!(
            reported,
            vec![mode.to_string()],
            "case {:?}: expected exactly one invocation reported as {mode:?}, got {reported:?}",
            row.case
        );
    }

    // The event the function was handed, as the echoing row reported it: every value read off
    // the request the client made and the group the request was forwarded to.
    let echoed = send(
        &router,
        "POST",
        &dns_name,
        "/echo?shelf=front&shelf=back",
        "a body the client sent",
    )
    .await;
    let event: Value = serde_json::from_str(&echoed.body)
        .unwrap_or_else(|error| panic!("case the echoed event: {error} in {:?}", echoed.body));
    for (member, expected) in [
        ("httpMethod", json!("POST")),
        ("path", json!("/echo")),
        // The single-valued member carries the LAST value a repeated name was given.
        ("query", json!({ "shelf": "back" })),
        ("body", json!("a body the client sent")),
        ("isBase64Encoded", json!(false)),
        ("proto", json!("http")),
        ("port", json!("80")),
        ("targetGroupArn", json!(tg_arn)),
    ] {
        assert_eq!(
            event.get(member).cloned().unwrap_or(Value::Null),
            expected,
            "case the echoed event: expected {member} {expected}, got {:?}",
            event.get(member)
        );
    }
    let _ = reported(&router, "the echoed event", &queue_url, BUDGET).await;

    // The grant is what admits the invocation: with the statement removed the load balancer
    // makes NO invocation at all and answers its own `502`, and with it added again the
    // function serves the request as before.
    let (status, removed) = lambda(
        &router,
        "DELETE",
        &format!("/2015-03-31/functions/{FUNCTION}/policy/{statement}"),
        Value::Null,
    )
    .await;
    assert_eq!(
        status, 204,
        "case the grant removed: expected RemovePermission to answer 204, got {status} {removed}"
    );
    let refused = send(&router, "GET", &dns_name, "/ok", "").await;
    assert_eq!(
        refused.status, 502,
        "case the grant removed: expected HTTP 502, got {} with body {:?}",
        refused.status, refused.body
    );
    let silent = reported(&router, "the grant removed", &queue_url, QUIET).await;
    assert!(
        silent.is_empty(),
        "case the grant removed: nothing may be invoked, got {silent:?}"
    );

    let (status, regranted) = lambda(
        &router,
        "POST",
        &format!("/2015-03-31/functions/{FUNCTION}/policy"),
        json!({
            "StatementId": statement,
            "Action": "lambda:InvokeFunction",
            "Principal": "elasticloadbalancing.amazonaws.com",
            "SourceArn": tg_arn,
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case the grant added again: expected AddPermission to answer 201, got {status} \
         {regranted}"
    );
    let served = send(&router, "GET", &dns_name, "/ok", "").await;
    assert_eq!(
        (served.status, served.body.as_str()),
        (200, TARGET_BODY),
        "case the grant added again: expected (200, {TARGET_BODY:?}), got ({}, {:?})",
        served.status,
        served.body
    );
    let again = reported(&router, "the grant added again", &queue_url, BUDGET).await;
    assert_eq!(
        again,
        vec!["ok".to_string()],
        "case the grant added again: expected exactly one invocation, got {again:?}"
    );

    // What the refusals cost the registration: nothing. The target group still lists the one
    // Lambda target, and it is still the target the load balancer forwards to.
    let mut form = action("DescribeTargetHealth");
    form.push(p("TargetGroupArn", &tg_arn));
    let reply = call(&router, &form).await;
    assert_eq!(
        reply.status, 200,
        "case the registration after the refusals: expected 200, got {} body {}",
        reply.status, reply.body
    );
    let described = members(&reply.body, "TargetHealthDescriptions");
    assert_eq!(
        described.len(),
        1,
        "case the registration after the refusals: expected the one Lambda target, got \
         {described:?}"
    );
    assert!(
        described[0].contains(&function_arn),
        "case the registration after the refusals: expected the function ARN {function_arn:?}, \
         got {:?}",
        described[0]
    );

    // And the store is untouched by the serving: the data plane answers out of the records
    // and writes none of its own.
    let state = engine.state::<ElbState>();
    let store = state.lock();
    let group = store
        .target_groups
        .get(&tg_arn)
        .unwrap_or_else(|| panic!("the target group {tg_arn:?} is still held"));
    assert_eq!(
        group.targets.len(),
        1,
        "case the store after the refusals: the registration is unchanged, got {:?}",
        group.targets
    );
}
