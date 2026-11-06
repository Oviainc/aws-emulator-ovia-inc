// SPDX-License-Identifier: CC0-1.0

//! What an event source mapping on an Amazon SQS queue HANDS the function it maps, asserted
//! through the function's own run: the handler of a real deployment package inspects every
//! message of the batch it was given and decides, from that message's own body, what to
//! report back — so what this suite reads back off the queue is a consequence of the messages
//! having reached the handler.
//!
//! RULE: once a mapping on a queue is `Enabled`, Lambda reads that queue's messages and
//! invokes the mapped function with one record per message ("Lambda reads items from the
//! event source and invokes the function"), each record naming the queue it came from and
//! carrying that message's own id, receipt handle and body — and a message LEAVES the queue
//! only once the invocation it was handed to has PROCESSED it. So the fate of each message is
//! the handler's own answer: a batch its handler returned from is consumed and the mapping
//! reports `OK`, a batch whose handler raised is left on the queue and the mapping reports
//! the function-call problem, and for a mapping whose `FunctionResponseTypes` asks for the
//! report ("A list of current response type enums applied to the event source mapping"), the
//! messages the handler names stay and the rest are consumed.
//!
//! This is the half of the queue data plane the other suites over it cannot see: they upload
//! an opaque package, so no batch they poll is ever processed. Here the package RUNS, and
//! every row's verdict is computed inside the handler from the record it was handed — a
//! handler that was never invoked, or invoked with a record of the wrong shape, reports
//! nothing or raises, and the row fails.
//!
//! The rows differ along what the rule generalises over: how many messages the batch holds,
//! what their bodies are, whether the mapping asks for its function's report of which
//! messages failed, which message of the batch the handler will not process, and how the
//! handler ends (a return, a raise).

use std::io::Write as _;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED, LAST_PROCESSING_RESULT_OK,
    REPORT_BATCH_ITEM_FAILURES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{
    call, call_sqs, create_mapping, create_queue, get_mapping, messages_held,
    queue_message_counts, router_with_queues,
};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern. No IAM is
/// mounted here, so nothing is known about it and nothing is refused for it.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The runtime every function below is created with, and the handler its package exposes.
const RUNTIME: &str = "python3.12";
/// See [`RUNTIME`].
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long a row waits for work the clock owes — a function going `Active`, a pass polling
/// and invoking — before it is a failure.
const BUDGET: Duration = Duration::from_secs(20);

/// How long between two looks while waiting.
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Several of the visibility windows these queues hand a batch over for: long enough that a
/// message no attempt consumed has come back and been handed over again, so what the queue
/// then holds is the steady state rather than one pass's.
const VISIBILITY_WINDOWS: Duration = Duration::from_secs(4);

/// The `Timeout` every function below carries, and the `VisibilityTimeout` of the queue it
/// polls: a mapping is accepted only while the queue hides a delivered message for at least
/// as long as one attempt at it may last, so the two are the same span here.
const ATTEMPT_SECONDS: i64 = 3;

/// The prefix a message body carries to be one the handler reports as unprocessed, and the
/// prefix that makes it raise instead. Nothing in the emulator reads either: they are read by
/// the handler, out of the record's own body.
const POISON: &str = "poison";
/// See [`POISON`].
const RAISE: &str = "raise";

/// The handler every row runs. It asserts the shape of the envelope Lambda hands a queue
/// batch over as — one record per message, naming its source and queue and carrying the
/// message's own id, receipt handle and body — and decides what to report from that body, so
/// the report is a consequence of the message having arrived.
const HANDLER_SOURCE: &str = r#"
import os

QUEUE_ARN = os.environ["QUEUE_ARN"]


def lambda_handler(event, context):
    records = event["Records"]
    if not records:
        raise ValueError("a batch was handed over holding no message")
    failures = []
    for record in records:
        if record["eventSource"] != "aws:sqs":
            raise ValueError("eventSource is " + repr(record["eventSource"]))
        if record["eventSourceARN"] != QUEUE_ARN:
            raise ValueError("eventSourceARN is " + repr(record["eventSourceARN"]))
        if not record["messageId"]:
            raise ValueError("a record carries no messageId")
        if not record["receiptHandle"]:
            raise ValueError("a record carries no receiptHandle")
        body = record["body"]
        if body.startswith("raise"):
            raise RuntimeError("the handler cannot process " + body)
        if body.startswith("poison"):
            failures.append({"itemIdentifier": record["messageId"]})
    return {"batchItemFailures": failures}
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

/// A function whose package RUNS the handler, told through its environment which queue the
/// messages it is handed must come from, waited out to `Active`.
async fn given_handler_function(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    queue_arn: &str,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": RUNTIME,
            "Handler": HANDLER,
            "Timeout": ATTEMPT_SECONDS,
            "Environment": { "Variables": { "QUEUE_ARN": queue_arn } },
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package()),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "case {case:?}: expected CreateFunction to answer 201, got {} {}",
        created.status,
        created.text()
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let read = call(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        if read.string("State").as_deref() == Some("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the function {name:?} never became Active: {}",
            read.text()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// `SendMessage` of `body` to the queue `url` addresses.
async fn send_message(router: &MultiServiceRouter, case: &str, url: &str, body: &str) {
    let (status, sent) = call_sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected SendMessage to answer 200, got {status} {sent}"
    );
}

/// Wait until the mapping `uuid` names reports `expected` as its `LastProcessingResult`,
/// answering whether it did.
async fn reports_result(
    router: &MultiServiceRouter,
    case: &str,
    uuid: &str,
    expected: &str,
) -> bool {
    let deadline = Instant::now() + BUDGET;
    loop {
        let reading = get_mapping(router, uuid).await;
        assert_eq!(
            reading.status, 200,
            "case {case:?}: expected GetEventSourceMapping to answer 200, got {} {}",
            reading.status,
            reading.text()
        );
        if reading.string("LastProcessingResult").as_deref() == Some(expected) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// One row: the bodies waiting on the queue, whether the mapping reads its function's report
/// of which messages failed, how many messages must be left on the queue once the handler has
/// run, and what the mapping must then report.
struct Row {
    case: &'static str,
    bodies: &'static [&'static str],
    reads_report: bool,
    left_on_queue: u32,
    expected_result: &'static str,
}

#[tokio::test]
async fn a_queue_mapping_consumes_exactly_what_its_handler_processed() {
    let rows: [Row; 4] = [
        Row {
            case: "one message the handler processes",
            bodies: &["order-1"],
            reads_report: false,
            left_on_queue: 0,
            expected_result: LAST_PROCESSING_RESULT_OK,
        },
        Row {
            case: "three messages the handler processes",
            bodies: &["order-1", "order-2", "order-3"],
            reads_report: false,
            left_on_queue: 0,
            expected_result: LAST_PROCESSING_RESULT_OK,
        },
        Row {
            // The handler raises, so no message of the batch was processed and the queue
            // keeps all of them.
            case: "a batch whose handler raises",
            bodies: &["order-1", RAISE],
            reads_report: false,
            left_on_queue: 2,
            expected_result: LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
        },
        Row {
            // The report names the one message the handler would not process: it stays and
            // the rest are consumed. Once it is all that is left, every pass hands it over
            // again and is reported as the message it did not process, so the result the
            // mapping SETTLES on is the unsuccessful attempt at that one message.
            case: "a reported partial failure in the middle of the batch",
            bodies: &["order-1", POISON, "order-3"],
            reads_report: true,
            left_on_queue: 1,
            expected_result: LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = router_with_queues();
        // A queue that hides a delivered message for the length of one attempt at it, so a
        // message no attempt consumed is back for a later pass inside the row's own window.
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("esm-handler-run-queue-{index}"),
            &json!({ "VisibilityTimeout": ATTEMPT_SECONDS.to_string() }),
        )
        .await;
        let function = format!("esm-handler-run-fn-{index}");
        given_handler_function(&router, case, &function, &arn).await;
        let mut request = json!({
            "FunctionName": function,
            "EventSourceArn": arn,
            "BatchSize": row.bodies.len(),
        });
        if row.reads_report {
            request["FunctionResponseTypes"] = json!([REPORT_BATCH_ITEM_FAILURES]);
        }
        let created = create_mapping(&router, &request).await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
        for body in row.bodies {
            send_message(&router, case, &url, body).await;
        }

        // What the attempt came to is the handler's own answer.
        assert!(
            reports_result(&router, case, &uuid, row.expected_result).await,
            "case {case:?}: the mapping must come to report {:?} for the batch its handler \
             ran on, got {}",
            row.expected_result,
            get_mapping(&router, &uuid).await.text()
        );

        // And the queue holds exactly the messages that attempt did not process — read
        // after several of its own visibility windows, so a message it left is one that came
        // back and was not consumed by a later pass either.
        tokio::time::sleep(VISIBILITY_WINDOWS).await;
        let counts = queue_message_counts(&router, case, &url).await;
        let held = messages_held(counts);
        assert_eq!(
            held, row.left_on_queue,
            "case {case:?}: the queue must hold the {} message(s) the attempt did not \
             process, got {held} ({counts:?})",
            row.left_on_queue
        );
    }
}
