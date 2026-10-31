// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for the ONE rule every operation taking an `aliasName`
//! shares: the member's value must carry the prefix the model documents on it,
//! and a value that does not is a malformed member — not a resource that is
//! missing, and not a delete that happened.
//!
//! Every assertion goes over the wire through the shared [`crate::harness`], so a
//! refusal is the refusal a client sees and a write a refused call did not make is
//! visibly absent from the next request's answer.

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::ops::INVALID_INPUT_MESSAGE;

use crate::harness::{
    call, error_code, fault_message, field_list, ok, router, store_with_validation_off,
};

/// The prefix every alias name carries, from the `aliasName` member's own model
/// documentation on each of `CreatePolicyStoreAlias`, `GetPolicyStoreAlias` and
/// `DeletePolicyStoreAlias` (`smithy_query(query="shape",
/// shape="GetPolicyStoreAliasInput")`): "The alias name must always be prefixed
/// with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The JSON pointer a refusal of that member names — the member's location in the
/// request body, which is the form `ValidationExceptionField$path` carries.
const ALIAS_NAME_POINTER: &str = "/aliasName";

/// Alias names that do NOT carry the documented prefix, spanning the ways a value
/// can miss it: absent entirely, truncated, a look-alike, present but not at the
/// start, and differing only in case or in a separator character the member's
/// `pattern` admits.
fn names_without_the_prefix() -> Vec<(&'static str, String)> {
    vec![
        ("empty name", String::new()),
        ("no prefix at all", "photos".to_string()),
        (
            "prefix without its separator",
            "policy-store-alias".to_string(),
        ),
        (
            "look-alike prefix",
            "policy-store-aliases/photos".to_string(),
        ),
        ("prefix not at the start", format!("x/{ALIAS_PREFIX}photos")),
        (
            "prefix in a different case",
            "Policy-Store-Alias/photos".to_string(),
        ),
        (
            "prefix with underscores for its hyphens",
            "policy_store_alias/photos".to_string(),
        ),
        (
            // A bare `/`-separated name: it has a resource path, just not this
            // service's.
            "another resource path",
            "policy-store/photos".to_string(),
        ),
    ]
}

/// Every request that carries `alias_name` as its `aliasName`, as
/// `(operation, body)` — the operations the rule spans. `store_id` is the store a
/// creation names, so the create row fails on the NAME and nothing else.
fn requests_naming(alias_name: &str, store_id: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "CreatePolicyStoreAlias",
            json!({ "aliasName": alias_name, "policyStoreId": store_id }),
        ),
        ("GetPolicyStoreAlias", json!({ "aliasName": alias_name })),
        ("DeletePolicyStoreAlias", json!({ "aliasName": alias_name })),
        (
            "DeletePolicyStoreAlias",
            json!({ "aliasName": alias_name, "deletionMode": "SoftDelete" }),
        ),
        (
            "DeletePolicyStoreAlias",
            json!({ "aliasName": alias_name, "deletionMode": "HardDelete" }),
        ),
    ]
}

/// RULE: an `aliasName` that does not carry the documented prefix is refused with
/// HTTP 400 `ValidationException` naming that member in `fieldList`, by EVERY
/// operation that takes the member — and the refusal is the same one whichever
/// operation read it.
///
/// The rows span the malformed names (how the prefix is missing) crossed with the
/// operations and, for the delete, with each `deletionMode` it accepts.
#[tokio::test]
async fn every_operation_refuses_an_alias_name_without_the_documented_prefix() {
    let router = router();
    let store_id = store_with_validation_off(&router).await;

    for (case, alias_name) in names_without_the_prefix() {
        for (operation, request) in requests_naming(&alias_name, &store_id) {
            let case = format!("{case} ({operation}, request {request})");
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected the ValidationException code, got {body}"
            );
            assert_eq!(
                fault_message(&body),
                INVALID_INPUT_MESSAGE,
                "case {case:?}: expected the fixed summary message, got {body}"
            );
            let fields = field_list(&body);
            assert_eq!(
                fields.len(),
                1,
                "case {case:?}: expected exactly one offending member, got {fields:?}"
            );
            let (path, reason) = &fields[0];
            assert_eq!(
                path, ALIAS_NAME_POINTER,
                "case {case:?}: expected the refusal to name {ALIAS_NAME_POINTER}, got {fields:?}"
            );
            assert!(
                reason.contains(ALIAS_PREFIX),
                "case {case:?}: expected the reason to state the required prefix, got {fields:?}"
            );
            assert!(
                !reason.contains(ALIAS_NAME_POINTER),
                "case {case:?}: the member is named by path, not repeated in the reason, \
                 got {fields:?}"
            );
        }
    }
}

/// RULE (the same one, on what the refusal must NOT do): a refused `aliasName`
/// changes nothing and reports nothing as missing — the alias whose name it
/// resembles is still `Active` and still readable under its full name, and a
/// well-formed name that names no alias keeps its own documented answers (404 for
/// the read, the idempotent 200 for the delete).
///
/// The rows differ along the alias name the fixture holds — one plain, one with
/// nested separators, one at the model's 150-character bound — so the rule is
/// asserted over a set rather than on a single name.
#[tokio::test]
async fn a_refused_alias_name_leaves_state_alone_and_is_not_a_missing_resource() {
    // `Alias`'s published length maximum (`length 0..150`), so the longest row
    // sits exactly on the bound.
    const ALIAS_MAX_LEN: usize = 150;
    let longest = "l".repeat(ALIAS_MAX_LEN - ALIAS_PREFIX.len());
    let suffixes: &[(&str, &str)] = &[
        ("plain name", "photos"),
        ("nested separators", "eu/prod/photos"),
        ("at the length bound", longest.as_str()),
    ];

    for (case, suffix) in suffixes {
        let router = router();
        let store_id = store_with_validation_off(&router).await;
        let alias_name = format!("{ALIAS_PREFIX}{suffix}");
        ok(
            &router,
            "CreatePolicyStoreAlias",
            &json!({ "aliasName": alias_name, "policyStoreId": store_id }),
        )
        .await;

        // The same name with the prefix dropped: every operation refuses it.
        for (operation, request) in requests_naming(suffix, &store_id) {
            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: {operation} must refuse the unprefixed name, got {status} {body}"
            );
        }

        // Nothing the refusals touched: the alias is still there, still Active,
        // still pointing at its store.
        let read = ok(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": alias_name }),
        )
        .await;
        assert_eq!(
            (
                read["aliasName"].as_str().unwrap_or_default(),
                read["policyStoreId"].as_str().unwrap_or_default(),
                read["state"].as_str().unwrap_or_default(),
            ),
            (alias_name.as_str(), store_id.as_str(), "Active"),
            "case {case:?}: the refused calls must have changed nothing, got {read}"
        );

        // And a WELL-FORMED name that names no alias keeps the answers the model
        // documents for it, so the prefix check did not swallow them.
        let absent = format!("{ALIAS_PREFIX}never-created");
        let (status, body) = call(
            &router,
            "GetPolicyStoreAlias",
            &json!({ "aliasName": absent }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a well-formed absent name is still not-found, got {status} {body}"
        );
        for mode in [None, Some("SoftDelete"), Some("HardDelete")] {
            let mut request = json!({ "aliasName": absent });
            if let Some(mode) = mode {
                request["deletionMode"] = json!(mode);
            }
            let (status, body) = call(&router, "DeletePolicyStoreAlias", &request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting a well-formed absent name is still the documented \
                 success for deletionMode={mode:?}, got {status} {body}"
            );
        }
    }
}
