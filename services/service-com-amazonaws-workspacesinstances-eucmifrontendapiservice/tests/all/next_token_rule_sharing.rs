// SPDX-License-Identifier: CC0-1.0

//! The bad-`NextToken` refusal, driven through EVERY paginated operation of this
//! service, over the wire it really speaks (awsJson1_0, `POST /`,
//! `X-Amz-Target: EUCMIFrontendAPIService.<Operation>`).
//!
//! The rule under test, stated once and asserted over a SET of inputs:
//!
//! > a `NextToken` this service did not issue is a property of the TOKEN, not of the
//! > list it was presented to: every paginated operation of the service refuses it with
//! > the same HTTP status, the same modelled error code, the same `Reason`, the same
//! > `FieldList` naming the `NextToken` member and the same `Message`.
//!
//! The rows differ along what that rule generalises over: the operation the token is
//! presented to (all three the service publishes as `@paginated` — `ListRegions`,
//! `ListWorkspaceInstances`, `ListInstanceTypes`), the token's form (not this service's
//! encoding at all, odd-length, bytes that are not text, an encoding that decodes to a
//! resume point no list of this service can name), and its length (each published bound
//! of the modelled `NextToken` and one past the top one). The other request members are
//! varied too, so the refusal cannot depend on a page size or a filter.
//!
//! Nothing here can pass against text written in this file: the refusals are compared
//! field by field BETWEEN the operations, and each is also checked against the structure
//! this service's validation refusals have.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::workspace_instance_lifecycle::{call, error_code, router};

/// The service's `@paginated` operations, with the `items` member each one reports —
/// `OPERATIONS.md`, the `Traits:` line of each (`@paginated(inputToken=NextToken,
/// outputToken=NextToken, items=...)`).
const PAGINATED: &[(&str, &str)] = &[
    ("ListRegions", "Regions"),
    ("ListWorkspaceInstances", "WorkspaceInstances"),
    ("ListInstanceTypes", "InstanceTypes"),
];

/// The `ValidationExceptionReason` a single bad member is reported with, as the
/// modelled enum value (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
const FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// What a refusal of a token this service did not issue says, as a client reads it.
/// Compared whole, so no two lists can answer the same bad token differently in any of
/// these.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    status: u16,
    code: String,
    reason: String,
    members: Vec<String>,
    message: String,
}

/// Read a [`Refusal`] out of one answer, failing the case when the answer does not have
/// the shape this service's validation refusals have.
fn refusal(case: &str, operation: &str, status: u16, payload: &Value) -> Refusal {
    let fields = payload["FieldList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: no FieldList in {payload}"));
    let members: Vec<String> = fields
        .iter()
        .map(|field| {
            field["Name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?} on {operation}: no Name in {payload}"))
                .to_string()
        })
        .collect();
    let message = payload["Message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?} on {operation}: no Message in {payload}"))
        .to_string();
    // The one wording reaches both places a client reads it: the overall `Message` and
    // the faulted field's own.
    for field in fields {
        assert_eq!(
            field["Message"].as_str(),
            Some(message.as_str()),
            "case {case:?} on {operation}: the field's Message must be the overall one, \
             got {payload}"
        );
    }
    Refusal {
        status,
        code: error_code(payload),
        reason: payload["Reason"].as_str().unwrap_or_default().to_string(),
        members,
        message,
    }
}

/// The token forms the rule is asserted over: none of them is a token any list of this
/// service ever handed out, whatever it looks like.
fn token_rows() -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = vec![
        ("not this service's encoding at all".to_string(), "not-a-token".to_string()),
        ("odd-length hex".to_string(), "abc".to_string()),
        ("hex of bytes that are not text".to_string(), "ff".to_string()),
        (
            "an opaque base64-looking token".to_string(),
            "QUFBQUFBQUE=".to_string(),
        ),
        (
            "this service's encoding of a Region name, which no list resumes at".to_string(),
            hex::encode("us-east-1"),
        ),
        (
            "this service's encoding of a well-shaped instance id nothing holds".to_string(),
            hex::encode("wsinst-ffffffffffffffff"),
        ),
        (
            "this service's encoding of an instance type no catalog publishes".to_string(),
            hex::encode("zz99.nonesuch"),
        ),
    ];
    // Each published bound of the modelled `NextToken` (`len 1..2048`, OPERATIONS.md's
    // input line for every paginated operation here).
    rows.push((
        "one character, the modelled minimum length".to_string(),
        "a".to_string(),
    ));
    rows.push((
        "2048 characters, the modelled maximum length".to_string(),
        "a".repeat(2048),
    ));
    rows
}

/// The request bodies one token is presented in, so the refusal cannot turn on a page
/// size or a filter: `operation` takes only the members its input models.
///
/// `pub(crate)`: the sibling suite that drives the spelling half of the same
/// bad-`NextToken` rule presents its tokens in these very bodies, so the two cannot
/// drift on what a token is presented in.
pub(crate) fn requests(operation: &str, token: &str) -> Vec<Value> {
    let mut bodies = vec![
        json!({ "NextToken": token }),
        json!({ "NextToken": token, "MaxResults": 1 }),
        // `MaxResults` ceilings differ per operation (25, 25 and 600); 25 is inside
        // every one of them.
        json!({ "NextToken": token, "MaxResults": 25 }),
    ];
    if operation == "ListWorkspaceInstances" {
        bodies.push(json!({ "NextToken": token, "ProvisionStates": ["ALLOCATING"] }));
    }
    if operation == "ListInstanceTypes" {
        bodies.push(json!({
            "NextToken": token,
            "InstanceConfigurationFilter": {
                // One combination the modelled value lists admit
                // (`smithy_query(query="shape",shape="BillingMode")`,
                // `PlatformTypeEnum`, `InstanceConfigurationTenancyEnum`).
                "BillingMode": "HOURLY",
                "PlatformType": "Windows",
                "Tenancy": "SHARED",
            },
        }));
    }
    bodies
}

#[tokio::test]
async fn every_paginated_list_refuses_a_token_this_service_did_not_issue_the_same_way() {
    let router = router();

    for (case, token) in token_rows() {
        let mut answers: Vec<(&str, Refusal, Value)> = Vec::new();
        for (operation, _items) in PAGINATED {
            for body in requests(operation, &token) {
                let case = format!("{case} in {body}");
                let (status, payload) = call(&router, operation, body.clone()).await;
                let read = refusal(&case, operation, status, &payload);
                assert_eq!(
                    (read.status, read.code.as_str(), read.reason.as_str(), &read.members),
                    (
                        400,
                        "ValidationException",
                        FIELD_VALIDATION_FAILED,
                        &vec!["NextToken".to_string()]
                    ),
                    "case {case:?} on {operation}: expected the modelled validation refusal \
                     naming NextToken, got {payload}"
                );
                answers.push((operation, read, payload));
            }
        }

        // The refusal is the TOKEN's, so every list answered it identically — compared
        // against the first answer rather than against a sentence written here.
        let (first_operation, first, first_payload) = answers
            .first()
            .expect("the paginated operations are driven above");
        for (operation, read, payload) in &answers {
            assert_eq!(
                read, first,
                "case {case:?}: {operation} must refuse the token exactly as \
                 {first_operation} does, got {payload} against {first_payload}"
            );
        }
        assert!(
            first.message.contains("NextToken"),
            "case {case:?}: expected the wording to name the member it faults, got {:?}",
            first.message
        );
    }
}

#[tokio::test]
async fn a_length_past_the_modelled_maximum_is_refused_by_every_paginated_list() {
    let router = router();

    // One past the published ceiling of `NextToken` (`len 1..2048`) is refused at the
    // boundary instead of by the handler, so only the status and the code are the
    // shared part there.
    let case = "2049 characters, one past the modelled maximum length";
    let token = "a".repeat(2049);
    for (operation, _items) in PAGINATED {
        let (status, payload) = call(&router, operation, json!({ "NextToken": &token })).await;
        assert_eq!(
            status, 400,
            "case {case:?} on {operation}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case:?} on {operation}: expected ValidationException, got {payload}"
        );
    }
}

#[tokio::test]
async fn the_refusal_is_the_tokens_and_a_request_carrying_none_is_still_served() {
    // The rule is about a token the service did not issue, so it refuses nothing else:
    // each paginated list still answers a request that carries no token, before and
    // after the refusals, with the items member its model marks required.
    let router = router();

    async fn served(router: &MultiServiceRouter, case: &str, operation: &str, items: &str) {
        let (status, payload) = call(router, operation, json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?} on {operation}: expected HTTP 200 without a NextToken, got \
             {status} with {payload}"
        );
        assert!(
            payload[items].is_array(),
            "case {case:?} on {operation}: expected the {items} member, got {payload}"
        );
    }

    for (operation, items) in PAGINATED {
        served(&router, "before the refusals", operation, items).await;
    }
    for (operation, _items) in PAGINATED {
        let (status, _) = call(&router, operation, json!({ "NextToken": "not-a-token" })).await;
        assert_eq!(
            status, 400,
            "case \"the refusal itself\" on {operation}: expected HTTP 400, got {status}"
        );
    }
    for (operation, items) in PAGINATED {
        served(&router, "after the refusals", operation, items).await;
    }
}
