// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the refusals this service WORDS ITSELF — the rules no
//! modelled constraint expresses, so no clause grammar applies to them.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is the message a signed AWS SDK request observes.
//!
//! The rule under test: a refusal a handler raises for a rule the model does not state
//! is worded as the service words that error class, and NOT through the
//! modelled-constraint envelope (`N validation error(s) detected: Value at '<path>'
//! failed to satisfy constraint: …`), which states which member violated which modelled
//! constraint. Three error classes, each asserted over a SET of inputs:
//!
//! 1. a resource policy document that is not a policy document in JSON format — one
//!    fixed sentence, which draws nothing from the request;
//! 2. the per-account/Region investigation-group quota — one fixed sentence, which
//!    draws nothing from the request and nothing from the cap the emulator enforces;
//! 3. a cross-account `sourceRoleArn` that does not name an account other than the
//!    investigation account — fixed text around two variable parts: the account the
//!    investigation group belongs to, and the ARN the request sent, spelled as it sent
//!    it.
//!
//! The constraint envelope is the control: the same operations still word a MODELLED
//! violation through it, so what is asserted here is the difference between the two and
//! not merely "some message".

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`): the account an
/// investigation group belongs to, which a cross-account source role may NOT name.
const ACCOUNT: &str = "000000000000";

/// An account OTHER than the investigation account, which a source role may name.
const FOREIGN_ACCOUNT: &str = "111122223333";

/// A role ARN that satisfies the modelled `RoleArn`
/// (`smithy_query(query="shape",shape="RoleArn")`: `@length` 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The modelled error shape a refused write reports
/// (`smithy_query(query="operation",operation="PutInvestigationGroupPolicy")`:
/// `ValidationException`, `@httpError 400`).
const VALIDATION: &str = "ValidationException";

/// The modelled error shape a create past the quota reports
/// (`smithy_query(query="operation",operation="CreateInvestigationGroup")`:
/// `ServiceQuotaExceededException`, `@httpError 402`).
const QUOTA_EXCEEDED: &str = "ServiceQuotaExceededException";

/// The sentence a document that is not a policy document in JSON format is refused
/// with.
const INVALID_POLICY_DOCUMENT: &str = "The specified resource policy document is invalid.";

/// The sentence a create past the per-account/Region investigation-group quota is
/// refused with.
const CANNOT_CREATE_ANOTHER_GROUP: &str = "Cannot create another investigation group.";

/// How the modelled-constraint envelope opens: the grammar none of the three sentences
/// above may be worded through.
const CONSTRAINT_ENVELOPE: &str = "validation error";

/// The sentence a source role that does not name an account other than the
/// investigation account is refused with: fixed text around the investigation account
/// and the ARN the request sent.
fn source_role_sentence(investigation_account: &str, source_role_arn: &str) -> String {
    format!(
        "Source role ARN cannot belong to the investigation account {investigation_account}. \
         Found ARN: {source_role_arn}"
    )
}

/// Real AWS Regions the rows sign for, in order; past the list the names are synthetic
/// ones inside what a SigV4 credential scope admits (`[a-zA-Z0-9-]`).
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "us-east-2",
    "eu-central-1",
    "eu-west-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-northeast-1",
    "sa-east-1",
    "ca-central-1",
];

/// The Region slot `n` signs for.
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
/// handlers read the Region from.
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

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_in(
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
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// Percent-encode a value for use as a single `@httpLabel` path segment, which is what
/// an SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
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

fn message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

async fn create(router: &MultiServiceRouter, region: &str, body: &Value) -> (u16, Value) {
    call_in(router, region, "POST", "/investigationGroups", Some(body)).await
}

async fn update(router: &MultiServiceRouter, region: &str, arn: &str, body: &Value) -> (u16, Value) {
    call_in(
        router,
        region,
        "PATCH",
        &format!("/investigationGroups/{}", label(arn)),
        Some(body),
    )
    .await
}

async fn put_policy(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    policy: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "POST",
        &format!("/investigationGroups/{}/policy", label(arn)),
        Some(&json!({ "policy": policy })),
    )
    .await
}

/// Create a group that must succeed in `region`, and return its ARN.
async fn create_ok(router: &MultiServiceRouter, region: &str, body: &Value) -> String {
    let (status, created) = create(router, region, body).await;
    assert_eq!(status, 201, "create {body}: expected 201, got {created:?}");
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {body}: response carried no arn, got {created:?}"))
        .to_string()
}

/// A create body naming nothing but the two members a create needs.
fn minimal(name: &str) -> Value {
    json!({ "name": name, "roleArn": ROLE_ARN })
}

/// Assert one refusal: the status, the modelled error shape, the whole message, and
/// that the message is not worded through the modelled-constraint envelope.
fn assert_sentence(case: &str, expected: &str, status: u16, want_status: u16, body: &Value) {
    assert_eq!(
        status, want_status,
        "case {case:?}: expected {want_status}, got {status} / {body:?}"
    );
    let reported = message(body);
    assert_eq!(
        reported, expected,
        "case {case:?}: expected {expected:?}, got {reported:?}"
    );
    assert!(
        !reported.contains(CONSTRAINT_ENVELOPE),
        "case {case:?}: a rule the model does not state must not be worded through the \
         constraint envelope, got {reported:?}"
    );
}

/// A document that is not a policy document in JSON format is refused with ONE fixed
/// sentence, whatever the document was: the sentence draws nothing from the request, so
/// every row must read identically.
///
/// Rows differ along the FORM the document arrives in: text that is no JSON at all, a
/// truncated object, each JSON type that is not an object, an object with trailing
/// bytes, and documents of very different lengths — all inside the member's modelled
/// `@length 1..32768` and `^[\u0009\u000A\u000D\u0020-\u00FF]+$`
/// (`smithy_query(query="shape",shape="InvestigationGroupPolicyDocument")`), so the
/// boundary walk admits each one and the handler is what refuses it.
#[tokio::test]
async fn an_invalid_policy_document_is_refused_with_one_fixed_sentence() {
    let router = emulator();
    let home = region(0);
    let arn = create_ok(&router, &home, &minimal("policy-wording")).await;

    let documents: Vec<(&str, String)> = vec![
        ("text that is not JSON", "{not-valid-json:::".to_string()),
        ("a single character", "x".to_string()),
        ("a truncated object", r#"{"Version":"2012-10-17""#.to_string()),
        ("a JSON array", "[]".to_string()),
        ("a JSON string", r#""a policy""#.to_string()),
        ("a JSON number", "17".to_string()),
        ("a JSON boolean", "true".to_string()),
        ("a JSON null", "null".to_string()),
        (
            "an object with trailing bytes",
            r#"{"Version":"2012-10-17"} oops"#.to_string(),
        ),
        ("a long run of text", "z".repeat(4096)),
    ];

    for (case, document) in &documents {
        let (status, body) = put_policy(&router, &home, &arn, document).await;
        assert_eq!(
            error_code(&body),
            VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {body:?}"
        );
        assert_sentence(case, INVALID_POLICY_DOCUMENT, status, 400, &body);
        assert!(
            !message(&body).contains(document.as_str()),
            "case {case:?}: the sentence draws nothing from the request, got {:?}",
            message(&body)
        );
    }

    // The control: a MODELLED violation of the same member on the same operation is
    // still worded through the constraint envelope, so the two classes stay apart.
    let (status, body) = call_in(
        &router,
        &home,
        "POST",
        &format!("/investigationGroups/{}/policy", label(&arn)),
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, VALIDATION),
        "the absent-member control must be a 400 {VALIDATION}, got {body:?}"
    );
    assert!(
        message(&body).contains(CONSTRAINT_ENVELOPE),
        "the absent-member control is a modelled violation and keeps the constraint \
         envelope, got {:?}",
        message(&body)
    );
}

/// A create refused by the per-account/Region investigation-group quota carries ONE
/// fixed sentence: it states no count, no Region, and nothing the request sent.
///
/// Rows differ along the REGION the create signs for (each Region has its own slot, and
/// the sentence is the same in all of them) and along what the refused create asked for
/// — another name, the name the Region's group already carries, and a fuller member set
/// — so nothing of the request can be reflected in the message.
#[tokio::test]
async fn a_quota_refusal_carries_one_fixed_sentence() {
    let router = emulator();

    for slot in 0..4 {
        let home = region(slot);
        let taken = format!("quota-wording-{slot}");
        create_ok(&router, &home, &minimal(&taken)).await;

        let refused: Vec<(&str, Value)> = vec![
            ("another name", minimal("second-group")),
            ("the name the Region's group carries", minimal(&taken)),
            (
                "a fuller member set",
                json!({
                    "name": format!("third-group-{slot}"),
                    "roleArn": ROLE_ARN,
                    "retentionInDays": 30,
                    "isCloudTrailEventHistoryEnabled": false,
                    "tags": { "Team": "ops" },
                    "crossAccountConfigurations": [
                        { "sourceRoleArn": format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/Reader") }
                    ],
                }),
            ),
        ];

        for (why, body) in &refused {
            let case = format!("{home}: a second create with {why}");
            let (status, answer) = create(&router, &home, body).await;
            assert_eq!(
                error_code(&answer),
                QUOTA_EXCEEDED,
                "case {case:?}: expected {QUOTA_EXCEEDED}, got {answer:?}"
            );
            assert_sentence(&case, CANNOT_CREATE_ANOTHER_GROUP, status, 402, &answer);
            let name = body
                .get("name")
                .and_then(Value::as_str)
                .expect("every row names the group it tried to create");
            assert!(
                !message(&answer).contains(name) && !message(&answer).contains(home.as_str()),
                "case {case:?}: the sentence draws nothing from the request, got {:?}",
                message(&answer)
            );
        }
    }
}

/// A `sourceRoleArn` that does not name an account other than the investigation account
/// is refused with the service's sentence for that error class: fixed text around the
/// investigation account and the ARN the request sent, echoed as sent.
///
/// Rows differ along the FORM the refused value arrives in — the investigation account
/// behind several resource shapes, and values from which no account id can be read at
/// all — along the SIZE of the list and the POSITION of the offending entry in it, and
/// along the OPERATION that stores the list (`CreateInvestigationGroup` and
/// `UpdateInvestigationGroup` share the rule, so both must word it identically).
#[tokio::test]
async fn a_same_account_source_role_refusal_echoes_the_arn_it_refused() {
    let router = emulator();
    let foreign = |n: usize| format!("arn:aws:iam::{FOREIGN_ACCOUNT}:role/Foreign{n}");
    let entry = |arn: &str| json!({ "sourceRoleArn": arn });

    // (case, the offending ARN, the whole list as sent)
    let rows: Vec<(&str, String, Value)> = vec![
        (
            "the investigation account, alone in the list",
            format!("arn:aws:iam::{ACCOUNT}:role/Own"),
            json!([entry(&format!("arn:aws:iam::{ACCOUNT}:role/Own"))]),
        ),
        (
            "the investigation account behind a resource path",
            format!("arn:aws:iam::{ACCOUNT}:role/team/ops/Own"),
            json!([entry(&format!("arn:aws:iam::{ACCOUNT}:role/team/ops/Own"))]),
        ),
        (
            "the investigation account in another partition",
            format!("arn:aws-us-gov:iam::{ACCOUNT}:role/Own"),
            json!([entry(&format!("arn:aws-us-gov:iam::{ACCOUNT}:role/Own"))]),
        ),
        (
            "a value carrying no account field at all",
            "arn:aws:iam::role/CrossAccountInvestigation".to_string(),
            json!([entry("arn:aws:iam::role/CrossAccountInvestigation")]),
        ),
        (
            "a value whose account field is empty",
            "arn:aws:iam:::role/CrossAccountInvestigation".to_string(),
            json!([entry("arn:aws:iam:::role/CrossAccountInvestigation")]),
        ),
        (
            "the offending entry last in a longer list",
            format!("arn:aws:iam::{ACCOUNT}:role/Last"),
            json!([
                entry(&foreign(1)),
                entry(&foreign(2)),
                entry(&format!("arn:aws:iam::{ACCOUNT}:role/Last"))
            ]),
        ),
        (
            "the FIRST offending entry of several",
            format!("arn:aws:iam::{ACCOUNT}:role/Earliest"),
            json!([
                entry(&foreign(1)),
                entry(&format!("arn:aws:iam::{ACCOUNT}:role/Earliest")),
                entry(&format!("arn:aws:iam::{ACCOUNT}:role/Later"))
            ]),
        ),
    ];

    // One Region per row for the create, and one Region holding a group for the update.
    let update_region = region(2 * rows.len());
    let target = create_ok(
        &router,
        &update_region,
        &json!({
            "name": "source-role-wording",
            "roleArn": ROLE_ARN,
            "crossAccountConfigurations": [entry(&foreign(9))],
        }),
    )
    .await;

    for (index, (case, offending, list)) in rows.iter().enumerate() {
        let expected = source_role_sentence(ACCOUNT, offending);
        let create_region = region(2 * index);
        let (create_status, created) = create(
            &router,
            &create_region,
            &json!({
                "name": format!("source-role-{index}"),
                "roleArn": ROLE_ARN,
                "crossAccountConfigurations": list,
            }),
        )
        .await;
        let (update_status, updated) = update(
            &router,
            &update_region,
            &target,
            &json!({ "crossAccountConfigurations": list }),
        )
        .await;

        for (operation, status, body) in [
            ("CreateInvestigationGroup", create_status, &created),
            ("UpdateInvestigationGroup", update_status, &updated),
        ] {
            let case = format!("{operation}: {case}");
            assert_eq!(
                error_code(body),
                VALIDATION,
                "case {case:?}: expected {VALIDATION}, got {body:?}"
            );
            assert_sentence(&case, &expected, status, 400, body);
            assert!(
                message(body).contains(offending.as_str()),
                "case {case:?}: the sentence echoes the ARN the request sent, got {:?}",
                message(body)
            );
        }
    }
}
