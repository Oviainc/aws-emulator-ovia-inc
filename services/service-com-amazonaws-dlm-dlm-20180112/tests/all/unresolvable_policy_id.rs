// SPDX-License-Identifier: CC0-1.0

//! What a client sees on the WIRE when it addresses a lifecycle policy by an id
//! that resolves to no policy.
//!
//! One rule, asserted over a table of id forms rather than the single id an
//! issue cited:
//!
//! > An operation that addresses one lifecycle policy by its `@httpLabel`
//! > `PolicyId` and models no 400-class client error answers EVERY id that
//! > resolves to no policy — whether the modelled constraints on the id refuse
//! > it or the store simply holds no such policy — with the modelled
//! > `ResourceNotFoundException`: HTTP 404, the `x-amzn-errortype` a restJson1
//! > client resolves the error shape from naming THAT shape, and the members the
//! > shape declares; never a status or an error type the operation does not
//! > model, and repeating the call answers the same thing.
//!
//! `DeleteLifecyclePolicy` and `GetLifecyclePolicy` are the two such operations
//! (`smithy_query(query="operation", operation="DeleteLifecyclePolicy")`:
//! `InternalServerException` 500, `LimitExceededException` 429,
//! `ResourceNotFoundException` 404 — and no 400-class error at all), and each
//! takes exactly one input member, the `@httpLabel` `PolicyId`
//! (`@pattern(^policy-[a-f0-9]+$)`, `@length(0..64)`).
//!
//! Why the repeat matters: "delete it, tolerate not-found" is how a client
//! cleans up, so the second delete of an id must answer the first delete's
//! error, and a refused id must leave the store exactly as it was.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this file is signed for.
const REGION: &str = "ap-southeast-2";

/// The only client error these two operations model for the resource they
/// address, its `@httpError` status and the `Code` the service reports for the
/// class (`smithy_query(query="shape", shape="ResourceNotFoundException")`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";
const RESOURCE_NOT_FOUND_STATUS: u16 = 404;
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFound";

/// The `ResourceType` token a lifecycle policy carries.
const POLICY_RESOURCE_TYPE: &str = "LIFECYCLE_POLICY";

/// The error type the framework's pre-dispatch envelope would name for a value
/// outside a modelled constraint. The DLM model declares no such shape, so no
/// answer from these operations may carry it.
const UNMODELLED_ERROR_TYPE: &str = "ValidationException";

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status, the `x-amzn-errortype` header
/// and the JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// The error shape this answer names, as a restJson1 client resolves it:
    /// the `x-amzn-errortype` header, falling back to the body's `__type`, with
    /// any `namespace#` prefix stripped.
    fn error_shape(&self) -> &str {
        let named = if self.error_type.is_empty() {
            self.body["__type"].as_str().unwrap_or_default()
        } else {
            self.error_type.as_str()
        };
        named.rsplit('#').next().unwrap_or_default()
    }

    /// The body's member names, sorted — what makes two refusals the same error
    /// shape to a client.
    fn members(&self) -> BTreeSet<String> {
        self.body
            .as_object()
            .map(|body| body.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The `PolicyId` `CreateLifecyclePolicy` returned.
    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A SigV4 credential scope naming this service in `REGION`; it is what routes a
/// request to dlm.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", authorization());
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!("{method} {uri} answered a body that is not JSON ({error}): {bytes:?}")
        })
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// The role every policy here names: one this account may pass.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// A well-formed `CreateLifecyclePolicy` request, as a client sends it.
fn create_request(description: &str) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
        "PolicyDetails": {
            "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
            "ResourceTypes": ["VOLUME"],
            "TargetTags": [{"Key": "backup", "Value": "daily"}],
            "Schedules": [{
                "Name": "Daily",
                "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS", "Times": ["03:00"]},
                "RetainRule": {"Count": 1},
            }],
        },
    })
}

/// Create one policy, asserting it was accepted, and answer its id.
async fn created_policy(router: &MultiServiceRouter, description: &str) -> String {
    let answer = call(
        router,
        "POST",
        "/policies",
        Some(create_request(description)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "the subject policy {description:?} must be created, got {} with {:?}",
        answer.status, answer.body
    );
    answer.policy_id()
}

/// Assert `answer` is the modelled `ResourceNotFoundException` for `named`.
///
/// `named` is the `PolicyId` the request addressed, after the URI label is
/// decoded: the id the error reports as the resource that was not found.
fn assert_policy_not_found(case: &str, named: &str, answer: &Answer) {
    assert_eq!(
        answer.status, RESOURCE_NOT_FOUND_STATUS,
        "case {case:?}: expected HTTP {RESOURCE_NOT_FOUND_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_shape(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "case {case:?}: expected the modelled {RESOURCE_NOT_FOUND_EXCEPTION}, got {:?}",
        answer.error_type
    );
    assert!(
        !answer.error_type.contains(UNMODELLED_ERROR_TYPE)
            && answer.body["__type"]
                .as_str()
                .is_none_or(|named| !named.contains(UNMODELLED_ERROR_TYPE)),
        "case {case:?}: these operations model no {UNMODELLED_ERROR_TYPE}, got {:?} with {:?}",
        answer.error_type,
        answer.body
    );
    assert_eq!(
        answer.body["Code"],
        json!(RESOURCE_NOT_FOUND_CODE),
        "case {case:?}: expected Code {RESOURCE_NOT_FOUND_CODE:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["ResourceType"],
        json!(POLICY_RESOURCE_TYPE),
        "case {case:?}: expected ResourceType {POLICY_RESOURCE_TYPE:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["ResourceIds"],
        json!([named]),
        "case {case:?}: ResourceIds must name the id that resolved to nothing, got {:?}",
        answer.body
    );
    // The modelled shape declares `Message`; the lowercase `message` key belongs
    // to the envelope this service's model does not declare.
    assert!(
        answer.body["Message"]
            .as_str()
            .is_some_and(|message| message.contains(named) && message.ends_with("was not found.")),
        "case {case:?}: the Message member must name the resource asked for, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.body["message"],
        Value::Null,
        "case {case:?}: the modelled error carries no lowercase message key, got {:?}",
        answer.body
    );
}

/// The rule, over the forms an unresolvable `PolicyId` takes, on both operations
/// that address a policy by it, called twice each.
///
/// The rows differ along WHAT makes the id resolve to nothing — which part of
/// the modelled constraints it breaks (the prefix, the separator, the hex
/// alphabet, the case, the length maximum), how it was encoded in the URI, and,
/// for the last two rows, nothing at all except that the store holds no such
/// policy. `@length(0..64)` is exercised at its maximum and one character past
/// it; the minimum, 0, is not addressable, because an empty path segment names
/// no `/policies/{PolicyId}` route at all.
#[tokio::test]
async fn an_unresolvable_policy_id_is_the_modelled_resource_not_found_exception() {
    let router = emulator();
    let subject = created_policy(&router, "unresolvable id subject").await;

    // `@length(max: 64)` counts the whole id: `policy-` is seven characters, so
    // 57 hex digits sits at the maximum and 58 is one past it.
    let at_length_maximum = format!("policy-{}", "c".repeat(57));
    let past_length_maximum = format!("policy-{}", "c".repeat(58));

    // (case, the id as the URI spells it, the id after the label is decoded)
    let cases: Vec<(&str, String, String)> = vec![
        (
            "a user-typed name",
            "backup-policy".to_string(),
            "backup-policy".to_string(),
        ),
        (
            "the prefix in its plural form",
            "policies-0a1b".to_string(),
            "policies-0a1b".to_string(),
        ),
        (
            "an underscore where the separator belongs",
            "policy_0a1b".to_string(),
            "policy_0a1b".to_string(),
        ),
        (
            "a space inside the id, percent-encoded",
            "policy-%200a1b".to_string(),
            "policy- 0a1b".to_string(),
        ),
        (
            "a non-ASCII character among the digits",
            "policy-%CE%B10a1b".to_string(),
            "policy-\u{3b1}0a1b".to_string(),
        ),
        (
            "hex digits with no prefix at all",
            "0123456789abcdef".to_string(),
            "0123456789abcdef".to_string(),
        ),
        (
            "uppercase hex digits",
            "policy-0A1B".to_string(),
            "policy-0A1B".to_string(),
        ),
        (
            "one character past the length maximum",
            past_length_maximum.clone(),
            past_length_maximum,
        ),
        // Well formed, so the handler decides — and it holds no such policy, so
        // the answer must be indistinguishable from the rows above.
        (
            "a well-formed id at the length maximum",
            at_length_maximum.clone(),
            at_length_maximum,
        ),
        (
            // The store mints ids in sequence from `policy-0000000000000001`, so
            // an id at the far end of that space is one it has not minted.
            "a well-formed id the store never minted",
            "policy-0fffffffffffffff".to_string(),
            "policy-0fffffffffffffff".to_string(),
        ),
    ];

    // The yardstick: a well-formed id the store never minted, read back. Every
    // row must answer the same error members, whatever form its id took.
    let reference = call(&router, "GET", "/policies/policy-0ffffffffffffffe", None).await;
    assert_policy_not_found("the reference id", "policy-0ffffffffffffffe", &reference);
    let reference_members = reference.members();

    for (row, uri_id, named) in &cases {
        for (operation, method) in [
            ("GetLifecyclePolicy", "GET"),
            ("DeleteLifecyclePolicy", "DELETE"),
        ] {
            let case = format!("{operation}: {row}");
            let first = call(&router, method, &format!("/policies/{uri_id}"), None).await;
            assert_policy_not_found(&case, named, &first);
            assert_eq!(
                first.members(),
                reference_members,
                "case {case:?} with {uri_id:?}: an unresolvable id must answer the same error \
                 members whatever form it took, got {:?}",
                first.body
            );
            // "Delete it, tolerate not-found" repeats the call: the second
            // answer must be byte-for-byte the first.
            let again = call(&router, method, &format!("/policies/{uri_id}"), None).await;
            assert_eq!(
                again, first,
                "case {case:?} with {uri_id:?}: repeating the call must answer the same thing, \
                 got {again:?} after {first:?}"
            );
        }
    }

    // None of those calls touched the store: the one policy created above is
    // still there, and is still the only one.
    let listed = call(&router, "GET", "/policies", None).await;
    assert_eq!(
        listed.body["Policies"].as_array().map(Vec::len),
        Some(1),
        "an unresolvable id must create or remove no policy, got {:?}",
        listed.body
    );
    let read = call(&router, "GET", &format!("/policies/{subject}"), None).await;
    assert_eq!(
        read.status, 200,
        "the stored policy must still be readable, got {} with {:?}",
        read.status, read.body
    );
}

/// An id the modelled constraints accept and the store resolves is SERVED, and
/// only once it is gone does it answer the same not-found error an id that never
/// resolved gets — the error carrying that id, with no error type the operation
/// does not model anywhere along the way.
#[tokio::test]
async fn a_resolvable_policy_id_is_served_until_it_is_deleted() {
    let router = emulator();
    let subject = created_policy(&router, "delete then re-delete").await;

    let read = call(&router, "GET", &format!("/policies/{subject}"), None).await;
    assert_eq!(
        read.status, 200,
        "the created policy must be readable, got {} with {:?}",
        read.status, read.body
    );
    assert!(
        read.error_type.is_empty(),
        "a served read carries no error type, got {:?}",
        read.error_type
    );

    let deleted = call(&router, "DELETE", &format!("/policies/{subject}"), None).await;
    assert_eq!(
        deleted.status, 200,
        "a well-formed id that names a policy must be deleted, got {} with {:?}",
        deleted.status, deleted.body
    );

    // From here the id resolves to nothing, so both operations answer the
    // modelled not-found error — including the repeated DELETE.
    for (case, method) in [
        ("the read after the delete", "GET"),
        ("the second delete", "DELETE"),
        ("the third delete", "DELETE"),
    ] {
        let answer = call(&router, method, &format!("/policies/{subject}"), None).await;
        assert_policy_not_found(case, &subject, &answer);
    }
}
