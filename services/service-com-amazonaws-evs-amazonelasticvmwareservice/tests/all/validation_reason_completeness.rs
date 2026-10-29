// SPDX-License-Identifier: CC0-1.0

//! Every `ValidationException` this service answers carries the members the model
//! declares for it, whichever layer refused the request.
//!
//! The rule under test: `com.amazonaws.evs#ValidationException` marks `message` AND
//! `reason` (a `ValidationExceptionReason`) `@required`
//! (`smithy_query(query="shape", shape="ValidationException")`), so every 400 this
//! service words as that shape states both — a refusal the FRAMEWORK raised against the
//! model's own constraints exactly as one a handler built. A refusal that names input
//! members carries the reason for a field check that failed and one `fieldList` entry per
//! refused member path; a request body that could not be read as the operation's input at
//! all reached no member, so it carries the reason for that and lists no field.
//!
//! The rows span what the rule generalises over: the constraint KIND (pattern, length,
//! range, enum value set, an absent required member), the member and the OPERATION
//! carrying it, the NESTING (a top-level member, a member inside a list element, a list
//! element itself), the NUMBER of simultaneous violations, and the LAYER that refused the
//! request (the boundary validator, and a handler's own precondition on the same member of
//! the same operation). The `reason` every row expects is checked against the modelled
//! enum's own value list, which the generated types carry.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_evs_amazonelasticvmwareservice::ops::types::VALIDATION_EXCEPTION_REASON_VALUES;
use service_com_amazonaws_evs_amazonelasticvmwareservice::state::{
    REASON_CANNOT_PARSE, REASON_FIELD_VALIDATION_FAILED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::environment_lifecycle::{call, emulator, error_type, Emulator, REGION};

/// One refused request and the member paths its `fieldList` must name.
struct Row {
    /// What this row is a case of.
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The member paths the refusal is about, as the clause names them.
    members: &'static [&'static str],
}

/// Call one operation with a body this test writes BYTE for byte, so a body that is not
/// the operation's input — or not JSON at all — can be sent.
async fn call_raw(router: &MultiServiceRouter, operation: &str, body: &[u8]) -> (u16, Value) {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("https://evs.us-west-2.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("AmazonElasticVMwareService.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/evs/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0"
            ),
        )
        .body(::bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Ten non-overlapping VLAN subnet blocks, one per `InitialVlans` member.
fn vlans() -> Value {
    crate::environment_lifecycle::vlans_with_prefix(28)
}

/// A `CreateEnvironment` body every modelled constraint accepts, naming the network
/// `emulator` holds, with `overrides` merged in; a `Value::Null` override REMOVES the
/// member, which is how an absent required member is spelled.
fn create_request_with(evs: &Emulator, overrides: Value) -> Value {
    let (vpc_id, subnet_id) = evs.fresh_network();
    let mut body = json!({
        "vpcId": vpc_id,
        "serviceAccessSubnetId": subnet_id,
        "vcfVersion": "VCF-5.2.1",
        "termsAccepted": true,
        "initialVlans": vlans(),
    });
    for (member, value) in overrides.as_object().expect("the overrides are an object") {
        if value.is_null() {
            body.as_object_mut()
                .expect("the request is an object")
                .remove(member);
        } else {
            body[member] = value.clone();
        }
    }
    body
}

/// Assert the shared half of the rule: HTTP 400, a `ValidationException`, and a `reason`
/// the model declares.
fn assert_reasoned_refusal(case: &str, status: u16, body: &Value, expected_reason: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} ({}) {body}",
        error_type(body)
    );
    assert!(
        error_type(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {} {body}",
        error_type(body)
    );
    let reason = body["reason"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: a ValidationException must state its reason, got {body}")
    });
    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
        "case {case:?}: {reason:?} is not a ValidationExceptionReason the model declares \
         ({VALIDATION_EXCEPTION_REASON_VALUES:?})"
    );
    assert_eq!(
        reason, expected_reason,
        "case {case:?}: expected reason {expected_reason:?}, got {reason:?} in {body}"
    );
    assert!(
        !body["message"].as_str().unwrap_or_default().is_empty(),
        "case {case:?}: a ValidationException states a message too, got {body}"
    );
}

/// The member paths a body's `fieldList` names.
fn listed_members(case: &str, body: &Value) -> Vec<String> {
    body["fieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no fieldList in {body}"))
        .iter()
        .map(|entry| {
            entry["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a fieldList entry with no name: {entry}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn a_boundary_refusal_states_the_reason_and_the_fields_it_refused() {
    let evs = emulator();

    let rows = vec![
        Row {
            case: "a pattern violation on a top-level member",
            operation: "CreateEnvironment",
            body: create_request_with(&evs, json!({ "vpcId": "vpc-zzzzzzzz" })),
            members: &["vpcId"],
        },
        Row {
            case: "the same member absent altogether",
            operation: "CreateEnvironment",
            body: create_request_with(&evs, json!({ "vpcId": null })),
            members: &["vpcId"],
        },
        Row {
            case: "a length violation one past the ceiling",
            operation: "CreateEnvironment",
            body: create_request_with(&evs, json!({ "environmentName": "n".repeat(101) })),
            members: &["environmentName"],
        },
        Row {
            case: "a value outside an enumerated set",
            operation: "CreateEnvironment",
            body: create_request_with(&evs, json!({ "vcfVersion": "VCF-9.9.9" })),
            members: &["vcfVersion"],
        },
        Row {
            case: "two simultaneous violations",
            operation: "CreateEnvironment",
            body: create_request_with(
                &evs,
                json!({
                    "vpcId": "vpc-zzzzzzzz",
                    "serviceAccessSubnetId": "subnet-zzzzzzzz",
                }),
            ),
            members: &["serviceAccessSubnetId", "vpcId"],
        },
        Row {
            case: "three simultaneous violations",
            operation: "CreateEnvironment",
            body: create_request_with(
                &evs,
                json!({
                    "vpcId": "vpc-zzzzzzzz",
                    "serviceAccessSubnetId": "subnet-zzzzzzzz",
                    "environmentName": "bad name!",
                }),
            ),
            members: &["environmentName", "serviceAccessSubnetId", "vpcId"],
        },
        Row {
            case: "a range violation one past the ceiling, on a list operation",
            operation: "ListEnvironments",
            body: json!({ "maxResults": 101 }),
            members: &["maxResults"],
        },
        Row {
            case: "a range violation one below the floor",
            operation: "ListEnvironments",
            body: json!({ "maxResults": 0 }),
            members: &["maxResults"],
        },
        Row {
            case: "an element of a list of enums, reported against the list member",
            operation: "ListEnvironments",
            body: json!({ "state": ["NOT_A_REAL_STATE"] }),
            members: &["state"],
        },
        Row {
            case: "a member inside a list element keeps its whole path",
            operation: "PutAccountSettings",
            body: json!({
                "settings": [{ "name": "vcfPortedCoreCount", "value": "not a number" }],
            }),
            members: &["settings.1.member.value"],
        },
        Row {
            case: "a pattern violation on another operation's own member",
            operation: "ListEnvironmentHosts",
            body: json!({ "environmentId": "not-an-environment-id" }),
            members: &["environmentId"],
        },
        Row {
            case: "an absent required member on a read operation",
            operation: "GetEnvironment",
            body: json!({}),
            members: &["environmentId"],
        },
    ];

    for row in rows {
        let Row {
            case,
            operation,
            body,
            members,
        } = row;
        let (status, answer) = call(&evs, operation, &body).await;
        assert_reasoned_refusal(case, status, &answer, REASON_FIELD_VALIDATION_FAILED);

        let listed = listed_members(case, &answer);
        let named: BTreeSet<&str> = listed.iter().map(String::as_str).collect();
        let expected: BTreeSet<&str> = members.iter().copied().collect();
        assert_eq!(
            named, expected,
            "case {case:?}: expected one fieldList entry per refused member ({expected:?}), \
             got {named:?} in {answer}"
        );
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {case:?}: expected {} fieldList entries, got {} in {answer}",
            expected.len(),
            listed.len()
        );

        // Every entry's sentence is one clause of the top-level message, so a client that
        // reads `fieldList` and one that reads `message` learn the same failures.
        let message = answer["message"].as_str().unwrap_or_default().to_string();
        for entry in answer["fieldList"].as_array().expect("a fieldList") {
            let text = entry["message"].as_str().unwrap_or_default();
            assert!(
                !text.is_empty() && message.contains(text),
                "case {case:?}: the entry {entry} must state a clause of {message:?}"
            );
            let name = entry["name"].as_str().unwrap_or_default();
            assert!(
                text.contains(name),
                "case {case:?}: the entry for {name:?} must name the member it refuses, \
                 got {text:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_body_that_cannot_be_read_as_the_input_says_so() {
    let evs = emulator();

    // No member of the input was reached, so the refusal names none: what failed is the
    // reading of the body itself.
    let rows: [(&str, &str, &[u8]); 5] = [
        ("not JSON at all", "ListEnvironments", b"this is not json"),
        ("a truncated object", "ListEnvironments", b"{\"maxResults\":"),
        (
            "a string where the model declares an integer",
            "ListEnvironments",
            b"{\"maxResults\":\"ten\"}",
        ),
        (
            "a list where the model declares a structure",
            "CreateEnvironment",
            b"[\"vpc-0123456789abcdef0\"]",
        ),
        (
            "a structure where the model declares a list",
            "PutAccountSettings",
            b"{\"settings\":{\"name\":\"vcfPortedCoreCount\"}}",
        ),
    ];

    for (case, operation, body) in rows {
        let (status, answer) = call_raw(&evs, operation, body).await;
        assert_reasoned_refusal(case, status, &answer, REASON_CANNOT_PARSE);
        assert!(
            answer.get("fieldList").is_none(),
            "case {case:?}: no member was reached, so none may be listed, got {answer}"
        );
    }
}

#[tokio::test]
async fn both_layers_refuse_one_member_with_one_shape() {
    let evs = emulator();

    // The same member of the same operation, refused by the boundary validator (for the
    // shape of the value) and by the handler (for what the account's own stores say about
    // it), plus a handler precondition on another operation's member. Every one of them
    // answers the same shape: the reason for a failed field check, and a `fieldList`
    // naming the member.
    let (_vpc_id, subnet_id) = evs.fresh_network();
    let rows = [
        (
            "vpcId refused at the boundary for its pattern",
            "CreateEnvironment",
            create_request_with(&evs, json!({ "vpcId": "vpc-zzzzzzzz" })),
            "vpcId",
        ),
        (
            "vpcId refused by the handler for naming no VPC this account holds",
            "CreateEnvironment",
            json!({
                "vpcId": "vpc-0fedcba987654321f",
                "serviceAccessSubnetId": subnet_id,
                "vcfVersion": "VCF-5.2.1",
                "termsAccepted": true,
                "initialVlans": vlans(),
            }),
            "vpcId",
        ),
        (
            "nextToken refused by the handler for naming no page this service issued",
            "ListEnvironments",
            json!({ "nextToken": "abcdef" }),
            "nextToken",
        ),
    ];

    let mut shapes: BTreeSet<String> = BTreeSet::new();
    for (case, operation, body, member) in rows {
        let (status, answer) = call(&evs, operation, &body).await;
        assert_reasoned_refusal(case, status, &answer, REASON_FIELD_VALIDATION_FAILED);
        let listed = listed_members(case, &answer);
        assert!(
            listed.iter().any(|name| name == member),
            "case {case:?}: expected a fieldList entry naming {member:?}, got {listed:?}"
        );
        // The MEMBERS the body carries — not their values — are what both layers must
        // agree on.
        let mut members: Vec<&str> = answer
            .as_object()
            .expect("an error body is an object")
            .keys()
            .map(String::as_str)
            .collect();
        members.sort_unstable();
        shapes.insert(members.join(","));
    }
    assert_eq!(
        shapes.len(),
        1,
        "two refusals of one member must carry the same members, got {shapes:?}"
    );
}

#[tokio::test]
async fn nothing_else_grows_a_reason() {
    let evs = emulator();

    // A success and an error of ANOTHER shape are both left exactly as their renderer
    // wrote them: `reason` belongs to `ValidationException` and to nothing else.
    let (status, answer) = call(&evs, "ListEnvironments", &json!({})).await;
    assert_eq!(status, 200, "a list of no environments is served: {answer}");
    assert!(
        answer.get("reason").is_none(),
        "a success carries no reason, got {answer}"
    );

    let (status, answer) = call(
        &evs,
        "GetEnvironment",
        &json!({ "environmentId": "env-0123456789" }),
    )
    .await;
    // Every client error of this service reaches the wire on 400, the body's `__type`
    // telling one from another (`tests/all/error_wire_status.rs` owns that rule).
    assert_eq!(
        status, 400,
        "an environment this service holds no record of is refused: {answer}"
    );
    assert!(
        error_type(&answer).contains("ResourceNotFoundException"),
        "expected a ResourceNotFoundException, got {} {answer}",
        error_type(&answer)
    );
    assert!(
        answer.get("reason").is_none(),
        "another error shape declares no reason, got {answer}"
    );
}
