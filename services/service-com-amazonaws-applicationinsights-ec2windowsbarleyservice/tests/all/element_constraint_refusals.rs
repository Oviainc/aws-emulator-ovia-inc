// SPDX-License-Identifier: CC0-1.0

//! How this endpoint reports a constraint its model declares on the ELEMENTS of a
//! collection member.
//!
//! RULE: such a constraint is reported against the collection MEMBER — the member path
//! without an element position and without the `member` step, with no rejected value
//! quoted and with every requirement the element shape declares nested inside
//! `Member must satisfy constraint: [ … ]` — so a caller is told what every element of
//! that member has to satisfy rather than which one element offended.
//!
//! Each table row names itself and carries the whole expected sentence, because the
//! sentence is what a client parses and asserts on. Every other clause the boundary
//! validator produces — one about a scalar member, one about the collection's own
//! `@length` — keeps its own path and its own value half, which the last two tests pin
//! so the rewording cannot leak past the elements it is about.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// An EC2 instance ARN: a `ResourceList` element that satisfies every constraint
/// `ResourceARN` declares.
const INSTANCE_ARN: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-0123456789abcdef0";

/// The requirement set `ResourceList`'s elements have to satisfy, as the clause states
/// it: `ResourceARN` declares `@length` 1..1011 and this `@pattern`
/// (`smithy_query(query="shape",shape="ResourceList")`).
const RESOURCE_ARN_SET: &str = "Member must satisfy constraint: [\
     Member must have length less than or equal to 1011, \
     Member must have length greater than or equal to 1, \
     Member must satisfy regular expression pattern: \
     ^arn:aws(-\\w+)*:[\\w\\d-]+:([\\w\\d-]*)?:[\\w\\d_-]*([:/].+)*$]";

/// The requirement set `TagKeyList`'s elements have to satisfy: `TagKey` declares
/// `@length` 1..128 and this `@pattern`
/// (`smithy_query(query="shape",shape="TagKeyList")`).
const TAG_KEY_SET: &str = "Member must satisfy constraint: [\
     Member must have length less than or equal to 128, \
     Member must have length greater than or equal to 1, \
     Member must satisfy regular expression pattern: \
     ^([\\p{L}\\p{Z}\\p{N}_.:/=+\\-@]*)$]";

/// A router serving this service alone over a fresh engine, with request
/// authentication off so a suite signs nothing.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Call one operation and return its HTTP status plus its parsed JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("EC2WindowsBarleyService.{operation}"),
        )
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/applicationinsights/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=not-verified",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(&body).expect("the request body serializes"),
        ))
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or_else(|e| {
            panic!(
                "{operation} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(&payload)
            )
        })
    };
    (status, parsed)
}

/// The `__type` an awsJson error body names, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The message an error body carries, whichever casing the envelope used — in the same
/// order the suite beside this one reads it, so the two cannot disagree about a body.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Assert one constraint refusal whole: its status, its wire error code and its
/// sentence.
fn assert_refused(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got body {body:?}"
    );
    assert_eq!(
        error_message(body),
        expected,
        "case {case:?}: expected the sentence {expected:?}, got {:?}",
        error_message(body)
    );
}

/// A constraint broken by a collection's elements is reported at the collection member,
/// with the element shape's whole declared constraint set and no rejected value.
///
/// The rows span what the rule generalises over: which constraint the element broke
/// (`@pattern`, the `@length` minimum, the `@length` maximum), where the offending
/// element sits in the list, how many elements offend, how long the list is, and which
/// operation and which collection member it belongs to — `ResourceList` on two
/// operations, and `TagKeys`, whose element shape declares a different set.
#[tokio::test]
async fn an_element_constraint_is_reported_at_its_collection_member() {
    let router = router();

    // 1012 characters: one past `ResourceARN`'s declared maximum, and otherwise an ARN
    // the `@pattern` accepts, so what this row breaks is the length bound.
    let too_long = format!("{INSTANCE_ARN}{}", "a".repeat(1012 - INSTANCE_ARN.len()));
    assert_eq!(too_long.len(), 1012, "the over-long ARN is one past the bound");

    // (case, operation, the request to send, the member the clause is stated at, the
    // element shape's requirement set)
    let cases: &[(&str, &str, Value, &str, &str)] = &[
        (
            "one element that is not an ARN",
            "CreateComponent",
            json!({
                "ResourceGroupName": "rg",
                "ComponentName": "web-tier",
                "ResourceList": ["not-an-arn"],
            }),
            "resourceList",
            RESOURCE_ARN_SET,
        ),
        (
            "a valid ARN first, a bad element second",
            "CreateComponent",
            json!({
                "ResourceGroupName": "rg",
                "ComponentName": "web-tier",
                "ResourceList": [INSTANCE_ARN, "not-an-arn"],
            }),
            "resourceList",
            RESOURCE_ARN_SET,
        ),
        (
            "three elements, two of them bad",
            "CreateComponent",
            json!({
                "ResourceGroupName": "rg",
                "ComponentName": "web-tier",
                "ResourceList": ["not-an-arn", INSTANCE_ARN, "nor-this-one"],
            }),
            "resourceList",
            RESOURCE_ARN_SET,
        ),
        (
            "an empty element: the length minimum",
            "CreateComponent",
            json!({
                "ResourceGroupName": "rg",
                "ComponentName": "web-tier",
                "ResourceList": [""],
            }),
            "resourceList",
            RESOURCE_ARN_SET,
        ),
        (
            "an element one character past the length maximum",
            "CreateComponent",
            json!({
                "ResourceGroupName": "rg",
                "ComponentName": "web-tier",
                "ResourceList": [too_long.as_str()],
            }),
            "resourceList",
            RESOURCE_ARN_SET,
        ),
        (
            "the same collection on another operation",
            "UpdateComponent",
            json!({
                "ResourceGroupName": "rg",
                "ComponentName": "web-tier",
                "ResourceList": ["not-an-arn"],
            }),
            "resourceList",
            RESOURCE_ARN_SET,
        ),
        (
            "a collection whose element shape declares another set",
            "UntagResource",
            json!({ "ResourceARN": INSTANCE_ARN, "TagKeys": [""] }),
            "tagKeys",
            TAG_KEY_SET,
        ),
        (
            "that collection with a bad key after a good one",
            "UntagResource",
            json!({
                "ResourceARN": INSTANCE_ARN,
                "TagKeys": ["owner", "x".repeat(129)],
            }),
            "tagKeys",
            TAG_KEY_SET,
        ),
    ];

    for (case, operation, request, member, set) in cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        let expected = format!(
            "1 validation error detected: Value at '{member}' failed to satisfy constraint: {set}"
        );
        assert_refused(case, status, &body, &expected);
    }
}

/// A clause about a SCALAR member is the framework's own: it quotes the value the
/// request sent and names that member's path, and only the constraint the value broke.
///
/// The rows are the two scalar members of the operation the elements rule is reported
/// on, so one request cannot be rewritten by the other rule by accident.
#[tokio::test]
async fn a_scalar_members_clause_keeps_its_value_and_its_own_path() {
    let router = router();

    // (case, the request, the clause the refusal states)
    let cases: &[(&str, Value, &str)] = &[
        (
            "a component name the pattern rejects",
            json!({
                "ResourceGroupName": "rg",
                "ComponentName": "web tier!",
                "ResourceList": [INSTANCE_ARN],
            }),
            "Value 'web tier!' at 'componentName' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: ^[\\d\\w\\-_\\.+]*$",
        ),
        (
            "a resource group name the pattern rejects",
            json!({
                "ResourceGroupName": "rg/slash",
                "ComponentName": "web-tier",
                "ResourceList": [INSTANCE_ARN],
            }),
            "Value 'rg/slash' at 'resourceGroupName' failed to satisfy constraint: \
             Member must satisfy regular expression pattern: ^[a-zA-Z0-9\\.\\-_]*$",
        ),
        (
            "a component name below the length minimum",
            json!({
                "ResourceGroupName": "rg",
                "ComponentName": "",
                "ResourceList": [INSTANCE_ARN],
            }),
            "Value '' at 'componentName' failed to satisfy constraint: \
             Member must have length greater than or equal to 1",
        ),
    ];

    for (case, request, clause) in cases {
        let (status, body) = call(&router, "CreateComponent", request.clone()).await;
        let expected = format!("1 validation error detected: {clause}");
        assert_refused(case, status, &body, &expected);
    }
}

/// One request breaking a scalar member's constraint AND a collection element's earns
/// both clauses in one envelope: the elements clause once however many elements
/// offended, the scalar clause with its own value, and the count the envelope states is
/// the number of clauses.
#[tokio::test]
async fn a_scalar_clause_and_an_element_clause_share_one_envelope() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateComponent",
        json!({
            "ResourceGroupName": "rg",
            "ComponentName": "web tier!",
            "ResourceList": ["not-an-arn", "nor-this-one"],
        }),
    )
    .await;

    assert_eq!(
        status, 400,
        "a request breaking two constraints is refused, got {body:?}"
    );
    let message = error_message(&body);
    let (count, clauses) = message
        .split_once(" validation errors detected: ")
        .unwrap_or_else(|| panic!("expected the plural envelope, got {message:?}"));
    assert_eq!(count, "2", "expected two clauses, got {message:?}");

    let element_clause =
        format!("Value at 'resourceList' failed to satisfy constraint: {RESOURCE_ARN_SET}");
    let mut stated: Vec<&str> = clauses.split("; ").collect();
    stated.sort_unstable();
    let mut expected = vec![
        "Value 'web tier!' at 'componentName' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: ^[\\d\\w\\-_\\.+]*$",
        element_clause.as_str(),
    ];
    expected.sort_unstable();
    assert_eq!(
        stated, expected,
        "expected both clauses once each, got {message:?}"
    );
}

/// A constraint on the COLLECTION ITSELF is not an element's: it keeps its own clause,
/// and a request whose elements all satisfy their shape earns no constraint refusal at
/// all.
#[tokio::test]
async fn a_collections_own_constraint_and_a_valid_list_are_left_alone() {
    let router = router();

    // The elements are all valid, so nothing about the list is refused: the request
    // reaches the handler, which refuses it for the application it names instead.
    let (status, body) = call(
        &router,
        "CreateComponent",
        json!({
            "ResourceGroupName": "rg",
            "ComponentName": "web-tier",
            "ResourceList": [INSTANCE_ARN],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a valid list reaches the handler, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "a valid list is not a constraint refusal, got {body:?}"
    );

    // `TagKeyList` carries its own `@length` 0..200
    // (`smithy_query(query="shape",shape="TagKeyList")`), so 201 valid keys break the
    // COLLECTION's bound and none of the elements' — one clause, at the same member,
    // stating the collection's own requirement.
    let keys: Vec<String> = (0..201).map(|index| format!("key-{index}")).collect();
    let (status, body) = call(
        &router,
        "UntagResource",
        json!({ "ResourceARN": INSTANCE_ARN, "TagKeys": keys }),
    )
    .await;
    assert_refused(
        "201 valid tag keys",
        status,
        &body,
        "1 validation error detected: Value at 'tagKeys' failed to satisfy constraint: \
         Member must have length less than or equal to 200",
    );
}
