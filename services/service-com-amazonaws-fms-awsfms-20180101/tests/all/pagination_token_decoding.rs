// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE `PaginationToken` decode rule every paginated
//! operation of this service shares.
//!
//! The rule: a `NextToken` names a page boundary exactly when it is the hex
//! encoding of a UTF-8 store key of that page class's own shape; any other value
//! names no boundary, so the request is refused rather than answered with an
//! invented page, and a refusal reports nothing and changes nothing.
//!
//! The decode step itself (`crate::state::decode_key_token`) is one helper every
//! specialised decoder delegates to, and each page class adds only its own shape
//! check on the decoded key — an `AWSAccountId` for the account-keyed pages, a
//! 36-character minted identifier for the policy-keyed ones. This suite drives
//! that shared path end-to-end through an operation of EACH class, so the two
//! kinds of page cannot come to disagree about what a token of this service is:
//!
//! * account-keyed — `ListAdminsManagingAccount` and `ListMemberAccounts`;
//! * minted-identifier-keyed — `ListPolicies`.
//!
//! Both rules are asserted over a TABLE rather than one request: the rows differ
//! in the page class, in the encoding form of the token (not hex at all, hex of
//! an odd number of digits, hex of bytes that are not UTF-8), in the CONTENT of
//! the decoded key (a control character, a key of another page class) and, for
//! the accepted tokens, in which of the class's own keys the token names and in
//! the page size the walk uses.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the wire harness in [`crate::policy_lifecycle`] — so the assertions
//! are about the WIRE response.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_scopes::{full_admin_scope, put_admin_account, wait_out_the_onboarding};
use crate::administrator_gated_operations::associate_admin;
use crate::compliance_status::put_policy;
use crate::policy_lifecycle::{call, emulator, error_code};

/// Administrator accounts onboarded beside the default one for the
/// `ListAdminsManagingAccount` class, so that class has several page boundaries.
/// The model constrains `AWSAccountId` only to `^[0-9]+$` with length 1..1024
/// (`smithy_query(query="shape", shape="AWSAccountId")`), so these differ in
/// length and digits.
const ONBOARDED_ADMINS: &[&str] = &["123456789012", "7"];

/// Account ids a stored policy names under the `ACCOUNT` key of its
/// `IncludeMap`, which is how the organisation roster `ListMemberAccounts`
/// reports gets more than one entry.
const SCOPED_ACCOUNTS: &[&str] = &["222222222222", "7", "123456789012"];

/// A key shaped like the ones the policy-keyed pages use — the form the model
/// gives `com.amazonaws.fms#PolicyId` (36 characters, `^[a-z0-9A-Z-]{36}$`) —
/// and therefore a key of NO account-keyed page.
const POLICY_SHAPED_KEY: &str = "0f13c2a4-1111-4a26-9d0e-6b4f8d2a7c31";

/// A key shaped like the ones the account-keyed pages use (an `AWSAccountId`),
/// and therefore a key of no policy-keyed page.
const ACCOUNT_SHAPED_KEY: &str = "000000000000";

/// One paginated page class: the operation that serves it, the output member
/// carrying its items, the member of an item that is the store key the page is
/// ordered by (`None` when the item IS that key), the modelled error code the
/// operation refuses a token it did not mint with, and a key of another class.
struct PageClass {
    operation: &'static str,
    items: &'static str,
    key_member: Option<&'static str>,
    refusal: &'static str,
    foreign_key: &'static str,
}

/// The page classes the shared decode rule is asserted over: one account-keyed
/// operation per caller role (the administrators managing the caller, and the
/// organisation's accounts) and one keyed by a minted identifier.
fn page_classes() -> Vec<PageClass> {
    vec![
        PageClass {
            operation: "ListAdminsManagingAccount",
            items: "AdminAccounts",
            key_member: None,
            refusal: "InvalidInputException",
            foreign_key: POLICY_SHAPED_KEY,
        },
        PageClass {
            operation: "ListMemberAccounts",
            items: "MemberAccounts",
            key_member: None,
            refusal: "ValidationException",
            foreign_key: POLICY_SHAPED_KEY,
        },
        PageClass {
            operation: "ListPolicies",
            items: "PolicyList",
            key_member: Some("PolicyId"),
            refusal: "ValidationException",
            foreign_key: ACCOUNT_SHAPED_KEY,
        },
    ]
}

/// A fresh emulator holding enough of this class's resources for its page to
/// have several boundaries, each created the way a client creates it.
async fn prepare(class: &PageClass) -> MultiServiceRouter {
    let router = emulator();
    associate_admin(&router).await;
    match class.operation {
        "ListAdminsManagingAccount" => {
            // An onboarded administrator manages the caller exactly when its
            // `AccountScope` covers it, so these carry the full scope.
            for account in ONBOARDED_ADMINS {
                put_admin_account(
                    &router,
                    class.operation,
                    account,
                    Some(&full_admin_scope()),
                )
                .await;
            }
            wait_out_the_onboarding();
        }
        "ListMemberAccounts" => {
            put_policy(
                &router,
                "organisation-roster",
                Some(json!({ "ACCOUNT": SCOPED_ACCOUNTS })),
                None,
            )
            .await;
        }
        "ListPolicies" => {
            for name in ["first.policy", "second.policy", "third.policy"] {
                put_policy(&router, name, None, None).await;
            }
        }
        other => panic!("case {other}: this suite has no setup for that page class"),
    }
    router
}

/// One call of a class's operation, as `(http status, error code or "", the
/// store keys of the items served, NextToken)`.
async fn page(
    router: &MultiServiceRouter,
    class: &PageClass,
    request: &Value,
) -> (u16, String, Vec<String>, Option<String>) {
    let (status, body) = call(router, class.operation, request).await;
    let code = if status == 200 {
        String::new()
    } else {
        error_code(&body)
    };
    let keys: Vec<String> = body[class.items]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let key = match class.key_member {
                Some(member) => &item[member],
                None => item,
            };
            key.as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {}: a page item must carry its store key, got {item:?}",
                        class.operation
                    )
                })
                .to_string()
        })
        .collect();
    let next_token = body
        .get("NextToken")
        .filter(|token| !token.is_null())
        .and_then(Value::as_str)
        .map(str::to_string);
    (status, code, keys, next_token)
}

/// The whole page of a prepared class, asserted to be served and to hold at
/// least two keys — the precondition the boundary rows below need.
async fn whole_page(router: &MultiServiceRouter, class: &PageClass) -> Vec<String> {
    let operation = class.operation;
    let (status, code, keys, next) = page(router, class, &json!({})).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case {operation}: expected the unbounded page to be served, got {status} / {code:?}"
    );
    assert!(
        keys.len() >= 2 && next.is_none(),
        "case {operation}: this suite needs one whole page of at least two keys, got {keys:?} \
         with NextToken {next:?}"
    );
    keys
}

/// A token is a page boundary exactly when it is the hex of a UTF-8 key of that
/// page class: the hex of ANY of the class's own keys resumes the page there,
/// and a value that is not hex, not UTF-8, carries a control character or
/// decodes to a key of another class is refused with the operation's modelled
/// error — reporting nothing and leaving the page as it was.
#[tokio::test]
async fn a_token_is_a_boundary_exactly_when_it_is_the_hex_of_a_key_of_that_page_class() {
    for class in page_classes() {
        let operation = class.operation;
        let router = prepare(&class).await;
        let whole = whole_page(&router, &class).await;

        for (index, key) in whole.iter().enumerate() {
            let token = hex::encode(key);
            let (status, code, resumed, _) =
                page(&router, &class, &json!({ "NextToken": token })).await;
            assert_eq!(
                (status, code.as_str()),
                (200, ""),
                "case {operation} / boundary {key:?}: a token naming a key of this page class \
                 must resume the page, got {status} / {code:?}"
            );
            assert_eq!(
                resumed,
                whole[index..].to_vec(),
                "case {operation} / boundary {key:?}: expected the page to resume at that key, \
                 got {resumed:?}"
            );
        }

        let refused = [
            ("not hex at all", "zzzz".to_string()),
            ("hex of an odd number of digits", "abc".to_string()),
            ("hex of bytes that are not UTF-8", "ff".to_string()),
            (
                "hex of one of its own keys carrying a control character",
                hex::encode(format!("{}\u{1}", whole[0])),
            ),
            (
                "hex of a key of another page class",
                hex::encode(class.foreign_key),
            ),
        ];
        for (case, token) in refused {
            let (status, code, reported, next) =
                page(&router, &class, &json!({ "NextToken": token })).await;
            assert_eq!(
                (status, code.as_str()),
                (400, class.refusal),
                "case {operation} / {case:?}: a token this page class did not mint must be \
                 refused, got {status} / {code:?}"
            );
            assert!(
                reported.is_empty() && next.is_none(),
                "case {operation} / {case:?}: a refused call must report nothing, got \
                 {reported:?} / {next:?}"
            );

            let (_, _, after, _) = page(&router, &class, &json!({})).await;
            assert_eq!(
                after, whole,
                "case {operation} / {case:?}: a refused token must leave the page as it was, got \
                 {after:?}"
            );
        }
    }
}

/// The token a class MINTS decodes through the same rule, so walking the chain
/// reports every key of the class exactly once, in the page's own order, at
/// every page size — the encode/decode round trip of the shared path.
#[tokio::test]
async fn the_token_a_page_class_mints_walks_its_whole_page_set() {
    for class in page_classes() {
        let operation = class.operation;
        let router = prepare(&class).await;
        let whole = whole_page(&router, &class).await;

        for max_results in [1usize, 2] {
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut ended = false;
            // One more round than the chain can need, so a token that never
            // clears fails here instead of looping forever.
            for round in 0..whole.len() + 3 {
                let mut request = json!({ "MaxResults": max_results });
                if let Some(token) = token.as_deref() {
                    request["NextToken"] = json!(token);
                }
                let (status, code, keys, next) = page(&router, &class, &request).await;
                assert_eq!(
                    (status, code.as_str()),
                    (200, ""),
                    "case {operation} / MaxResults {max_results} / page {round}: expected 200, \
                     got {status} / {code:?}"
                );
                assert!(
                    keys.len() <= max_results,
                    "case {operation} / MaxResults {max_results} / page {round}: a page must not \
                     exceed the requested size, got {keys:?}"
                );
                walked.extend(keys);
                match next {
                    Some(next) => token = Some(next),
                    None => {
                        ended = true;
                        break;
                    }
                }
            }
            assert!(
                ended,
                "case {operation} / MaxResults {max_results}: the NextToken chain did not end, \
                 walked {walked:?}"
            );
            assert_eq!(
                walked, whole,
                "case {operation} / MaxResults {max_results}: walking the minted tokens must \
                 report every key exactly once in the page's order, got {walked:?}"
            );
        }
    }
}
