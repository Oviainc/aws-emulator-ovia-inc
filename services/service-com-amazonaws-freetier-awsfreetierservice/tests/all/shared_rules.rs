// SPDX-License-Identifier: CC0-1.0

//! The rules this crate states ONCE and more than one operation serves,
//! driven over the wire through an operation from every call site.
//!
//! Two rules live in `crate::state` and are reached from two handlers each, so
//! these tests assert the handlers agree rather than that one of them happens
//! to be right:
//!
//! 1. The page cut and the `nextToken` it issues (`crate::state::cut_page`,
//!    beside the token codec and the page-size resolution it belongs with):
//!    a page reports at most `maxResults` of the selected rows, in the stored
//!    order, and a token is issued exactly while a selected row is still
//!    unreported — the `@paginated` contract is that an absent token ends the
//!    iteration, so the exact-fit page carries none. Driven through
//!    `ListAccountActivities` AND `GetFreeTierUsage`.
//! 2. The `languageCode` membership check
//!    (`crate::state::language_code_refusal`, the sibling of the per-row
//!    `LocalizedText::in_language` resolution it guards): a code outside the
//!    modelled `LanguageCode` enum names no language the catalog could be read
//!    in and is refused, in one wording. Driven through `GetAccountActivity`
//!    AND `ListAccountActivities`.

use serde_json::{json, Value};
use service_com_amazonaws_freetier_awsfreetierservice::ops::types::LANGUAGE_CODE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use crate::free_tier::{call, error_code, harness, ok, text, ACTIVITIES_PAGE_SIZE_CEILING};

/// The largest page `ListAccountActivities` serves
/// ([`ACTIVITIES_PAGE_SIZE_CEILING`]), which both paginated operations accept,
/// so this asks for the whole collection in one call.
const MAX_RESULTS_CEILING: usize = ACTIVITIES_PAGE_SIZE_CEILING as usize;

/// This service's paginated operations, each with the output member carrying
/// its page. Both cut their page and issue their token through the one shared
/// rule, so every row below is asserted against the same expectation.
const PAGINATED: [(&str, &str); 2] = [
    ("ListAccountActivities", "activities"),
    ("GetFreeTierUsage", "freeTierUsages"),
];

/// This service's activity readers, both of which model an optional
/// `languageCode` and check it through the one shared rule.
const ACTIVITY_READERS: [&str; 2] = ["GetAccountActivity", "ListAccountActivities"];

/// Every row one paginated operation reports when asked for the whole
/// collection at once, which is the order the pages below must reassemble.
async fn whole_collection(
    router: &MultiServiceRouter,
    operation: &str,
    items_member: &str,
) -> Vec<Value> {
    let payload = ok(router, operation, &json!({"maxResults": MAX_RESULTS_CEILING})).await;
    assert!(
        payload["nextToken"].is_null(),
        "case {operation} at the maxResults ceiling: a page holding every row must carry no \
         nextToken, got {payload}"
    );
    payload[items_member]
        .as_array()
        .unwrap_or_else(|| panic!("case {operation}: {items_member} must be a list, got {payload}"))
        .clone()
}

/// The id of one stored activity, so the detail reader has something to read.
async fn one_activity_id(router: &MultiServiceRouter) -> String {
    let rows = whole_collection(router, "ListAccountActivities", "activities").await;
    let first = rows
        .first()
        .expect("the account holds at least one activity");
    text(first, "activityId")
}

/// Both paginated operations cut a page and issue a token by the SAME rule, at
/// every page size: the page holds `min(maxResults, rows still unreported)`
/// rows, a token comes back exactly while a row is still unreported, and the
/// pages reassemble the collection in the stored order exactly once.
///
/// The page sizes span the class the rule generalises over — one row at a time,
/// sizes that divide the collection and sizes that do not, the EXACT fit (the
/// boundary where a token must not be issued) and a size past the end — and the
/// same table runs against both operations, so a change to one copy of the rule
/// cannot pass here.
#[tokio::test]
async fn both_paginated_operations_page_by_one_rule() {
    let router = harness().0;

    for (operation, items_member) in PAGINATED {
        let rows = whole_collection(&router, operation, items_member).await;
        let total = rows.len();
        assert!(
            total >= 2,
            "case {operation}: the account must hold at least two rows for paging to be \
             observable, got {total}"
        );

        for page_size in 1..=total + 1 {
            let mut reported: Vec<Value> = Vec::new();
            let mut token: Option<String> = None;
            let mut terminated = false;

            // A walk over `total` rows takes at most `total` pages plus the
            // empty tail, so anything beyond that is a non-terminating walk.
            for page_index in 0..total + 2 {
                let mut body = json!({"maxResults": page_size});
                if let Some(token) = token.as_deref() {
                    body["nextToken"] = json!(token);
                }
                let payload = ok(&router, operation, &body).await;
                let items = payload[items_member]
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!(
                            "case {operation} size {page_size} page {page_index}: {items_member} \
                             must be a list, got {payload}"
                        )
                    })
                    .clone();

                let already = reported.len();
                let expected = (total - already).min(page_size);
                assert_eq!(
                    items.len(),
                    expected,
                    "case {operation} size {page_size} page {page_index}: expected {expected} \
                     rows after {already} reported, got {}",
                    items.len()
                );
                reported.extend(items);

                let issued = payload["nextToken"].as_str().map(str::to_string);
                assert_eq!(
                    issued.is_some(),
                    reported.len() < total,
                    "case {operation} size {page_size} page {page_index}: a nextToken is issued \
                     exactly while a selected row is unreported ({} of {total} reported), got \
                     {issued:?}",
                    reported.len()
                );
                match issued {
                    Some(issued) => token = Some(issued),
                    None => {
                        terminated = true;
                        break;
                    }
                }
            }

            assert!(
                terminated,
                "case {operation} size {page_size}: the pagination walk did not terminate after \
                 {} rows",
                reported.len()
            );
            assert_eq!(
                reported, rows,
                "case {operation} size {page_size}: the pages must reassemble the collection in \
                 the stored order exactly once, got {reported:?}"
            );
        }
    }
}

/// Both activity readers check `languageCode` by the SAME rule: a code the
/// model's enum does not declare is refused with the reader's own modelled
/// `ValidationException` (HTTP 400) carrying ONE message, and every modelled
/// code is accepted by both.
///
/// The refused rows differ in HOW the code misses the enum — separator, case,
/// the primary subtag alone, an unmodelled region, an unmodelled language,
/// nothing at all — and the accepted rows are the model's whole `LanguageCode`
/// value set (`shared_types::LANGUAGE_CODE_VALUES`), including codes this
/// crate's other suites do not name.
#[tokio::test]
async fn both_activity_readers_check_the_language_code_by_one_rule() {
    let router = harness().0;
    let activity_id = one_activity_id(&router).await;

    /// A request body for one reader at one `languageCode`.
    fn body(operation: &str, activity_id: &str, code: &str) -> Value {
        match operation {
            "GetAccountActivity" => json!({"activityId": activity_id, "languageCode": code}),
            _ => json!({"maxResults": 1, "languageCode": code}),
        }
    }

    for code in ["en_US", "EN-US", "en", "fr-CA", "sv-SE", "", "zh", "en-US\t"] {
        assert!(
            !LANGUAGE_CODE_VALUES.contains(&code),
            "case {code:?}: the probe must not be a modelled LanguageCode"
        );

        let mut messages: Vec<(&str, String)> = Vec::new();
        for operation in ACTIVITY_READERS {
            let request = body(operation, &activity_id, code);
            let (status, payload) = call(&router, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {operation} {code:?}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "ValidationException",
                "case {operation} {code:?}: expected a ValidationException, got {payload}"
            );
            let message = text(&payload, "message");
            assert!(
                message.contains(code) || code.is_empty(),
                "case {operation} {code:?}: the refusal must name the code it refused, got \
                 {message:?}"
            );
            for modelled in LANGUAGE_CODE_VALUES {
                assert!(
                    message.contains(modelled),
                    "case {operation} {code:?}: the refusal must name the modelled code \
                     {modelled}, got {message:?}"
                );
            }
            messages.push((operation, message));
        }

        let [(first_op, first), (second_op, second)] = messages.as_slice() else {
            unreachable!("every reader answered exactly once");
        };
        assert_eq!(
            first, second,
            "case {code:?}: {first_op} and {second_op} must refuse in one wording, got {first:?} \
             against {second:?}"
        );
    }

    // Every modelled code is accepted by both readers, so the check refuses
    // nothing the catalog can be read in.
    for code in LANGUAGE_CODE_VALUES {
        for operation in ACTIVITY_READERS {
            let request = body(operation, &activity_id, code);
            let (status, payload) = call(&router, operation, &request).await;
            assert_eq!(
                status, 200,
                "case {operation} {code}: a modelled language code must be served, got {status} \
                 with {payload}"
            );
        }
    }
}
