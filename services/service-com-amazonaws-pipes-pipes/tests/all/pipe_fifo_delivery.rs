// SPDX-License-Identifier: CC0-1.0

//! A FIFO-sourced pipe, end to end over the wire: what reaches a FIFO target and in what
//! order.
//!
//! Pipes and SQS are mounted on the SAME engine and every call goes through
//! `MultiServiceRouter::dispatch` — the one path a socket-served request takes — because
//! a delivery is asserted where a client sees it: on the target queue.
//!
//! The rules asserted, each over a TABLE whose rows differ along what the rule
//! generalises over (how many records, how many source message groups, the batch size the
//! records are polled under, and whether the target parameters carry literal values or
//! paths into the record):
//!
//! 1. A RUNNING pipe moves every record of its source to its target and delivers the
//!    records of one message group in the order they were sent — one poller, one delivery
//!    at a time, so a group's order survives the pipe.
//! 2. A target parameter's value is used as configured unless it is a JSON path into the
//!    event, which is resolved against the record being delivered — so one pipe can give
//!    each delivery its own message group or deduplication token instead of one constant
//!    for every record.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{call, Reply, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-fifo";

/// How long a wait for work the pipe has to do is given before it is a failure. Well
/// above the data plane's poll interval, so a row fails only when the pipe did not do the
/// work at all.
const BUDGET: Duration = Duration::from_secs(20);

/// How long between two looks at a queue while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(25);

/// The system attribute a received message reports its message group by. Source: the
/// `MessageSystemAttributeName` enum of the Amazon SQS model (`MessageGroupId`).
const MESSAGE_GROUP_ID: &str = "MessageGroupId";

/// The JSON path a row points a target parameter at to read the record's OWN message
/// group: the record envelope reports the source message's system attributes under
/// `attributes`.
const GROUP_OF_THE_RECORD: &str = "$.attributes.MessageGroupId";

/// The JSON path a row points the deduplication token at: the id of the source message
/// the event was derived from, which is distinct per record.
const ID_OF_THE_RECORD: &str = "$.messageId";

/// An input template that renders the record's BODY alone, so the target receives the
/// text the producer sent and nothing else.
const BODY_TEMPLATE: &str = "<$.body>";

/// An input template that renders a JSON object of its own, out of two values it reads
/// from the record.
const OBJECT_TEMPLATE: &str = r#"{"order": "<$.body>", "from": "<$.attributes.MessageGroupId>"}"#;

/// Pipes and SQS on one engine: a pipe's source and target are both queues here.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One `awsJson` RPC call against Amazon SQS.
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
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/sqs/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&payload).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// A FIFO queue called `<name>.fifo`; answers its url and the ARN a pipe names it by.
///
/// `content_based` enables `ContentBasedDeduplication`, which is what lets a send carry
/// no deduplication token of its own.
async fn given_fifo_queue(
    router: &MultiServiceRouter,
    name: &str,
    content_based: bool,
) -> (String, String) {
    let mut attributes = serde_json::Map::new();
    attributes.insert("FifoQueue".to_string(), json!("true"));
    if content_based {
        attributes.insert("ContentBasedDeduplication".to_string(), json!("true"));
    }
    let (status, body) = sqs(
        router,
        "CreateQueue",
        &json!({
            "QueueName": format!("{name}.fifo"),
            "Attributes": Value::Object(attributes),
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    (url, arn)
}

/// Send `body` to the FIFO queue at `url` under the message group `group`.
async fn send_in_group(router: &MultiServiceRouter, url: &str, group: &str, body: &str) {
    let (status, answer) = sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body, "MessageGroupId": group }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case send {body}: SendMessage answered {status} {answer}"
    );
}

/// How a row's deliveries are READ: what the target message's body is, which is what
/// the pipe's `InputTemplate` (or its absence) decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reading {
    /// No template: the body is the Amazon SQS record envelope, so the marker is its
    /// `body` and the source message group is its `attributes`.
    Envelope,
    /// A template that renders the record's body alone: the body IS the marker, and the
    /// group the delivery was sent under is what says which source group it came from.
    Marker,
    /// A template that renders a JSON object of its own: the marker and the source group
    /// are the members the template spells out.
    Rendered,
}

/// The members a [`Reading::Rendered`] template writes the marker and the source group
/// to.
const RENDERED_MARKER: &str = "order";
const RENDERED_GROUP: &str = "from";

/// One message as this suite reads a delivery: the marker the source record carried, the
/// source message group it came from, and the message group the TARGET message itself was
/// sent under.
#[derive(Debug, Clone)]
struct Delivered {
    marker: String,
    source_group: String,
    target_group: String,
}

/// Receive up to ten messages from the queue at `url` and DELETE each one, so a FIFO
/// queue hands over what is behind them on the next call (a message of a group that is
/// still in flight stops that group).
async fn drain(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    reading: Reading,
) -> Vec<Delivered> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageSystemAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {body}"
    );
    let mut taken: Vec<Delivered> = Vec::new();
    for message in body["Messages"].as_array().cloned().unwrap_or_default() {
        let text = message["Body"].as_str().unwrap_or_default().to_string();
        let target_group = message["Attributes"][MESSAGE_GROUP_ID]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let parsed = |what: &str| -> Value {
            serde_json::from_str(&text).unwrap_or_else(|error| {
                panic!("case {case:?}: a delivered body is not {what} ({error}): {text:?}")
            })
        };
        let (marker, source_group) = match reading {
            Reading::Envelope => {
                let record = parsed("the Amazon SQS record envelope");
                (
                    record["body"].as_str().unwrap_or_default().to_string(),
                    record["attributes"][MESSAGE_GROUP_ID]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            }
            Reading::Marker => (text.clone(), target_group.clone()),
            Reading::Rendered => {
                let rendered = parsed("the object its input template renders");
                (
                    rendered[RENDERED_MARKER]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    rendered[RENDERED_GROUP]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            }
        };
        taken.push(Delivered {
            marker,
            source_group,
            target_group,
        });
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            let (status, answer) = sqs(
                router,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteMessage answered {status} {answer}"
            );
        }
    }
    taken
}

/// Everything that arrives at `url`, in arrival order, until there are at least `want`
/// messages or `budget` has elapsed.
async fn collect_at_least(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    reading: Reading,
    want: usize,
    budget: Duration,
) -> Vec<Delivered> {
    let deadline = Instant::now() + budget;
    let mut collected: Vec<Delivered> = Vec::new();
    loop {
        collected.extend(drain(router, url, case, reading).await);
        if collected.len() >= want || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `CreatePipe` for one row.
async fn create_pipe(router: &MultiServiceRouter, name: &str, body: Value) -> Reply {
    call(router, "POST", &format!("/v1/pipes/{name}"), Some(body), REGION).await
}

/// One row of the FIFO delivery table.
struct Fifo {
    /// The case, which is also the pipe's and the queues' name.
    case: &'static str,
    /// The source message group each record is sent under, in send order; the markers are
    /// derived from the case and the index.
    groups: &'static [&'static str],
    /// `BatchSize`, when the row configures one.
    batch_size: Option<i64>,
    /// The target's `MessageGroupId` as the pipe configures it: a literal group, or a
    /// path into the record.
    target_group: &'static str,
    /// The target's `MessageDeduplicationId`, when the row configures one.
    target_deduplication: Option<&'static str>,
    /// Whether the TARGET queue enables `ContentBasedDeduplication` — a FIFO queue that
    /// does not requires every send to carry its own token.
    target_content_based: bool,
    /// `TargetParameters.InputTemplate`, when the row configures one: what the target
    /// receives instead of the record envelope.
    target_template: Option<&'static str>,
    /// How this row's deliveries are read, which follows from its template.
    reading: Reading,
}

/// RULE: once a pipe reports RUNNING it polls its source with no client action and
/// delivers every record to its target under the pipe's target parameters, keeping the
/// order the records of one message group were sent in; what the target RECEIVES is the
/// record envelope, or what the pipe's `InputTemplate` renders from the record, and a
/// parameter or a placeholder whose value is a JSON path carries, for each delivery, the
/// value that path selects in the record being delivered.
///
/// The rows differ along what the rule generalises over: how many records are sent, how
/// many source message groups they are spread over, the batch size they are polled under,
/// whether the target parameters are literal or paths into the record, whether the target
/// queue needs a deduplication token per send, and whether an input template reshapes the
/// payload (and into text or into JSON).
#[tokio::test]
async fn a_fifo_sourced_pipe_delivers_each_group_in_send_order() {
    let rows = [
        Fifo {
            case: "one-record-per-batch",
            groups: &["g1", "g1", "g1"],
            batch_size: Some(1),
            target_group: "pipe-group",
            target_deduplication: None,
            target_content_based: true,
            target_template: None,
            reading: Reading::Envelope,
        },
        Fifo {
            case: "default-batch-size",
            groups: &["g1", "g1", "g1", "g1", "g1"],
            batch_size: None,
            target_group: "pipe-group",
            target_deduplication: None,
            target_content_based: true,
            target_template: None,
            reading: Reading::Envelope,
        },
        Fifo {
            case: "two-source-groups",
            groups: &["ga", "gb", "ga", "gb", "ga", "gb"],
            batch_size: Some(10),
            target_group: "pipe-group",
            target_deduplication: None,
            target_content_based: true,
            target_template: None,
            reading: Reading::Envelope,
        },
        Fifo {
            case: "group-read-from-the-record",
            groups: &["ga", "gb", "ga", "gb"],
            batch_size: Some(2),
            target_group: GROUP_OF_THE_RECORD,
            target_deduplication: None,
            target_content_based: true,
            target_template: None,
            reading: Reading::Envelope,
        },
        Fifo {
            case: "token-read-from-the-record",
            groups: &["g1", "g1", "g1"],
            batch_size: Some(1),
            target_group: "pipe-group",
            target_deduplication: Some(ID_OF_THE_RECORD),
            // No content-based deduplication, so each send has to carry its own token —
            // which the row reads from the record.
            target_content_based: false,
            target_template: None,
            reading: Reading::Envelope,
        },
        Fifo {
            case: "template-renders-the-body",
            groups: &["ga", "gb", "ga", "gb"],
            batch_size: Some(1),
            // The group is read from the record, so each delivery still says which source
            // group it came from even though its body is the record's body alone.
            target_group: GROUP_OF_THE_RECORD,
            target_deduplication: None,
            target_content_based: true,
            target_template: Some(BODY_TEMPLATE),
            reading: Reading::Marker,
        },
        Fifo {
            case: "template-renders-an-object",
            groups: &["ga", "gb", "ga"],
            batch_size: Some(3),
            target_group: "pipe-group",
            target_deduplication: None,
            target_content_based: true,
            target_template: Some(OBJECT_TEMPLATE),
            reading: Reading::Rendered,
        },
    ];

    for row in &rows {
        run_row(row).await;
    }
}

/// The markers one row sends: `<case>-0`, `<case>-1`, … — distinct per row and per
/// record, so a delivery can be attributed.
fn markers(case: &str, count: usize) -> Vec<String> {
    (0..count).map(|index| format!("{case}-{index}")).collect()
}

/// The markers of `delivered` that belong to the source group `group`, in arrival order.
fn arrivals_of(delivered: &[Delivered], group: &str) -> Vec<String> {
    delivered
        .iter()
        .filter(|message| message.source_group == group)
        .map(|message| message.marker.clone())
        .collect()
}

/// The markers one row sent into the source group `group`, in send order.
fn sends_of(groups: &[&str], markers: &[String], group: &str) -> Vec<String> {
    groups
        .iter()
        .zip(markers)
        .filter(|(sent_group, _)| **sent_group == group)
        .map(|(_, marker)| marker.clone())
        .collect()
}

/// The distinct source groups of one row, in the order they first appear.
fn groups_of(groups: &[&str]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for group in groups {
        if !seen.iter().any(|held| held == group) {
            seen.push((*group).to_string());
        }
    }
    seen
}

/// The target group each delivery must carry: the literal the pipe configures, or the
/// record's own group when the parameter is the path to it.
fn expected_target_group(configured: &str, source_group: &str) -> String {
    if configured == GROUP_OF_THE_RECORD {
        source_group.to_string()
    } else {
        configured.to_string()
    }
}

/// Keep the helpers above honest: every row of the table is driven by this one body.
async fn run_row(row: &Fifo) {
    let case = row.case;
    let router = emulator();
    let (source_url, source_arn) = given_fifo_queue(&router, &format!("{case}-src"), true).await;
    let (target_url, target_arn) =
        given_fifo_queue(&router, &format!("{case}-tgt"), row.target_content_based).await;

    let mut source_parameters = serde_json::Map::new();
    if let Some(size) = row.batch_size {
        source_parameters.insert("BatchSize".to_string(), json!(size));
    }
    let mut target_parameters = serde_json::Map::new();
    target_parameters.insert(MESSAGE_GROUP_ID.to_string(), json!(row.target_group));
    if let Some(token) = row.target_deduplication {
        target_parameters.insert("MessageDeduplicationId".to_string(), json!(token));
    }
    let mut parameters = serde_json::Map::new();
    parameters.insert(
        "SqsQueueParameters".to_string(),
        Value::Object(target_parameters),
    );
    if let Some(template) = row.target_template {
        parameters.insert("InputTemplate".to_string(), json!(template));
    }
    let created = create_pipe(
        &router,
        case,
        json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": { "SqsQueueParameters": Value::Object(source_parameters) },
            "TargetParameters": Value::Object(parameters),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    let sent = markers(case, row.groups.len());
    for (group, marker) in row.groups.iter().zip(&sent) {
        send_in_group(&router, &source_url, group, marker).await;
    }

    let delivered =
        collect_at_least(&router, &target_url, case, row.reading, sent.len(), BUDGET).await;
    let seen: BTreeSet<String> = delivered
        .iter()
        .map(|message| message.marker.clone())
        .collect();
    let expected: BTreeSet<String> = sent.iter().cloned().collect();
    assert_eq!(
        seen, expected,
        "case {case:?}: expected every sent record delivered exactly once, got {delivered:?}"
    );
    assert_eq!(
        delivered.len(),
        sent.len(),
        "case {case:?}: expected {} deliveries, got {delivered:?}",
        sent.len()
    );

    // Per-group order: the records of one source message group arrive in the order they
    // were sent, whatever else shares the queue.
    for group in groups_of(row.groups) {
        let arrived = arrivals_of(&delivered, &group);
        let in_send_order = sends_of(row.groups, &sent, &group);
        assert_eq!(
            arrived, in_send_order,
            "case {case:?}: expected group {group:?} delivered in send order \
             {in_send_order:?}, got {arrived:?}"
        );
    }

    // The target parameters each delivery carried.
    let mut per_group: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for message in &delivered {
        let expected = expected_target_group(row.target_group, &message.source_group);
        assert_eq!(
            message.target_group, expected,
            "case {case:?}: expected the delivery of {:?} sent under message group \
             {expected:?}, got {message:?}",
            message.marker
        );
        per_group
            .entry(message.target_group.clone())
            .or_default()
            .push(message.marker.clone());
    }
    assert!(
        !per_group.is_empty(),
        "case {case:?}: every delivery must carry a message group, got {delivered:?}"
    );

    // Nothing is left on the source: a record is deleted once what it produced was
    // delivered.
    let left = wait_for_visible(&router, &source_url, case, 0, BUDGET).await;
    assert_eq!(
        left, 0,
        "case {case:?}: expected the delivered records to be deleted from the source, \
         {left} are still visible"
    );
}

/// Wait until the queue at `url` reports `want` visible messages, answering what it
/// reported last.
async fn wait_for_visible(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    want: i64,
    budget: Duration,
) -> i64 {
    let deadline = Instant::now() + budget;
    loop {
        let (status, body) = sqs(
            router,
            "GetQueueAttributes",
            &json!({ "QueueUrl": url, "AttributeNames": ["ApproximateNumberOfMessages"] }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetQueueAttributes answered {status} {body}"
        );
        let seen: i64 = body["Attributes"]["ApproximateNumberOfMessages"]
            .as_str()
            .unwrap_or_default()
            .parse()
            .unwrap_or(-1);
        if seen == want || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}
