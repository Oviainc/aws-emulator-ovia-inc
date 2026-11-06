// SPDX-License-Identifier: CC0-1.0

//! An ACCEPTED asynchronous invocation is DELIVERED: the handler runs once the queued
//! event's attempt is due, and the side effects it performs are observable where it
//! performed them.
//!
//! The rule pinned here: "Event – Invoke the function asynchronously… The API response only
//! includes a status code" and "a success response (202 Accepted) indicates that the request
//! is queued for invocation" (`Invoke$InvocationType`), so the accepting call promises a
//! later invocation and says nothing about it — and that invocation then HAPPENS. That is
//! what every caller which invokes a function asynchronously depends on: an EventBridge
//! rule's Lambda target, an SNS subscription, a bucket notification, and a client calling
//! `Invoke` with `Event` or `InvokeAsync`.
//!
//! The rows differ along what the rule generalises over: WHICH call queued the event
//! (`Invoke` with `InvocationType=Event`, `InvokeAsync`), which version the event was
//! addressed to (the unpublished one, a published one), and what the handler does with the
//! event (write one message, write one per element of the event). Each row asserts both
//! halves: the queue entry settles into the outcome of a real run, and the work the handler
//! performed is there to be read.

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    AsyncInvocationOutcome, ComAmazonawsLambdaAwsgirapiserviceState, ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute every package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling.
const SLACK: Duration = Duration::from_millis(150);

/// How long this suite waits for a delivery the clock owes before giving up on it.
const DELIVERY_WAIT: Duration = Duration::from_secs(15);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The handler every row runs: it writes one message per element the event names to the
/// queue the event names, through the AWS SDK this runtime supplies.
const HANDLER_SOURCE: &str = r#"
import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    written = []
    for body in event["bodies"]:
        answered = client.send_message(QueueUrl=event["queue"], MessageBody=body)
        written.append(answered["MessageId"])
    return {"written": written, "requestId": context.aws_request_id}
"#;

/// Lambda and Amazon SQS over ONE shared engine, as `core/router/src/lib.rs` mounts them.
fn router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
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
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Amazon SQS call that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("authorization", authorization("sqs"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
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
    (parts.status.as_u16(), parsed)
}

/// A .zip deployment package holding [`HANDLER_SOURCE`] as `lambda_function.py`.
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

/// Create the function, publishing version 1 so a qualified reference resolves too.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Publish": true,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// Every queued event this store holds for `name`, in the order they were accepted.
fn queued(
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) -> Vec<(String, AsyncInvocationOutcome)> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(name)
        .map(|record| {
            record
                .async_invocations
                .iter()
                .map(|entry| (entry.request_id.clone(), entry.outcome.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// The outcome the one queued event of `name` settles into, waited for because the delivery
/// is owed by the CLOCK rather than by any call.
async fn settled(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
) -> (String, AsyncInvocationOutcome) {
    let looks = DELIVERY_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        if let Some(entry) = queued(engine, name).into_iter().find(|(_, outcome)| {
            !matches!(
                outcome,
                AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering
            )
        }) {
            return entry;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the queued event of {name} was never delivered; it is still {:?}",
        queued(engine, name)
    );
}

/// Which call queued the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Queuedby {
    /// `Invoke` with `InvocationType=Event`.
    InvokeEvent,
    /// `InvokeAsync`, the older surface that queues the same entry.
    InvokeAsync,
}

/// One row: how the event is queued, which version it is addressed to, and what the
/// handler writes.
struct Row {
    case: &'static str,
    queued_by: Queuedby,
    /// The qualifier the accepting call names, if any.
    qualifier: Option<&'static str>,
    /// The bodies the handler writes, one message each.
    bodies: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "Invoke with InvocationType=Event, on the unpublished version",
            queued_by: Queuedby::InvokeEvent,
            qualifier: None,
            bodies: &["one"],
        },
        Row {
            case: "Invoke with InvocationType=Event, on a published version",
            queued_by: Queuedby::InvokeEvent,
            qualifier: Some("1"),
            bodies: &["one"],
        },
        Row {
            case: "Invoke with InvocationType=Event, whose handler writes three messages",
            queued_by: Queuedby::InvokeEvent,
            qualifier: None,
            bodies: &["first", "second", "third"],
        },
        Row {
            case: "InvokeAsync, the older surface that queues the same entry",
            queued_by: Queuedby::InvokeAsync,
            qualifier: None,
            bodies: &["one"],
        },
    ]
}

/// Every row: the accepted event is delivered, the run succeeds, and the messages the
/// handler wrote are on the queue.
#[tokio::test]
async fn an_accepted_event_runs_the_handler_and_its_writes_are_observable() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let (router, engine) = router();
        let queue_name = format!("async-probe-{index}");
        let created = sqs(
            &router,
            case,
            "CreateQueue",
            &json!({ "QueueName": queue_name }),
        )
        .await;
        let queue_url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a QueueUrl, got {created}"))
            .to_string();

        let function = format!("async-caller-{index}");
        create_function(&router, case, &function).await;
        let event = json!({ "queue": queue_url, "bodies": row.bodies });

        let (status, answer) = match row.queued_by {
            Queuedby::InvokeEvent => {
                let uri = match row.qualifier {
                    Some(qualifier) => format!(
                        "{FUNCTIONS_URI}/{function}/invocations?Qualifier={qualifier}"
                    ),
                    None => format!("{FUNCTIONS_URI}/{function}/invocations"),
                };
                lambda(
                    &router,
                    "POST",
                    &uri,
                    event.to_string().as_bytes(),
                    &[("x-amz-invocation-type", "Event")],
                )
                .await
            }
            Queuedby::InvokeAsync => {
                lambda(
                    &router,
                    "POST",
                    &format!("/2014-11-13/functions/{function}/invoke-async/"),
                    event.to_string().as_bytes(),
                    &[],
                )
                .await
            }
        };
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 Accepted, got {status} ({answer})"
        );

        let (id, outcome) = settled(&engine, case, &function).await;
        assert_eq!(
            outcome,
            AsyncInvocationOutcome::Delivered {
                function_error: None
            },
            "case {case:?}: expected the delivered event to record a successful run, got \
             {outcome:?}"
        );
        assert!(
            !id.is_empty(),
            "case {case:?}: expected the delivered event to carry its own invocation id"
        );

        let drained = sqs(
            &router,
            case,
            "ReceiveMessage",
            &json!({ "QueueUrl": queue_url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        let mut bodies: Vec<String> = drained["Messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
            .collect();
        bodies.sort();
        let mut expected: Vec<String> = row.bodies.iter().map(ToString::to_string).collect();
        expected.sort();
        assert_eq!(
            bodies, expected,
            "case {case:?}: expected the handler's writes on the queue, got {bodies:?} \
             ({drained})"
        );
    }
}
