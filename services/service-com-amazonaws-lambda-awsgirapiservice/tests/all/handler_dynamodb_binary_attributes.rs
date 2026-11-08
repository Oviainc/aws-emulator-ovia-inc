// SPDX-License-Identifier: CC0-1.0

//! The same rule as [`crate::handler_secret_sdk`]'s first one, read at DEPTH: Lambda and
//! Amazon DynamoDB on ONE engine, and a `python3.x` function that writes and reads item
//! attributes carrying BYTES through `boto3.client("dynamodb")`.
//!
//! The rule pinned here: a value the called operation's model types as a blob is handed to
//! the handler as the `bytes` the answer carries, decoded from the base64 a JSON protocol
//! travels it as — wherever in the answer that value sits. A DynamoDB attribute value is the
//! case that makes "wherever" the point: the bytes of a `B` attribute sit under a map of
//! attribute values, inside a list (`L`), inside another map (`M`) or inside a set (`BS`), so
//! the runtime has to read the model's whole output tree rather than its top-level members.
//!
//! The rows differ along where the value sits — the attribute itself, inside a list, inside a
//! nested map, inside a binary set — and along the bytes it carries, including bytes no text
//! encoding carries.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module and attribute the package below carries its handler as.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. The transition is owed by the
/// CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How long this suite waits for the table it created to become `ACTIVE`, and how often it
/// looks again: the transition is one the clock owes, so the suite waits for it rather than
/// writing before it is due.
const BUDGET: Duration = Duration::from_secs(30);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// The table the handler writes to, and its one key attribute.
const TABLE: &str = "handler-binary-attributes";
const KEY_ATTRIBUTE: &str = "pk";

/// Lambda and Amazon DynamoDB over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them: the handler's SDK call is dispatched back into this same router, so the item it reads
/// is the item it wrote.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
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

/// One Amazon DynamoDB call (`awsJson1_0`, `DynamoDB_20120810.<Operation>`) that must answer
/// 200.
async fn dynamodb(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("authorization", authorization("dynamodb"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
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
        "case {case:?}: expected HTTP 200 from DynamoDB {operation}, got {status} with {parsed}"
    );
    parsed
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

/// A .zip deployment package holding `source` as `lambda_function.py`, written in memory the
/// way a caller uploads one.
fn package(source: &str) -> Vec<u8> {
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

/// Create a function whose code is `source` and wait out the provisioning window.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str, source: &str) {
    let (status, _headers, created) = lambda(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function synchronously with `event`, answering the payload the run produced.
async fn invoke(router: &MultiServiceRouter, case: &str, name: &str, event: &Value) -> Value {
    let (status, headers, payload) = lambda(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        event.to_string().as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Invoke, got {status} ({payload})"
    );
    let signal = headers
        .get("x-amz-function-error")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    assert_eq!(
        signal, None,
        "case {case:?}: the handler must run to completion, got the function error {signal:?} \
         with {payload}"
    );
    payload
}

/// The table the handler writes, waited out to `ACTIVE` — "you can perform read and write
/// operations only on an ACTIVE table".
async fn given_table(router: &MultiServiceRouter, case: &str) {
    dynamodb(
        router,
        case,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "AttributeDefinitions": [{"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}],
            "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let deadline = Instant::now() + BUDGET;
    loop {
        let described =
            dynamodb(router, case, "DescribeTable", &json!({"TableName": TABLE})).await;
        if described["Table"]["TableStatus"] == json!("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the table never became ACTIVE: {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The handler every row runs: it writes the attribute the event names, reads the item back
/// and reports, for every value it found, the PYTHON TYPE it arrived as and its base64 — which
/// only a bytes-like value encodes.
const HANDLER_SOURCE: &str = r#"
import base64
import os

import boto3

CLIENT = boto3.client("dynamodb")
TABLE = os.environ["TABLE"]


def report(value):
    """What one attribute value IS: the type of every member it carries, and its bytes."""
    kind, carried = next(iter(value.items()))
    if kind == "B":
        return {
            "kind": kind,
            "type": type(carried).__name__,
            "base64": base64.b64encode(carried).decode("ascii"),
        }
    if kind == "BS":
        return {
            "kind": kind,
            "type": sorted({type(member).__name__ for member in carried}),
            "base64": sorted(base64.b64encode(member).decode("ascii") for member in carried),
        }
    if kind == "L":
        return {"kind": kind, "members": [report(member) for member in carried]}
    if kind == "M":
        return {
            "kind": kind,
            "members": {name: report(held) for name, held in carried.items()},
        }
    return {"kind": kind, "type": type(carried).__name__, "value": carried}


def lambda_handler(event, context):
    """Write and read back one item per row the event names, in one invocation."""
    written = []
    for row in event["rows"]:
        key = {event["key_attribute"]: {"S": row["key"]}}
        item = dict(key)
        item["attr"] = _written(row["written"])
        CLIENT.put_item(TableName=TABLE, Item=item)
        read = CLIENT.get_item(TableName=TABLE, Key=key, ConsistentRead=True)["Item"]
        written.append({
            "attr": report(read["attr"]),
            "key": report(read[event["key_attribute"]]),
        })
    return {"rows": written}


def _written(written):
    """The attribute value the event describes, with every blob as the BYTES it names.

    The event carries bytes as base64 text, because an invocation's own payload is JSON;
    what the handler passes to the SDK is `bytes`, which is what a handler written against
    the SDK passes for a blob member.
    """
    kind, carried = next(iter(written.items()))
    if kind == "B":
        return {kind: base64.b64decode(carried)}
    if kind == "BS":
        return {kind: [base64.b64decode(member) for member in carried]}
    if kind == "L":
        return {kind: [_written(member) for member in carried]}
    if kind == "M":
        return {kind: {name: _written(held) for name, held in carried.items()}}
    return {kind: carried}
"#;

/// One row: where the bytes sit inside the attribute value, and the bytes themselves.
struct Row {
    case: &'static str,
    /// The bytes the row writes, as the one blob inside its attribute value.
    bytes: Vec<u8>,
    /// The attribute value the row writes, built around that blob's base64 — and the shape the
    /// report of it must have.
    shape: Shape,
}

/// Where the blob sits inside the attribute value the row writes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// The attribute IS the blob (`B`).
    Blob,
    /// A list of attribute values (`L`) holding it.
    InsideAList,
    /// A map of attribute values (`M`) holding a list holding it.
    InsideANestedMap,
    /// A binary set (`BS`), a list of blobs rather than of attribute values.
    InsideABinarySet,
}

/// The rows: every place a DynamoDB attribute value carries bytes, over byte contents that
/// span what the rule generalises over — a single byte, bytes no text encoding carries, and
/// every byte value.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the attribute is the blob",
            bytes: vec![0x00, 0x01, 0xfe],
            shape: Shape::Blob,
        },
        Row {
            case: "the attribute is the blob, carrying every byte value",
            bytes: (0u8..=255).collect(),
            shape: Shape::Blob,
        },
        Row {
            case: "the blob sits inside a list of attribute values",
            bytes: vec![0xff],
            shape: Shape::InsideAList,
        },
        Row {
            case: "the blob sits inside a list inside a map of attribute values",
            bytes: vec![0x80, 0x00, 0x7f],
            shape: Shape::InsideANestedMap,
        },
        Row {
            case: "the blob is a member of a binary set",
            bytes: vec![0xfe, 0xdc],
            shape: Shape::InsideABinarySet,
        },
    ]
}

impl Row {
    /// The base64 the event carries this row's bytes as.
    fn base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.bytes)
    }

    /// The attribute value the handler is to write, as the event describes it.
    fn written(&self) -> Value {
        let blob = json!({"B": self.base64()});
        match self.shape {
            Shape::Blob => blob,
            Shape::InsideAList => json!({"L": [{"S": "before"}, blob]}),
            Shape::InsideANestedMap => json!({"M": {"inner": {"L": [blob]}}}),
            Shape::InsideABinarySet => json!({"BS": [self.base64()]}),
        }
    }

    /// What the handler must report for the attribute it read back: the bytes it wrote, as
    /// `bytes`, wherever they sat.
    fn expected(&self) -> Value {
        let blob = json!({"kind": "B", "type": "bytes", "base64": self.base64()});
        match self.shape {
            Shape::Blob => blob,
            Shape::InsideAList => json!({
                "kind": "L",
                "members": [{"kind": "S", "type": "str", "value": "before"}, blob],
            }),
            Shape::InsideANestedMap => json!({
                "kind": "M",
                "members": {"inner": {"kind": "L", "members": [blob]}},
            }),
            Shape::InsideABinarySet => json!({
                "kind": "BS",
                "type": ["bytes"],
                "base64": [self.base64()],
            }),
        }
    }
}

/// RULE: every value the called operation's model types as a blob is handed to the handler as
/// the bytes the answer carries, wherever in the answer it sits — a `B` attribute, one inside
/// a list or a nested map, or a member of a binary set — while a text member stays `str`.
#[tokio::test]
async fn a_blob_at_any_depth_of_an_answer_is_handed_to_the_handler_as_bytes() {
    let _stream = one_invocation_stream_at_a_time().await;
    let case = "a handler reading back the item it wrote";
    let function_name = "dynamodb-binary-attributes";
    let router = router();
    given_table(&router, case).await;
    create_function(&router, case, function_name, HANDLER_SOURCE).await;
    // The table the handler writes to, which it reads out of its own environment.
    let (status, _headers, updated) = lambda(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{function_name}/configuration"),
        &serde_json::to_vec(&json!({"Environment": {"Variables": {"TABLE": TABLE}}}))
            .expect("the update body serializes"),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateFunctionConfiguration, got {status} \
         ({updated})"
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;

    let rows = rows();
    let event_rows: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| json!({"key": format!("item-{index}"), "written": row.written()}))
        .collect();
    let payload = invoke(
        &router,
        case,
        function_name,
        &json!({"key_attribute": KEY_ATTRIBUTE, "rows": event_rows}),
    )
    .await;
    let answered = payload["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        answered.len(),
        rows.len(),
        "case {case:?}: expected one answer per row, got {payload}"
    );
    for (index, (row, answer)) in rows.iter().zip(&answered).enumerate() {
        let case = row.case;
        assert_eq!(
            answer["attr"],
            row.expected(),
            "case {case:?}: expected the attribute's bytes as bytes, got {answer}"
        );
        // The key attribute is text, so it arrives as text: the decoding follows the model's
        // own type for each member rather than the value's form.
        assert_eq!(
            answer["key"],
            json!({"kind": "S", "type": "str", "value": format!("item-{index}")}),
            "case {case:?}: expected a text attribute as str, got {answer}"
        );
    }
}
