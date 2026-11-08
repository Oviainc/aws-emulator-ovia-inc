// SPDX-License-Identifier: CC0-1.0

//! What a managed `python3.x` runtime PRESENTS to a handler, end to end over the wire:
//! Lambda, IAM, the token service and Amazon S3 on ONE engine, a function whose code imports
//! the standard library and the AWS SDK and then reads and writes objects, and the objects
//! read back where a client reads them.
//!
//! RULE: a managed Python runtime presents the COMPLETE standard library and the AWS SDK it
//! ships on the handler's import path, and a call the handler makes with either — an SDK
//! operation of any service this emulator serves, whatever protocol that service speaks, or
//! an HTTP request the handler signs itself — reaches that service AS THE FUNCTION'S OWN
//! EXECUTION ROLE and answers with that service's own answer: the operation's output members
//! for a success, and the service's own error code and message for a refusal. The credential
//! the request is signed with is never the handler's to read.
//!
//! The rows differ along what the rule generalises over: WHICH standard-library module is
//! imported (one row per module that a runtime missing `os.stat_result` or `os.O_RDONLY`
//! could not import), WHICH binding class of the object store's protocol the operation uses
//! (a payload read, a payload write, a listing whose answer is an XML document, a read whose
//! answer is all headers, a copy whose source travels as a header, a delete answering no
//! content), whether the call is served or refused, and HOW the call was made (through the
//! SDK, as a URL the SDK presigned, or as an HTTP request the handler signed and sent
//! itself). The last assertions read the work back through the object store's OWN API, so a
//! call that answered without doing the work fails here.
//!
//! The suite RUNS a handler, so it takes the target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]).

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_source_mapping_role_access::{element, given_role, iam};
use crate::event_source_mappings::{await_function_active, call, router_on};

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The object the handler reads, and what it holds.
const KEY: &str = "reports/first.json";
const CONTENT: &str = "{\"report\": \"first\"}";

/// Lambda, IAM, the token service and Amazon S3 over ONE shared engine, as
/// `core/router/src/lib.rs` mounts them: the handler's own calls are dispatched back into
/// this same router, so the bucket it writes to is the bucket this suite reads, and the role
/// it runs as is the role IAM holds.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = router_on(&engine);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router
}

/// One request to Amazon S3, made as a CLIENT makes one: its protocol is `restXml`, so the
/// request is a method and a path and the answer is a document.
async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(path)
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
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// The standard-library modules the handler imports, one row each.
///
/// Every one of them reads a name off `os` that only a COMPLETE operating-system module
/// carries — `shutil` and the three modules that import it read `os.stat_result`, `glob` and
/// `pathlib` read `os.O_RDONLY`, `random` (and so `secrets` and `statistics`) registers an
/// at-fork hook, and `http.client` and `urllib.request` are built on `socket` — so a runtime
/// presenting less than the whole of it cannot import them at all.
const IMPORTED_MODULES: &[&str] = &[
    "shutil",
    "zipfile",
    "tempfile",
    "tarfile",
    "pathlib",
    "glob",
    "random",
    "secrets",
    "statistics",
    "socket",
    "http.client",
    "urllib.request",
    "importlib.metadata",
];

/// The handler this suite runs: one step per class the rule covers, each reported under its
/// own name so what is asserted is what the runtime gave the guest.
const HANDLER_SOURCE: &str = r#"
import json
import os

import boto3
import botocore.exceptions
import urllib.request
from botocore.auth import SigV4Auth
from botocore.awsrequest import AWSRequest


def _imported(names):
    """Which of `names` this runtime can import, and what stopped the rest."""
    read = {}
    for name in names:
        try:
            __import__(name)
            read[name] = "ok"
        except BaseException as error:
            read[name] = repr(error)
    return read


def lambda_handler(event, context):
    steps = {"imports": _imported(event["modules"])}

    # The TYPE an operating-system module reports a file's attributes as: a sequence whose
    # fields are also reachable by name.
    attributes = os.stat_result((33188, 2, 3, 1, 0, 0, 12, 1.0, 2.0, 3.0))
    steps["stat_result"] = [
        attributes.st_mode,
        attributes.st_size,
        attributes[6],
        len(attributes),
    ]

    bucket = event["bucket"]
    key = event["key"]
    region = os.environ["AWS_REGION"]
    client = boto3.client("s3")

    # A payload read: the object's bytes are a stream the caller reads.
    try:
        read = client.get_object(Bucket=bucket, Key=key)
        steps["get"] = {
            "body": read["Body"].read().decode("utf-8"),
            "content_type": read.get("ContentType"),
        }
    except botocore.exceptions.ClientError as error:
        steps["get"] = {
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
        }

    # A listing, whose answer is an XML document: the keys, and the count as the NUMBER the
    # model types it as.
    listed = client.list_objects_v2(Bucket=bucket, Prefix="reports/")
    steps["list"] = {
        "keys": [one["Key"] for one in listed.get("Contents", [])],
        "key_count": listed.get("KeyCount"),
        "counted_as_number": isinstance(listed.get("KeyCount"), int),
        "name": listed.get("Name"),
    }

    # A read whose answer is all headers, with its own types.
    head = client.head_object(Bucket=bucket, Key=key)
    steps["head"] = {
        "length": head.get("ContentLength"),
        "measured_as_number": isinstance(head.get("ContentLength"), int),
        "etag_present": bool(head.get("ETag")),
    }

    # A payload WRITE, of bytes the handler made itself.
    written = event["written"]
    put = client.put_object(
        Bucket=bucket,
        Key=written,
        Body=b"written by the handler",
        ContentType="text/plain",
    )
    steps["put"] = {"etag_present": bool(put.get("ETag"))}

    # A COPY, whose source travels as a header the SDK renders from the bucket and the key.
    copied = event["copied"]
    copy = client.copy_object(
        Bucket=bucket, Key=copied, CopySource={"Bucket": bucket, "Key": key}
    )
    steps["copy"] = {
        "etag_present": bool((copy.get("CopyObjectResult") or {}).get("ETag"))
    }

    # A DELETE, which answers no content at all, and the read that shows it happened.
    client.delete_object(Bucket=bucket, Key=written)
    try:
        client.head_object(Bucket=bucket, Key=written)
        steps["deleted"] = "still there"
    except botocore.exceptions.ClientError as error:
        steps["deleted"] = error.response["Error"]["Code"]

    # A refusal is the SERVICE's own code and message.
    try:
        client.get_object(Bucket=bucket, Key="reports/absent.json")
        steps["missing"] = None
    except botocore.exceptions.ClientError as error:
        steps["missing"] = {
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"]["Message"],
        }

    # A URL the SDK PRESIGNS, which carries its own signature.
    steps["presigned"] = client.generate_presigned_url(
        "get_object", Params={"Bucket": bucket, "Key": key}, ExpiresIn=120
    )

    # What the handler holds of the credential its calls are signed with: nothing.
    credentials = boto3.Session().get_credentials()
    steps["credentials"] = {
        "access_key": credentials.access_key,
        "secret_key": credentials.secret_key,
        "token": credentials.token,
    }

    # An HTTP request the handler SIGNS AND SENDS ITSELF, which is how a data plane
    # addressed by its own endpoint is reached.
    url = "https://s3.%s.amazonaws.com/%s/%s" % (region, bucket, key)
    signed = AWSRequest(method="GET", url=url, headers={})
    SigV4Auth(credentials, "s3", region).add_auth(signed)
    sent = urllib.request.Request(url, headers=dict(signed.headers), method="GET")
    try:
        with urllib.request.urlopen(sent) as answer:
            steps["sent"] = {
                "status": answer.status,
                "body": answer.read().decode("utf-8"),
            }
    except BaseException as error:
        steps["sent"] = {"failed": repr(error)}

    # And one addressed OFF this emulator, which is no address a handler can reach.
    try:
        urllib.request.urlopen("https://example.test/anything")
        steps["off_box"] = "served"
    except BaseException as error:
        steps["off_box"] = type(error).__name__

    return steps
"#;

/// The policy a role that may read and write the bucket carries.
fn object_access() -> Value {
    json!([{ "Effect": "Allow", "Action": "s3:*", "Resource": "*" }])
}

/// Create the function `name`, running as `role_arn`, out of [`HANDLER_SOURCE`].
async fn given_function(router: &MultiServiceRouter, case: &str, name: &str, role_arn: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": role_arn,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::synchronous_invoke_run::package(HANDLER_SOURCE)),
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
    await_function_active(router, case, name).await;
}

/// Invoke `name` over the bucket `bucket`, and answer what the handler returned.
async fn invoked(router: &MultiServiceRouter, case: &str, name: &str, bucket: &str) -> Value {
    let answered = call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/invocations"),
        &json!({
            "bucket": bucket,
            "key": KEY,
            "written": "reports/written.txt",
            "copied": "reports/copied.json",
            "modules": IMPORTED_MODULES,
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
    answered.json()
}

/// Everything the runtime presents, in one invocation: the standard library, the SDK over an
/// object store's own protocol, a presigned URL, a request the handler signed itself — and
/// the objects read back through the store's OWN API.
#[tokio::test]
async fn a_handler_reads_and_writes_objects_with_what_the_runtime_presents() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler calling an object store through the runtime's SDK";
    let router = emulator();
    let bucket = "runtime-sdk-objects";
    let (created, body) = s3(&router, "PUT", &format!("/{bucket}"), b"").await;
    assert!(
        created == 200,
        "case {case:?}: expected CreateBucket to answer 200, got {created}: {body}"
    );
    let (stored, body) = s3(
        &router,
        "PUT",
        &format!("/{bucket}/{KEY}"),
        CONTENT.as_bytes(),
    )
    .await;
    assert!(
        stored == 200,
        "case {case:?}: expected PutObject to answer 200, got {stored}: {body}"
    );

    let role = given_role(&router, case, "runtime-sdk-role", Some(object_access())).await;
    let function = "runtime-sdk-reader";
    given_function(&router, case, function, &role).await;
    let steps = invoked(&router, case, function, bucket).await;

    // One row per standard-library module: every one of them imports.
    for module in IMPORTED_MODULES {
        assert_eq!(
            steps["imports"][*module],
            json!("ok"),
            "case {module:?}: expected the module to import, got {}",
            steps["imports"]
        );
    }
    assert_eq!(
        steps["stat_result"],
        json!([33188, 12, 12, 10]),
        "case {case:?}: expected the stat fields by name, by index and by length, got {steps}"
    );

    // The SDK's own calls, one row per binding class.
    assert_eq!(
        steps["get"]["body"],
        json!(CONTENT),
        "case {case:?}: expected the object's own bytes, got {}",
        steps["get"]
    );
    assert_eq!(
        steps["list"]["keys"],
        json!([KEY]),
        "case {case:?}: expected the listing to name the object, got {}",
        steps["list"]
    );
    assert_eq!(
        steps["list"]["counted_as_number"],
        json!(true),
        "case {case:?}: expected KeyCount as the number it is typed as, got {}",
        steps["list"]
    );
    assert_eq!(
        steps["list"]["name"],
        json!(bucket),
        "case {case:?}: expected the listing to name the bucket, got {}",
        steps["list"]
    );
    assert_eq!(
        steps["head"]["length"],
        json!(CONTENT.len()),
        "case {case:?}: expected the stored length, got {}",
        steps["head"]
    );
    assert_eq!(
        steps["head"]["measured_as_number"],
        json!(true),
        "case {case:?}: expected ContentLength as the number it is typed as, got {}",
        steps["head"]
    );
    assert_eq!(
        steps["put"]["etag_present"],
        json!(true),
        "case {case:?}: expected the write to answer its ETag, got {}",
        steps["put"]
    );
    assert_eq!(
        steps["copy"]["etag_present"],
        json!(true),
        "case {case:?}: expected the copy to answer its result, got {}",
        steps["copy"]
    );
    assert_eq!(
        steps["deleted"],
        json!("404"),
        "case {case:?}: expected the deleted object to be gone, got {}",
        steps["deleted"]
    );
    assert_eq!(
        steps["missing"]["code"],
        json!("NoSuchKey"),
        "case {case:?}: expected the service's own error code, got {}",
        steps["missing"]
    );

    // A presigned URL addresses the operation and carries its own signature.
    let presigned = steps["presigned"].as_str().unwrap_or_default().to_string();
    for carried in [
        bucket,
        "X-Amz-Algorithm=AWS4-HMAC-SHA256",
        "X-Amz-Expires=120",
        "X-Amz-Signature=",
    ] {
        assert!(
            presigned.contains(carried),
            "case {carried:?}: expected the presigned URL to carry it, got {presigned:?}"
        );
    }

    // The credential the calls are signed with is the runtime's, and the handler holds none
    // of it.
    for member in ["access_key", "secret_key", "token"] {
        assert_eq!(
            steps["credentials"][member],
            Value::Null,
            "case {member:?}: expected the handler to hold no credential, got {}",
            steps["credentials"]
        );
    }

    // The request the handler signed and sent itself reached the service.
    assert_eq!(
        steps["sent"]["status"],
        json!(200),
        "case {case:?}: expected the handler's own request to be answered, got {}",
        steps["sent"]
    );
    assert_eq!(
        steps["sent"]["body"],
        json!(CONTENT),
        "case {case:?}: expected the object's own bytes over HTTP, got {}",
        steps["sent"]
    );
    assert_ne!(
        steps["off_box"],
        json!("served"),
        "case {case:?}: expected an address off this emulator to be unreachable, got {}",
        steps["off_box"]
    );

    // What the STORE itself holds: the copy the handler made is there with the object's own
    // bytes, and the object it deleted is gone.
    let (status, body) = s3(&router, "GET", &format!("/{bucket}/reports/copied.json"), b"").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the copied object to be held by the store, got {status}: \
         {body}"
    );
    assert_eq!(
        body, CONTENT,
        "case {case:?}: expected the copy to carry the object's bytes, got {body:?}"
    );
    let (status, _) = s3(
        &router,
        "GET",
        &format!("/{bucket}/reports/written.txt"),
        b"",
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the deleted object to be gone from the store, got {status}"
    );
}

/// WHO the call is made as: the function's own execution role, which is what the service
/// authorizing it decides about.
///
/// The rows are the two halves of ONE role's own document, read by the service that serves
/// the call: a role whose only grant is the listing may list the bucket and may NOT read an
/// object out of it, and the refusal names the assumed-role SESSION of that role and that
/// function — the principal AWS names for a function's own call.
const IDENTITY_HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def lambda_handler(event, context):
    client = boto3.client("s3")
    steps = {}
    attempts = (
        ("listed", lambda: client.list_objects_v2(Bucket=event["bucket"])),
        ("read", lambda: client.get_object(Bucket=event["bucket"], Key=event["key"])),
    )
    for label, attempt in attempts:
        try:
            attempt()
            steps[label] = {"ok": True}
        except botocore.exceptions.ClientError as error:
            steps[label] = {
                "ok": False,
                "code": error.response["Error"]["Code"],
                "message": error.response["Error"]["Message"],
            }
    return steps
"#;

/// The policy of the role the row below runs as: the listing, and nothing else.
fn listing_only(bucket: &str) -> Value {
    json!([{
        "Effect": "Allow",
        "Action": "s3:ListBucket",
        "Resource": format!("arn:aws:s3:::{bucket}"),
    }])
}

#[tokio::test]
async fn a_handlers_call_is_made_as_the_functions_execution_role() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let case = "a handler running as a role whose only grant is the listing";
    let router = emulator();
    let bucket = "runtime-sdk-denied";
    let (created, body) = s3(&router, "PUT", &format!("/{bucket}"), b"").await;
    assert!(
        created == 200,
        "case {case:?}: expected CreateBucket to answer 200, got {created}: {body}"
    );
    let (stored, _) = s3(
        &router,
        "PUT",
        &format!("/{bucket}/{KEY}"),
        CONTENT.as_bytes(),
    )
    .await;
    assert_eq!(
        stored, 200,
        "case {case:?}: expected PutObject to answer 200, got {stored}"
    );
    let name = "runtime-sdk-lister-role";
    let role = given_role(&router, case, name, Some(listing_only(bucket))).await;
    let function = "runtime-sdk-lister";
    let created = call(
        &router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": function,
            "Role": role,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 60,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(crate::synchronous_invoke_run::package(IDENTITY_HANDLER_SOURCE)),
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
    await_function_active(&router, case, function).await;
    let answered = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{function}/invocations"),
        &json!({ "bucket": bucket, "key": KEY }),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: expected HTTP 200 from Invoke, got {} {}",
        answered.status,
        answered.text()
    );
    let steps = answered.json();

    // What the role's document ALLOWS is served.
    assert_eq!(
        steps["listed"]["ok"],
        json!(true),
        "case {case:?}: expected the listing the role allows to be served, got {}",
        steps["listed"]
    );
    // What it does not is refused, and the refusal names the session of THIS role run for
    // THIS function.
    assert_eq!(
        steps["read"]["code"],
        json!("AccessDenied"),
        "case {case:?}: expected the read the role does not allow to be refused, got {}",
        steps["read"]
    );
    let message = steps["read"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let expected = format!("assumed-role/{name}/{function}");
    assert!(
        message.contains(&expected),
        "case {case:?}: expected the refusal to name {expected:?}, got {message:?}"
    );
    // And IAM holds exactly the role the function names, so what decided the calls is the
    // document that service holds.
    let (status, read) = iam(&router, "GetRole", &[("RoleName", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRole to answer 200, got {status}: {read}"
    );
    assert_eq!(
        element(&read, "Arn"),
        role,
        "case {case:?}: expected the role the function runs as, got {read}"
    );
}
