// SPDX-License-Identifier: CC0-1.0

//! Every refusal of one exception type carries that type's ONE fixed sentence.
//!
//! AWS WAF Classic answers a refusal CLASS with a single request-independent sentence:
//! the recorded corpus shows `GetIPSet` answering the identical bytes for an id that was
//! never created and for an id whose IPSet was deleted, and the same per-type constant for
//! the non-empty-entity and stale-token refusals. So the rule these cases pin is that the
//! `message` of a modelled exception is a function of the EXCEPTION TYPE alone: the same
//! string whatever the request was, for every operation that raises it, and carrying
//! nothing taken from the request — no id, name, change token, address, count or limit.
//!
//! Each row below provokes one refusal with a DIFFERENT request: a different operation, a
//! different id (never created, deleted, well-formed but unknown), a different token, a
//! different descriptor count, a different name length, and — for the types two resource
//! families share — a different resource type. The expected sentence is spelled out here
//! as a literal rather than read from the crate, so the test pins the wire text and not
//! just the fact that some constant was used.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_wafregional_awswaf_regional_20161128::state::{
    MAX_IP_SETS, MAX_UPDATES_PER_REQUEST,
};

use crate::wire::{call, change_token, error_code, ok_call, router};

/// Create an IPSet named `name` and return its `IPSetId`.
async fn create_ip_set(router: &MultiServiceRouter, name: &str) -> String {
    let token = change_token(router).await;
    let body = ok_call(
        router,
        "CreateIPSet",
        &json!({"Name": name, "ChangeToken": token}),
    )
    .await;
    body["IPSet"]["IPSetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateIPSet returned no IPSetId: {body}"))
        .to_string()
}

/// One `IPSetUpdate` on the wire.
fn update(action: &str, kind: &str, value: &str) -> Value {
    json!({
        "Action": action,
        "IPSetDescriptor": {"Type": kind, "Value": value}
    })
}

/// Apply `updates` to `ip_set_id` with a fresh change token.
async fn apply_updates(
    router: &MultiServiceRouter,
    ip_set_id: &str,
    updates: Vec<Value>,
) -> (u16, Value) {
    let token = change_token(router).await;
    call(
        router,
        "UpdateIPSet",
        &json!({"IPSetId": ip_set_id, "ChangeToken": token, "Updates": updates}),
    )
    .await
}

/// `WAFNonexistentItemException`, as the corpus recorded it.
const NONEXISTENT_ITEM: &str = "The referenced item does not exist.";
/// `WAFNonEmptyEntityException`, as the corpus recorded it.
const NON_EMPTY_ENTITY: &str = "The referenced entity is not empty.";
/// `WAFStaleDataException`, as the corpus recorded it.
const STALE_DATA: &str = "The input token is no longer current.";
/// `WAFNonexistentContainerException`: the same "referenced item" sentence this crate's
/// other handlers answer for the nonexistent-container class.
const NONEXISTENT_CONTAINER: &str = "The referenced item does not exist.";
/// `WAFInvalidOperationException`.
const INVALID_OPERATION: &str = "The operation is not valid.";
/// `WAFLimitsExceededException` (the model's documentation opens with this sentence).
const LIMITS_EXCEEDED: &str = "The operation exceeds a resource limit.";

/// An id of the right shape that no IPSet was ever created under.
const NEVER_CREATED_ID: &str = "00000000-0000-0000-0000-000000000000";
/// A second one, so "unknown id" is tested on more than one value.
const OTHER_UNKNOWN_ID: &str = "ffffffff-ffff-4fff-8fff-ffffffffffff";
/// A change token this service never minted.
const NEVER_MINTED_TOKEN: &str = "22222222-2222-4222-8222-222222222222";

/// One refusal to provoke: the request, the modelled error it must answer, and that
/// error's fixed sentence.
struct Case {
    label: &'static str,
    operation: &'static str,
    request: Value,
    code: &'static str,
    message: &'static str,
}

/// The `message` member of an error body, whatever the error.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// Every string of length 4 or more anywhere in `request` — the request-derived values no
/// error body may echo back.
fn request_strings(request: &Value) -> Vec<String> {
    match request {
        Value::String(text) if text.len() >= 4 => vec![text.clone()],
        Value::Array(items) => items.iter().flat_map(request_strings).collect(),
        Value::Object(members) => members.values().flat_map(request_strings).collect(),
        _ => Vec::new(),
    }
}

/// Run one case: HTTP 400, the modelled code, that code's fixed sentence, and nothing of
/// the request reflected into the body.
async fn check(router: &MultiServiceRouter, case: &Case) {
    let (status, body) = call(router, case.operation, &case.request).await;
    assert_eq!(
        status, 400,
        "case {}: {} must answer 400, got {status}: {body}",
        case.label, case.operation
    );
    assert!(
        error_code(&body).contains(case.code),
        "case {}: expected {}, got {} ({body})",
        case.label,
        case.code,
        error_code(&body)
    );
    let message = error_message(&body);
    assert_eq!(
        message, case.message,
        "case {}: {} must answer {}'s fixed sentence {:?}, got {message:?}",
        case.label, case.operation, case.code, case.message
    );
    for value in request_strings(&case.request) {
        assert!(
            !message.contains(&value),
            "case {}: the {} message must carry nothing from the request, but {message:?} \
             echoes {value:?}",
            case.label,
            case.code
        );
    }
}

/// Create an IPSet named `name` holding `addresses`, and return its `IPSetId`.
async fn ip_set_with(
    router: &MultiServiceRouter,
    name: &str,
    addresses: &[(&str, &str)],
) -> String {
    let ip_set_id = create_ip_set(router, name).await;
    if !addresses.is_empty() {
        let updates = addresses
            .iter()
            .map(|(kind, value)| update("INSERT", kind, value))
            .collect();
        let (status, body) = apply_updates(router, &ip_set_id, updates).await;
        assert_eq!(status, 200, "seeding {name} with addresses: {body}");
    }
    ip_set_id
}

#[tokio::test]
async fn every_refusal_of_one_exception_type_carries_that_type_s_fixed_sentence() {
    let router = router();

    // The records the cases are refused against: one IPSet that was deleted, one empty
    // one, and two holding different numbers of addresses.
    let deleted_id = ip_set_with(&router, "gone-ipset", &[]).await;
    let token = change_token(&router).await;
    ok_call(
        &router,
        "DeleteIPSet",
        &json!({"IPSetId": deleted_id, "ChangeToken": token}),
    )
    .await;
    let empty_id = ip_set_with(&router, "empty-ipset", &[]).await;
    let one_address_id = ip_set_with(&router, "one-address", &[("IPV4", "198.51.100.7/32")]).await;
    let three_address_id = ip_set_with(
        &router,
        "three-addresses",
        &[
            ("IPV4", "203.0.113.0/24"),
            ("IPV4", "192.0.2.0/24"),
            ("IPV6", "2001:db8::/48"),
        ],
    )
    .await;
    // A token one successful create already consumed.
    let spent_token = change_token(&router).await;
    ok_call(
        &router,
        "CreateIPSet",
        &json!({"Name": "token-spender", "ChangeToken": spent_token}),
    )
    .await;

    let mut cases = vec![
        Case {
            label: "GetIPSet on an id nothing was ever created under",
            operation: "GetIPSet",
            request: json!({"IPSetId": NEVER_CREATED_ID}),
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM,
        },
        Case {
            label: "GetIPSet on a second unknown id",
            operation: "GetIPSet",
            request: json!({"IPSetId": OTHER_UNKNOWN_ID}),
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM,
        },
        Case {
            label: "GetIPSet on the id of a deleted IPSet",
            operation: "GetIPSet",
            request: json!({"IPSetId": deleted_id}),
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM,
        },
        Case {
            label: "GetChangeTokenStatus on a token this service never minted",
            operation: "GetChangeTokenStatus",
            request: json!({"ChangeToken": NEVER_MINTED_TOKEN}),
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM,
        },
        Case {
            label: "GetSqlInjectionMatchSet on an id nothing was ever created under",
            operation: "GetSqlInjectionMatchSet",
            request: json!({"SqlInjectionMatchSetId": NEVER_CREATED_ID}),
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM,
        },
        Case {
            label: "GetGeoMatchSet on an id nothing was ever created under",
            operation: "GetGeoMatchSet",
            request: json!({"GeoMatchSetId": OTHER_UNKNOWN_ID}),
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM,
        },
        Case {
            label: "GetSizeConstraintSet on an id nothing was ever created under",
            operation: "GetSizeConstraintSet",
            request: json!({"SizeConstraintSetId": NEVER_CREATED_ID}),
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM,
        },
        Case {
            label: "CreateSqlInjectionMatchSet reusing a spent change token",
            operation: "CreateSqlInjectionMatchSet",
            request: json!({"Name": "stale-sqli", "ChangeToken": spent_token}),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "CreateGeoMatchSet reusing a spent change token",
            operation: "CreateGeoMatchSet",
            request: json!({"Name": "stale-geo", "ChangeToken": spent_token}),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "GetRegexPatternSet on an id nothing was ever created under",
            operation: "GetRegexPatternSet",
            request: json!({"RegexPatternSetId": NEVER_CREATED_ID}),
            code: "WAFNonexistentItemException",
            message: NONEXISTENT_ITEM,
        },
        Case {
            label: "CreateRegexPatternSet reusing a spent change token",
            operation: "CreateRegexPatternSet",
            request: json!({"Name": "stale-regex", "ChangeToken": spent_token}),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "CreateSizeConstraintSet reusing a spent change token",
            operation: "CreateSizeConstraintSet",
            request: json!({"Name": "stale-size", "ChangeToken": spent_token}),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "CreateIPSet reusing a spent change token",
            operation: "CreateIPSet",
            request: json!({"Name": "stale-a", "ChangeToken": spent_token}),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "CreateIPSet reusing a spent token, under a much longer resource name",
            operation: "CreateIPSet",
            request: json!({
                "Name": "stale-b-with-a-considerably-longer-resource-name",
                "ChangeToken": spent_token,
            }),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "CreateIPSet with a token this service never minted",
            operation: "CreateIPSet",
            request: json!({"Name": "stale-c", "ChangeToken": NEVER_MINTED_TOKEN}),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "UpdateIPSet reusing a spent change token",
            operation: "UpdateIPSet",
            request: json!({
                "IPSetId": empty_id,
                "ChangeToken": spent_token,
                "Updates": [update("INSERT", "IPV4", "198.51.100.9/32")],
            }),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "DeleteIPSet reusing a spent change token",
            operation: "DeleteIPSet",
            request: json!({"IPSetId": empty_id, "ChangeToken": spent_token}),
            code: "WAFStaleDataException",
            message: STALE_DATA,
        },
        Case {
            label: "UpdateIPSet with more updates than one request may carry",
            operation: "UpdateIPSet",
            request: json!({
                "IPSetId": empty_id,
                "ChangeToken": NEVER_MINTED_TOKEN,
                "Updates": (0..=MAX_UPDATES_PER_REQUEST)
                    .map(|n| update("INSERT", "IPV4", &format!("10.{}.{}.0/24", n / 256, n % 256)))
                    .collect::<Vec<Value>>(),
            }),
            code: "WAFLimitsExceededException",
            message: LIMITS_EXCEEDED,
        },
    ];
    // The cases that need a change token no call has used yet: each one is refused before
    // the token is consumed, but each gets its own so no case depends on another's.
    for (label, operation, request) in [
        (
            "DeleteIPSet on an IPSet holding one address",
            "DeleteIPSet",
            json!({"IPSetId": one_address_id}),
        ),
        (
            "DeleteIPSet on an IPSet holding three addresses",
            "DeleteIPSet",
            json!({"IPSetId": three_address_id}),
        ),
    ] {
        let mut request = request;
        request["ChangeToken"] = json!(change_token(&router).await);
        cases.push(Case {
            label,
            operation,
            request,
            code: "WAFNonEmptyEntityException",
            message: NON_EMPTY_ENTITY,
        });
    }
    for (label, request) in [
        (
            "UpdateIPSet against an id nothing was ever created under",
            json!({
                "IPSetId": NEVER_CREATED_ID,
                "Updates": [update("INSERT", "IPV4", "198.51.100.11/32")],
            }),
        ),
        (
            "UpdateIPSet against the id of a deleted IPSet",
            json!({
                "IPSetId": deleted_id,
                "Updates": [update("DELETE", "IPV6", "2001:db8:1::/48")],
            }),
        ),
    ] {
        let mut request = request;
        request["ChangeToken"] = json!(change_token(&router).await);
        cases.push(Case {
            label,
            operation: "UpdateIPSet",
            request,
            code: "WAFNonexistentContainerException",
            message: NONEXISTENT_CONTAINER,
        });
    }
    for (label, request) in [
        (
            "UpdateIPSet deleting an address the IPSet does not hold",
            json!({
                "IPSetId": one_address_id,
                "Updates": [update("DELETE", "IPV4", "192.0.2.77/32")],
            }),
        ),
        (
            "UpdateIPSet inserting an address the IPSet already holds",
            json!({
                "IPSetId": one_address_id,
                "Updates": [update("INSERT", "IPV4", "198.51.100.7/32")],
            }),
        ),
    ] {
        let mut request = request;
        request["ChangeToken"] = json!(change_token(&router).await);
        cases.push(Case {
            label,
            operation: "UpdateIPSet",
            request,
            code: "WAFInvalidOperationException",
            message: INVALID_OPERATION,
        });
    }

    for case in &cases {
        check(&router, case).await;
    }
}

/// The quota refusal of a SECOND operation carries the same `WAFLimitsExceededException`
/// sentence as the first, which is what "one constant sentence per exception type" means
/// across operations.
#[tokio::test]
async fn the_ip_set_quota_refusal_carries_the_limits_sentence_too() {
    let router = router();
    for n in 0..MAX_IP_SETS {
        create_ip_set(&router, &format!("quota-filler-{n}")).await;
    }
    let token = change_token(&router).await;
    check(
        &router,
        &Case {
            label: "CreateIPSet once the account holds its maximum number of IPSets",
            operation: "CreateIPSet",
            request: json!({"Name": "one-too-many", "ChangeToken": token}),
            code: "WAFLimitsExceededException",
            message: LIMITS_EXCEEDED,
        },
    )
    .await;
}
