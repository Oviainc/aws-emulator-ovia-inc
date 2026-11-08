// SPDX-License-Identifier: CC0-1.0

//! The Amazon S3 call a RUNNING handler makes, bound end to end over the wire: Lambda and
//! Amazon S3 on ONE engine, a `python3.x` function whose code does `import boto3` and reads
//! and writes the bucket, and what it did read back where a client reads it.
//!
//! RULE: a managed runtime's SDK reaches every service this emulator serves, over that
//! service's OWN protocol, so what a handler's client method answers is the operation's own
//! output members and what it sends is its own input members — the object's bytes as the
//! payload they are (handed back as the stream a client answers one with), the values a
//! LISTING carries as the types the model gives them, and a refusal as the service's own
//! error code and message raised as `botocore.exceptions.ClientError`. Which operations that
//! SDK binds is [`service_com_amazonaws_lambda_awsgirapiservice::state::execution`]'s to say;
//! what this pins is that each one it binds is bound RIGHT.
//!
//! The rows differ along what the rule generalises over: WHICH binding class the operation
//! uses — a write carrying bytes whose answer is the object's own metadata, a read whose
//! payload is the bytes and whose other members are metadata, a read of the metadata alone, a
//! copy whose answer nests a structure, a listing bound by query parameters whose answer
//! holds a list of structures and typed scalars, a listing that matches nothing, and a delete
//! — and whether the call is served or refused. They run in ONE invocation, because each step
//! reads what the one before it wrote and because an invocation is the dearest thing this
//! target does; the last assertions read the whole thing back through S3's OWN API, so a call
//! that answered without doing the work fails here.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]): this emulator serves at
//! most `max_concurrent_runs` invocations at once for the whole process.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mappings::{await_function_active, call, router_on};
// The one .zip writer this crate's suites package a handler with, exported by the suite that
// first needed it rather than copied here.
use crate::node_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The bucket this suite runs over.
const BUCKET: &str = "orders-archive";

/// The key the handler writes and reads, whose `/` is what makes the object's key a path
/// rather than one segment.
const KEY: &str = "orders/one.txt";

/// Where the handler COPIES that object to.
const COPY: &str = "orders/copy.txt";

/// The object's own bytes, which the handler passes as `bytes` and reads back.
const OBJECT_BODY: &str = "first order";

/// Lambda and Amazon S3 over ONE shared engine, as `core/router/src/lib.rs` mounts them: the
/// handler's SDK call is dispatched back into this same router, so the bucket it writes to is
/// the bucket this suite reads.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One Amazon S3 call made as a CLIENT makes it — path-style, signed for `s3` — answering its
/// status and its body bytes.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, bytes::Bytes) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=not-verified"
            ),
        )
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, response.into_body())
}

/// The handler the suite runs: one step per binding class, each reported under its own name,
/// so what is asserted is what the SDK gave the guest.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions
import botocore.response


def lambda_handler(event, context):
    bucket = event["bucket"]
    key = event["key"]
    s3 = boto3.client("s3")
    steps = {}

    # A write carrying the handler's own BYTES, answered with the object's own metadata.
    written = s3.put_object(
        Bucket=bucket, Key=key, Body=event["body"].encode("utf-8"), ContentType="text/plain"
    )
    steps["written"] = {
        "status": written["ResponseMetadata"]["HTTPStatusCode"],
        "etag_quoted": written.get("ETag", "").startswith('"'),
    }

    # The read: the payload is the object's bytes, handed over as a stream, and the rest of
    # the answer is its metadata.
    read = s3.get_object(Bucket=bucket, Key=key)
    steps["read"] = {
        "streaming": isinstance(read["Body"], botocore.response.StreamingBody),
        "body": read["Body"].read().decode("utf-8"),
        "content_length": read.get("ContentLength"),
        "content_type": read.get("ContentType"),
    }

    # The metadata alone, with no payload at all.
    head = s3.head_object(Bucket=bucket, Key=key)
    steps["head"] = {
        "content_length": head.get("ContentLength"),
        "content_type": head.get("ContentType"),
        "has_body": "Body" in head,
    }

    # A copy, whose answer NESTS a structure of its own.
    copied = s3.copy_object(
        Bucket=bucket, Key=event["copy"], CopySource="%s/%s" % (bucket, key)
    )
    steps["copied"] = {
        "etag_quoted": copied.get("CopyObjectResult", {}).get("ETag", "").startswith('"'),
        "has_last_modified": "LastModified" in copied.get("CopyObjectResult", {}),
    }

    # A listing bound by QUERY parameters, answering a list of structures beside typed
    # scalars.
    listed = s3.list_objects_v2(Bucket=bucket, Prefix="orders/")
    steps["listed"] = {
        "keys": sorted(one["Key"] for one in listed.get("Contents", [])),
        "sizes": sorted(one["Size"] for one in listed.get("Contents", [])),
        "key_count": listed.get("KeyCount"),
        "name": listed.get("Name"),
        "prefix": listed.get("Prefix"),
        "truncated": listed.get("IsTruncated"),
    }

    # A listing that matches nothing carries no list at all, which is what an SDK client
    # reads: the member is absent rather than empty.
    nothing = s3.list_objects_v2(Bucket=bucket, Prefix="nothing/")
    steps["empty"] = {
        "has_contents": "Contents" in nothing,
        "key_count": nothing.get("KeyCount"),
    }

    # The delete, and the read that no longer finds it: a refusal is the SERVICE's own, its
    # code and its message reaching the handler.
    s3.delete_object(Bucket=bucket, Key=event["copy"])
    try:
        s3.get_object(Bucket=bucket, Key=event["copy"])
    except botocore.exceptions.ClientError as error:
        steps["deleted"] = {
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
        }
    else:
        steps["deleted"] = None

    return {"steps": steps}
"#;

/// One row of the table: the step of the handler's chain, and what the SDK must have given the
/// guest for it.
struct Row {
    case: &'static str,
    /// The step the handler reported it under.
    step: &'static str,
    /// The document the guest must have read back.
    expected: Value,
}

/// Every binding class the runtime's SDK binds for this service, in one invocation, each
/// asserted on what the guest read back — and then on what S3 ITSELF holds.
#[tokio::test]
async fn a_handler_calls_amazon_s3_through_the_sdk_the_runtime_supplies() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler calling Amazon S3 through the runtime's SDK";
    let router = emulator();

    // The bucket exists before the handler runs, created the way a client creates one.
    let (created_bucket, body) = s3(&router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created_bucket,
        200,
        "case {case:?}: expected CreateBucket to answer 200, got {created_bucket} ({})",
        String::from_utf8_lossy(&body)
    );

    let caller = "object-storage-sdk-caller";
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": caller,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 10,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[("lambda_function.py", HANDLER_SOURCE)])),
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
    await_function_active(&router, case, caller).await;

    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{caller}/invocations"),
        &json!({
            "bucket": BUCKET,
            "key": KEY,
            "copy": COPY,
            "body": OBJECT_BODY,
        }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    let payload = answered.json();
    let steps = payload["steps"].clone();

    let rows = vec![
        Row {
            case: "a write carrying the handler's own bytes",
            step: "written",
            expected: json!({ "status": 200, "etag_quoted": true }),
        },
        Row {
            case: "a read whose payload is the object's bytes, handed over as a stream",
            step: "read",
            expected: json!({
                "streaming": true,
                "body": OBJECT_BODY,
                "content_length": OBJECT_BODY.len(),
                "content_type": "text/plain",
            }),
        },
        Row {
            case: "a read of the metadata alone, with no payload",
            step: "head",
            expected: json!({
                "content_length": OBJECT_BODY.len(),
                "content_type": "text/plain",
                "has_body": false,
            }),
        },
        Row {
            case: "a copy, whose answer nests a structure of its own",
            step: "copied",
            expected: json!({ "etag_quoted": true, "has_last_modified": true }),
        },
        Row {
            case: "a listing bound by query parameters",
            step: "listed",
            expected: json!({
                "keys": [COPY, KEY],
                "sizes": [OBJECT_BODY.len(), OBJECT_BODY.len()],
                "key_count": 2,
                "name": BUCKET,
                "prefix": "orders/",
                "truncated": false,
            }),
        },
        Row {
            case: "a listing that matches nothing, whose list member is absent",
            step: "empty",
            expected: json!({ "has_contents": false, "key_count": 0 }),
        },
        Row {
            case: "a read of what the delete removed, refused by the service itself",
            step: "deleted",
            expected: json!({
                "code": "NoSuchKey",
                "message": "The specified key does not exist.",
            }),
        },
    ];
    for row in &rows {
        let actual = steps[row.step].clone();
        assert_eq!(
            actual, row.expected,
            "case {:?}: expected {}, got {actual} (the whole run: {payload})",
            row.case, row.expected
        );
    }

    // What S3 ITSELF holds: the work the calls reported is the work that happened, read
    // through that service's own API the way a client reads it.
    let (status, held) = s3(&router, "GET", &format!("/{BUCKET}/{KEY}"), b"").await;
    assert_eq!(
        (status, String::from_utf8_lossy(&held).to_string()),
        (200, OBJECT_BODY.to_string()),
        "case {case:?}: expected S3 to serve the handler's own object"
    );
    let (status, gone) = s3(&router, "GET", &format!("/{BUCKET}/{COPY}"), b"").await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the copy the handler deleted to be gone, got {status} ({})",
        String::from_utf8_lossy(&gone)
    );
}

/// The handler the node row runs: the same two binding classes through the v3 SDK this
/// runtime supplies — a command carrying bytes, and an answer whose payload is a stream.
const NODE_HANDLER_SOURCE: &str = r#"
const { S3Client, PutObjectCommand, GetObjectCommand } = require('@aws-sdk/client-s3');

exports.handler = async (event) => {
    const s3 = new S3Client({ region: process.env.AWS_REGION });
    await s3.send(new PutObjectCommand({
        Bucket: event.bucket,
        Key: event.key,
        Body: Buffer.from(event.body, 'utf8')
    }));
    const read = await s3.send(new GetObjectCommand({
        Bucket: event.bucket,
        Key: event.key
    }));
    return {
        body: await read.Body.transformToString(),
        content_length: read.ContentLength
    };
};
"#;

/// The rule holds for the OTHER language this runtime ships an SDK for: the same binding of
/// the bytes a command carries and the same stream an answer's payload is handed over as,
/// through the v3 SDK a `nodejs*` runtime supplies.
#[tokio::test]
async fn a_node_handler_reaches_the_same_service_through_the_sdk_the_runtime_supplies() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a node handler calling Amazon S3 through the runtime's SDK";
    let router = emulator();
    let key = "orders/node.txt";
    let body = "second order";

    let (created_bucket, answered) = s3(&router, "PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        created_bucket,
        200,
        "case {case:?}: expected CreateBucket to answer 200, got {created_bucket} ({})",
        String::from_utf8_lossy(&answered)
    );

    let caller = "object-storage-node-caller";
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": caller,
            "Role": ROLE,
            "Runtime": "nodejs22.x",
            "Handler": "index.handler",
            "Timeout": 10,
            "MemorySize": 256,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(&[("index.js", NODE_HANDLER_SOURCE)])),
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
    await_function_active(&router, case, caller).await;

    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{caller}/invocations"),
        &json!({ "bucket": BUCKET, "key": key, "body": body }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    let payload = answered.json();
    assert_eq!(
        payload,
        json!({ "body": body, "content_length": body.len() }),
        "case {case:?}: expected the handler to read its own object back, got {payload}"
    );

    // And S3 itself holds the bytes the command carried, read where a client reads them.
    let (status, held) = s3(&router, "GET", &format!("/{BUCKET}/{key}"), b"").await;
    assert_eq!(
        (status, String::from_utf8_lossy(&held).to_string()),
        (200, body.to_string()),
        "case {case:?}: expected S3 to serve the node handler's own object"
    );
}
