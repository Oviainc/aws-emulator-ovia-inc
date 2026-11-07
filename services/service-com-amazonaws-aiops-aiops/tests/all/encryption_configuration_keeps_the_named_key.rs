// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the KEY a write keeps when the request names one in
//! `encryptionConfiguration`.
//!
//! The rule under test: `encryptionConfiguration` names the key an investigation
//! group's data is encrypted with, so a write KEEPS the key the request names — the
//! supplied `kmsKeyId` is stored and read back, and a structure that names the id
//! without a `type` (neither member is `@required`, see
//! `smithy_query(query="shape",shape="EncryptionConfiguration")`) is stored as the
//! customer managed key type it asks for, because `kmsKeyId` is documented as the id of
//! the customer managed key the group uses. Only a request that names NO key at all
//! gets the Amazon Web Services owned key, which is the case
//! `CreateInvestigationGroupInput$encryptionConfiguration` documents ("If you omit this
//! parameter ...") and which the sibling suites cover.
//!
//! Both writers of the member apply the rule, so each row is driven through
//! `CreateInvestigationGroup` AND through `UpdateInvestigationGroup`, and read back
//! through `GetInvestigationGroup`.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes. An account holds one
//! investigation group per Region, so every leg signs for a Region of its own.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_aiops_aiops::ops::types::ENCRYPTION_CONFIGURATION_TYPE_VALUES;
use service_com_amazonaws_aiops_aiops::state::{AWS_OWNED_KEY_TYPE, CUSTOMER_MANAGED_KMS_KEY_TYPE};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN that satisfies the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The modelled error shape a request outside a modelled constraint is refused with
/// (`smithy_query(query="operation",operation="CreateInvestigationGroup")`:
/// `ValidationException`, `@httpError 400`).
const VALIDATION: &str = "ValidationException";

/// The modelled maximum length of `EncryptionConfiguration$kmsKeyId`
/// (`smithy_query(query="shape",shape="EncryptionConfiguration")`: len 1..256).
const MAX_KMS_KEY_ID: usize = 256;

/// Real AWS Regions the legs sign for, in order; past the list the names are synthetic
/// ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "us-east-2",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-southeast-4",
    "ap-northeast-1",
    "sa-east-1",
    "ca-central-1",
    "ap-south-1",
    "eu-north-1",
    "me-south-1",
    "af-south-1",
    "ap-east-1",
    "us-gov-west-1",
];

/// The Region slot `n` signs for; each leg of each row needs one of its own.
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:04}"), |name| (*name).to_string())
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in `region`, the signal the router routes on and the
/// handlers read the region and the caller principal from.
fn request(
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDAIOPSTEST/20260101/{region}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`; the
/// framework's `x-amzn-errortype` header is folded into `__type` so one accessor reads
/// either carrier of the error shape.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(region, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.aiops#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is what
/// an SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value.replace('/', "%2F")
}

/// Create a group that must succeed, and answer its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, body: &Value, case: &str) -> String {
    let (status, created) = call(router, region, "POST", "/investigationGroups", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateInvestigationGroup, got {status} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the create carried no arn, got {created:?}"))
        .to_string()
}

/// `PATCH` a group, asserting the update succeeded.
async fn update_ok(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    patch: &Value,
    case: &str,
) {
    let (status, answer) = call(
        router,
        region,
        "PATCH",
        &format!("/investigationGroups/{}", label(identifier)),
        Some(patch),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateInvestigationGroup, got {status} / {answer:?}"
    );
}

/// The `encryptionConfiguration` a read of the group reports.
async fn stored_configuration(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
    case: &str,
) -> Value {
    let (status, body) = call(
        router,
        region,
        "GET",
        &format!("/investigationGroups/{}", label(identifier)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetInvestigationGroup, got {status} / {body:?}"
    );
    body.get("encryptionConfiguration")
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the read reported no encryptionConfiguration, got {body:?}")
        })
}

/// One row: a `kmsKeyId` a request names, and whether it names a `type` beside it.
struct Row {
    case: &'static str,
    key: String,
    /// The `type` the request carries beside the key id; `None` carries no `type`.
    key_type: Option<&'static str>,
}

/// A `kmsKeyId` of exactly `length` characters that satisfies the modelled member
/// (len 1..256, `^arn:.*$`): a key ARN padded in its resource field.
fn key_of_length(length: usize) -> String {
    let prefix = format!("arn:aws:kms:us-west-2:{ACCOUNT}:key/");
    assert!(
        length >= prefix.len(),
        "a key id of {length} characters cannot carry the ARN prefix {prefix:?}"
    );
    format!("{prefix}{}", "a".repeat(length - prefix.len()))
}

/// The rows, differing along the FORM and LENGTH of the key id the request names, and
/// along whether the request names the customer managed `type` beside it. The key forms
/// are the ones a KMS key ARN takes — a single-Region key, a multi-Region key (`mrk-`),
/// an alias and a key in another partition — and the lengths are the modelled bounds of
/// `EncryptionConfiguration$kmsKeyId` (len 1..256,`^arn:.*$`, so the shortest value the
/// member admits is `arn:`).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a single-Region key ARN, no type",
            key: format!("arn:aws:kms:us-west-2:{ACCOUNT}:key/8f1c2d3e-4a5b-6c7d-8e9f-0a1b2c3d4e5f"),
            key_type: None,
        },
        Row {
            case: "a multi-Region key ARN, no type",
            key: format!("arn:aws:kms:eu-central-1:{ACCOUNT}:key/mrk-1234abcd12ab34cd56ef1234567890ab"),
            key_type: None,
        },
        Row {
            case: "a key alias ARN, no type",
            key: format!("arn:aws:kms:ap-south-1:{ACCOUNT}:alias/investigations"),
            key_type: None,
        },
        Row {
            case: "a key ARN in another partition, no type",
            key: format!("arn:aws-us-gov:kms:us-gov-west-1:{ACCOUNT}:key/0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9"),
            key_type: None,
        },
        Row {
            case: "the shortest key id the member admits, no type",
            key: "arn:".to_string(),
            key_type: None,
        },
        Row {
            case: "a key id at the modelled maximum length, no type",
            key: key_of_length(MAX_KMS_KEY_ID),
            key_type: None,
        },
        Row {
            case: "a single-Region key ARN with the customer managed type",
            key: format!("arn:aws:kms:us-east-1:{ACCOUNT}:key/5e4d3c2b-1a09-8765-4321-fedcba098765"),
            key_type: Some(CUSTOMER_MANAGED_KMS_KEY_TYPE),
        },
        Row {
            case: "a key id at the modelled maximum length with the customer managed type",
            key: key_of_length(MAX_KMS_KEY_ID),
            key_type: Some(CUSTOMER_MANAGED_KMS_KEY_TYPE),
        },
    ]
}

/// The configuration a row's request carries.
fn supplied(row: &Row) -> Value {
    let mut structure = serde_json::Map::new();
    if let Some(key_type) = row.key_type {
        structure.insert("type".to_string(), json!(key_type));
    }
    structure.insert("kmsKeyId".to_string(), json!(row.key));
    Value::Object(structure)
}

/// What the group must report afterwards: the key the request named, under the type it
/// named or — when it named none — the customer managed type its key id asks for.
fn expected(row: &Row) -> Value {
    json!({
        "type": row.key_type.unwrap_or(CUSTOMER_MANAGED_KMS_KEY_TYPE),
        "kmsKeyId": row.key,
    })
}

/// Whatever key a row names, the configuration the group reports names it — and names a
/// key type out of the modelled value set, and reads the same every time.
fn assert_reported(got: &Value, row: &Row, leg: &str) {
    let case = row.case;
    let want = expected(row);
    assert_eq!(
        got, &want,
        "case {case:?} ({leg}): a write carrying {} must store {want}, got {got:?}",
        supplied(row)
    );
    let key_type = got.get("type").and_then(Value::as_str);
    assert!(
        key_type.is_some_and(|value| ENCRYPTION_CONFIGURATION_TYPE_VALUES.contains(&value)),
        "case {case:?} ({leg}): the stored configuration must name a modelled key type out of \
         {ENCRYPTION_CONFIGURATION_TYPE_VALUES:?}, got {got:?}"
    );
    assert_ne!(
        key_type,
        Some(AWS_OWNED_KEY_TYPE),
        "case {case:?} ({leg}): a write that named a key must not report the Amazon Web \
         Services owned key, got {got:?}"
    );
}

/// A create that names a key stores it, and every later read of the group reports it.
#[tokio::test]
async fn a_create_keeps_the_key_the_request_names() {
    let router = emulator();
    for (index, row) in rows().into_iter().enumerate() {
        let region = region(index);
        let body = json!({
            "name": "keeps-the-key",
            "roleArn": ROLE_ARN,
            "encryptionConfiguration": supplied(&row),
        });
        let arn = create_ok(&router, &region, &body, row.case).await;

        let got = stored_configuration(&router, &region, &arn, row.case).await;
        assert_reported(&got, &row, "create");

        // Reporting it does not change it: the next read sees the same configuration.
        let again = stored_configuration(&router, &region, &arn, row.case).await;
        assert_eq!(
            again, got,
            "case {:?} (create): a second read must report the same configuration, got {again:?}",
            row.case
        );
    }
}

/// An update that names a key stores it over whatever the group carried before — here a
/// group created with the Amazon Web Services owned key, so the key id can only have
/// come from the update.
#[tokio::test]
async fn an_update_keeps_the_key_the_request_names() {
    let router = emulator();
    for (index, row) in rows().into_iter().enumerate() {
        let region = region(index + rows().len());
        let arn = create_ok(
            &router,
            &region,
            &json!({ "name": "keeps-the-key", "roleArn": ROLE_ARN,
                     "encryptionConfiguration": { "type": AWS_OWNED_KEY_TYPE } }),
            row.case,
        )
        .await;

        update_ok(
            &router,
            &region,
            &arn,
            &json!({ "encryptionConfiguration": supplied(&row) }),
            row.case,
        )
        .await;

        let got = stored_configuration(&router, &region, &arn, row.case).await;
        assert_reported(&got, &row, "update");
    }
}

/// The modelled bound itself: a key id one character past the member's maximum length is
/// refused by the constraint walk with the modelled `ValidationException`, naming the
/// member — so a key id is carried through the write because it PASSED the model's own
/// check, not because the member is unchecked.
#[tokio::test]
async fn a_key_id_past_the_modelled_maximum_is_refused() {
    let router = emulator();
    let case = "a key id one past the modelled maximum length";
    let (status, body) = call(
        &router,
        "us-west-2",
        "POST",
        "/investigationGroups",
        Some(&json!({
            "name": "keeps-the-key",
            "roleArn": ROLE_ARN,
            "encryptionConfiguration": { "kmsKeyId": key_of_length(MAX_KMS_KEY_ID + 1) },
        })),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from CreateInvestigationGroup, got {status} / {body:?}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, VALIDATION,
        "case {case:?}: expected the modelled {VALIDATION}, got {code} / {body:?}"
    );
    let message = body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>");
    assert!(
        message.contains("encryptionConfiguration.kmsKeyId"),
        "case {case:?}: the refusal must name the member it refused, got {message:?}"
    );
}
