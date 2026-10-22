// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the registered data-lake location: `RegisterResource` and the
//! `DescribeResource` read that proves what it persisted.
//!
//! Every call goes over the wire the way an SDK client does — a `restJson1` `POST` of a
//! JSON body through the router that claims `lakeformation` — so a response can only be
//! what the handler computed from the single shared engine store.
//!
//! The tables below span the classes the rules generalise over: which member names the
//! data access role, which optional flags the request carries, the ARN's service and
//! resource form, and the malformed shapes `ResourceArnString` admits (the model gives it
//! no pattern). The corpus' own request — an Amazon S3 location registered with a role —
//! is one row among them.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_lakeformation_awslakeformation::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator over one shared engine, reached exactly as the production router reaches
/// it: `MultiServiceRouter::dispatch` of a buffered HTTP request.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// `POST /<operation>` with `body` as the JSON payload, answering `(status, body)`.
///
/// The `Authorization` credential scope names `lakeformation`, which is the routing
/// signal `resolve_route_key` reads first; no authenticator is registered, so the
/// signature itself is not checked.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://lakeformation.us-east-1.amazonaws.com/{operation}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20260101/us-east-1/lakeformation/aws4_request, \
             SignedHeaders=host, Signature=0000",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize the request body"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// The wire error code (`__type`) of an error envelope, or `""` for a success body.
fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The service-linked role a registration carries when the request sets
/// `UseServiceLinkedRole` and names no `RoleArn`: the role `RegisterResource`'s own
/// documentation names, in this emulator's account.
fn service_linked_role() -> String {
    format!(
        "arn:aws:iam::{}:role/aws-service-role/lakeformation.amazonaws.com/AWSServiceRoleForLakeFormationDataAccess",
        engine::emulator_account_id()
    )
}

/// `DescribeResource`'s `ResourceInfo` for `arn`, with the status it answered.
async fn describe(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "DescribeResource", &json!({ "ResourceArn": arn })).await
}

/// The `ResourceInfo` of a `DescribeResource` payload with `LastModified` split off, so a
/// test can compare the request-derived members exactly and the clock-derived one
/// separately.
fn split_info(payload: &Value, case: &str) -> (Map<String, Value>, Value) {
    let mut info = payload
        .get("ResourceInfo")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceInfo, got {payload:?}"));
    let last_modified = info.remove("LastModified").unwrap_or(Value::Null);
    (info, last_modified)
}

/// Seconds since the Unix epoch a `LastModifiedTimestamp` carries on the wire.
///
/// `restJson1` renders a timestamp with no `@timestampFormat` as epoch seconds; a string
/// rendering is parsed too so the assertion tests the VALUE, not the encoding.
fn epoch_seconds(value: &Value, case: &str) -> f64 {
    match value {
        Value::Number(seconds) => seconds
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: LastModified {seconds} is not a number")),
        Value::String(text) => aws_smithy_types::DateTime::from_str(
            text,
            aws_smithy_types::date_time::Format::DateTime,
        )
        .unwrap_or_else(|error| {
            panic!("case {case:?}: LastModified {text:?} is not a timestamp: {error}")
        })
        .as_secs_f64(),
        other => panic!("case {case:?}: expected a LastModified timestamp, got {other:?}"),
    }
}

/// Now, in seconds since the Unix epoch.
fn now_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// One register-then-describe row: the request, and the `ResourceInfo` it must read back
/// (every member but `LastModified`).
struct Row {
    case: &'static str,
    request: Value,
    expected_info: Value,
}

#[tokio::test]
async fn register_then_describe_reads_the_registration_back() {
    let slr = service_linked_role();
    // Rows differ along what the rule generalises over: which member names the data
    // access role (`RoleArn`, or `UseServiceLinkedRole` with none), which optional flags
    // the request carries, the ARN's service and resource form (bucket root, one prefix,
    // a nested prefix, a non-Amazon-S3 resource), and whether
    // `ExpectedResourceOwnerAccount` is named.
    let rows = vec![
        Row {
            case: "bucket root registered with a role",
            request: json!({
                "ResourceArn": "arn:aws:s3:::lf-bucket",
                "RoleArn": "arn:aws:iam::123456789012:role/my-data-access-role",
            }),
            expected_info: json!({
                "ResourceArn": "arn:aws:s3:::lf-bucket",
                "RoleArn": "arn:aws:iam::123456789012:role/my-data-access-role",
                "WithPrivilegedAccess": false,
            }),
        },
        Row {
            case: "trailing-slash prefix registered with the service-linked role",
            request: json!({
                "ResourceArn": "arn:aws:s3:::lf-bucket/warehouse/",
                "UseServiceLinkedRole": true,
            }),
            expected_info: json!({
                "ResourceArn": "arn:aws:s3:::lf-bucket/warehouse/",
                "RoleArn": slr,
                "WithPrivilegedAccess": false,
            }),
        },
        Row {
            case: "nested prefix with every optional flag set",
            request: json!({
                "ResourceArn": "arn:aws:s3:::lf-bucket/warehouse/sales/",
                "RoleArn": "arn:aws:iam::210987654321:role/service-role/lf-role",
                "WithFederation": true,
                "HybridAccessEnabled": true,
                "WithPrivilegedAccess": true,
                "ExpectedResourceOwnerAccount": "210987654321",
            }),
            expected_info: json!({
                "ResourceArn": "arn:aws:s3:::lf-bucket/warehouse/sales/",
                "RoleArn": "arn:aws:iam::210987654321:role/service-role/lf-role",
                "WithFederation": true,
                "HybridAccessEnabled": true,
                "WithPrivilegedAccess": true,
                "ExpectedResourceOwnerAccount": "210987654321",
            }),
        },
        Row {
            case: "flags explicitly false are reported false, not dropped",
            request: json!({
                "ResourceArn": "arn:aws:s3:::other-bucket/raw/",
                "RoleArn": "arn:aws:iam::123456789012:role/my-data-access-role",
                "WithFederation": false,
                "HybridAccessEnabled": false,
                "UseServiceLinkedRole": false,
            }),
            expected_info: json!({
                "ResourceArn": "arn:aws:s3:::other-bucket/raw/",
                "RoleArn": "arn:aws:iam::123456789012:role/my-data-access-role",
                "WithFederation": false,
                "HybridAccessEnabled": false,
                "WithPrivilegedAccess": false,
            }),
        },
        Row {
            // `ResourceArnString` is not restricted to Amazon S3, so an ARN of another
            // service with a region and an account id registers the same way.
            case: "a non-Amazon-S3 resource ARN",
            request: json!({
                "ResourceArn": "arn:aws:glue:us-east-1:123456789012:database/sales",
                "RoleArn": "arn:aws:iam::123456789012:role/my-data-access-role",
                "WithPrivilegedAccess": true,
            }),
            expected_info: json!({
                "ResourceArn": "arn:aws:glue:us-east-1:123456789012:database/sales",
                "RoleArn": "arn:aws:iam::123456789012:role/my-data-access-role",
                "WithPrivilegedAccess": true,
            }),
        },
    ];

    let router = emulator();
    let before = now_seconds();
    for row in &rows {
        let (status, payload) = call(&router, "RegisterResource", &row.request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected RegisterResource 200, got {status} {payload:?}",
            row.case
        );
    }

    for row in &rows {
        let arn = row.request["ResourceArn"].as_str().expect("an ARN");
        let (status, payload) = describe(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeResource 200, got {status} {payload:?}",
            row.case
        );
        let (info, last_modified) = split_info(&payload, row.case);
        assert_eq!(
            Value::Object(info.clone()),
            row.expected_info,
            "case {:?}: expected ResourceInfo {:?}, got {:?}",
            row.case,
            row.expected_info,
            Value::Object(info)
        );
        // The registration was created by these calls, so its LastModified lies between
        // the instant before the registers and now; `VerificationStatus` is absent
        // because no access verification ran.
        let seconds = epoch_seconds(&last_modified, row.case);
        assert!(
            seconds >= before - 1.0 && seconds <= now_seconds() + 1.0,
            "case {:?}: expected LastModified within [{before}, now], got {seconds}",
            row.case
        );
    }
}

#[tokio::test]
async fn re_registering_a_resource_is_refused_and_leaves_the_registration_unchanged() {
    // Rows differ in how the second request differs from the first: a different role, the
    // service-linked role instead of a role, and an identical repeat.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "a different role",
            "arn:aws:s3:::dup-bucket/a/",
            json!({ "RoleArn": "arn:aws:iam::123456789012:role/other-role" }),
        ),
        (
            "the service-linked role",
            "arn:aws:s3:::dup-bucket/b/",
            json!({ "UseServiceLinkedRole": true }),
        ),
        (
            "an identical repeat",
            "arn:aws:s3:::dup-bucket/c/",
            json!({ "RoleArn": "arn:aws:iam::123456789012:role/first-role" }),
        ),
    ];

    let router = emulator();
    for (case, arn, second) in &cases {
        let first = json!({
            "ResourceArn": arn,
            "RoleArn": "arn:aws:iam::123456789012:role/first-role",
            "WithFederation": true,
        });
        let (status, payload) = call(&router, "RegisterResource", &first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first RegisterResource to succeed, got {status} {payload:?}"
        );

        let mut repeat = second.clone();
        repeat["ResourceArn"] = json!(arn);
        let (status, payload) = call(&router, "RegisterResource", &repeat).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the second RegisterResource to be refused with 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "AlreadyExistsException",
            "case {case:?}: expected AlreadyExistsException, got {payload:?}"
        );

        // The refused call changed nothing: the live registration still carries the role
        // and the flag the first one set.
        let (status, payload) = describe(&router, arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeResource 200, got {status} {payload:?}"
        );
        let (info, _) = split_info(&payload, case);
        assert_eq!(
            info.get("RoleArn").and_then(Value::as_str),
            Some("arn:aws:iam::123456789012:role/first-role"),
            "case {case:?}: expected the first role to survive, got {info:?}"
        );
        assert_eq!(
            info.get("WithFederation").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected the first registration's flags to survive, got {info:?}"
        );
    }
}

#[tokio::test]
async fn a_request_naming_no_data_access_role_is_refused_and_persists_nothing() {
    // "If UseServiceLinkedRole is not set to true, you must provide or set the RoleArn"
    // (`RegisterResource`'s documentation). Rows span the ways a request can name no
    // role: the member absent, set false, and set false beside other flags.
    let cases: Vec<(&str, Value)> = vec![
        (
            "neither member",
            json!({ "ResourceArn": "arn:aws:s3:::no-role-bucket/a/" }),
        ),
        (
            "UseServiceLinkedRole false",
            json!({
                "ResourceArn": "arn:aws:s3:::no-role-bucket/b/",
                "UseServiceLinkedRole": false,
            }),
        ),
        (
            "UseServiceLinkedRole false beside other flags",
            json!({
                "ResourceArn": "arn:aws:s3:::no-role-bucket/c/",
                "UseServiceLinkedRole": false,
                "WithFederation": true,
                "WithPrivilegedAccess": true,
            }),
        ),
    ];

    let router = emulator();
    for (case, request) in &cases {
        let (status, payload) = call(&router, "RegisterResource", request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {payload:?}"
        );

        let arn = request["ResourceArn"].as_str().expect("an ARN");
        let (status, payload) = describe(&router, arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refused registration to persist nothing, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException, got {payload:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_refused_by_both_operations() {
    // `ResourceArnString` carries no pattern in the model, so these all reach the
    // handler: it is the handler that must refuse them as invalid input rather than
    // registering or "not finding" them. Rows span each field of
    // arn:<partition>:<service>:<region>:<account-id>:<resource> that may not be empty,
    // plus a missing field and a value that is no ARN at all.
    let cases: Vec<(&str, &str)> = vec![
        ("empty", ""),
        ("not an ARN", "lf-bucket"),
        ("an S3 URI", "s3://lf-bucket/warehouse/"),
        ("missing the resource field", "arn:aws:s3:::"),
        ("only five fields", "arn:aws:s3::lf-bucket"),
        ("an empty partition", "arn::s3:::lf-bucket"),
        ("an empty service", "arn:aws::::lf-bucket"),
        ("the wrong prefix", "urn:aws:s3:::lf-bucket"),
    ];

    let router = emulator();
    for (case, arn) in &cases {
        for operation in ["RegisterResource", "DescribeResource"] {
            let mut request = json!({ "ResourceArn": arn });
            if operation == "RegisterResource" {
                request["RoleArn"] = json!("arn:aws:iam::123456789012:role/my-data-access-role");
            }
            let (status, payload) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected 400, got {status} {payload:?}"
            );
            assert_eq!(
                error_code(&payload),
                "InvalidInputException",
                "case {case:?} on {operation}: expected InvalidInputException, got {payload:?}"
            );
        }
    }
}

#[tokio::test]
async fn describing_a_resource_that_is_not_registered_is_not_found() {
    let router = emulator();
    let registered = "arn:aws:s3:::present-bucket/data/";
    let (status, payload) = call(
        &router,
        "RegisterResource",
        &json!({
            "ResourceArn": registered,
            "RoleArn": "arn:aws:iam::123456789012:role/my-data-access-role",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the registration to succeed, got {status} {payload:?}"
    );

    // Rows differ from the registered ARN along exactly one axis each: the bucket, the
    // prefix, the trailing slash, the case, and the service.
    let cases: Vec<(&str, &str)> = vec![
        ("another bucket", "arn:aws:s3:::absent-bucket/data/"),
        ("another prefix", "arn:aws:s3:::present-bucket/other/"),
        ("the bucket root", "arn:aws:s3:::present-bucket"),
        ("no trailing slash", "arn:aws:s3:::present-bucket/data"),
        ("a different case", "arn:aws:s3:::PRESENT-BUCKET/data/"),
        (
            "another service",
            "arn:aws:glue:us-east-1:123456789012:database/present",
        ),
    ];

    for (case, arn) in &cases {
        let (status, payload) = describe(&router, arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {payload:?}"
        );
        assert_eq!(
            error_code(&payload),
            "EntityNotFoundException",
            "case {case:?}: expected EntityNotFoundException, got {payload:?}"
        );
    }
}

#[tokio::test]
async fn describe_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let arn = "arn:aws:s3:::stable-bucket/data/";
    let (status, payload) = call(
        &router,
        "RegisterResource",
        &json!({
            "ResourceArn": arn,
            "UseServiceLinkedRole": true,
            "HybridAccessEnabled": true,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the registration to succeed, got {status} {payload:?}"
    );

    let (_, first) = describe(&router, arn).await;
    for read in 2..=4 {
        let (status, again) = describe(&router, arn).await;
        assert_eq!(
            status, 200,
            "read {read}: expected DescribeResource 200, got {status} {again:?}"
        );
        assert_eq!(
            again, first,
            "read {read}: a report must not change state — expected {first:?}, got {again:?}"
        );
    }
}
