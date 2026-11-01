// SPDX-License-Identifier: CC0-1.0

//! The one cursor rule every paginated operation of this service applies,
//! driven through each of the four operations that page.
//!
//! The rule now lives in one place (`crate::state::page_offset`), and each
//! handler only wraps its message in its own operation's error variant:
//!
//! * an absent `nextToken` resumes at the first result;
//! * a `nextToken` the operation itself minted resumes the walk past that page;
//! * a `nextToken` some OTHER paginator minted, or one no paginator mints at
//!   all, is a 400 `ValidationException` carrying one sentence.
//!
//! The rows span what the rule generalises over: which of the four operations
//! serves the request, and which shape of foreign or malformed cursor it is
//! handed (another paginator's token at offset 0 and past it, a token that is
//! not hex at all, a token that carries no offset, and one whose offset is not a
//! count). Everything goes through the same `MultiServiceRouter` the emulator
//! boots, so what is pinned is the HTTP behaviour; the harness (`Emulator`,
//! `text`) is the one the sibling lifecycle suite defines.

use serde_json::{json, Value};
use service_com_amazonaws_savingsplans_awssavingsplan::state;

use crate::savings_plans_lifecycle::{text, Emulator, ONE_YEAR, THREE_YEARS};

/// The sentence with which this service refuses a cursor it did not mint.
pub(crate) const REFUSAL: &str = "nextToken is not a token this operation issued";

/// Every paginated operation of this service, as `(operation, page kind, the
/// member it reports its page in)`. The page kind is the one each handler
/// passes to the shared cursor rule, so a token minted under any OTHER kind is
/// foreign to this operation.
pub(crate) const PAGINATED: [(&str, &str, &str); 4] = [
    ("DescribeSavingsPlansOfferings", "offerings", "searchResults"),
    (
        "DescribeSavingsPlansOfferingRates",
        "offeringRates",
        "searchResults",
    ),
    ("DescribeSavingsPlans", "plans", "savingsPlans"),
    ("DescribeSavingsPlanRates", "planRates", "searchResults"),
];

/// Two plans, bought from two different offerings, so the plan search and a
/// plan's rate card both have more than one page to walk. Returns the id of the
/// first.
async fn two_plans(emulator: &Emulator) -> String {
    let mut ids: Vec<String> = Vec::new();
    for (plan_type, payment_option, duration, commitment) in [
        ("EC2Instance", state::PAYMENT_NO_UPFRONT, ONE_YEAR, "0.1"),
        ("Database", state::PAYMENT_ALL_UPFRONT, THREE_YEARS, "0.5"),
    ] {
        let offering = emulator.offering(plan_type, payment_option, duration).await;
        ids.push(
            emulator
                .purchase(json!({
                    "savingsPlanOfferingId": text(&offering, "offeringId"),
                    "commitment": commitment,
                }))
                .await,
        );
    }
    ids.swap_remove(0)
}

/// The request that drives `operation` with the smallest page the model admits.
pub(crate) fn request(operation: &str, savings_plan_id: &str, page_size: i64) -> Value {
    let mut body = json!({ "maxResults": page_size });
    if operation == "DescribeSavingsPlanRates" {
        body["savingsPlanId"] = json!(savings_plan_id);
    }
    body
}

/// Assert `answer` is the shared refusal: a 400 `ValidationException` carrying
/// the one sentence.
pub(crate) fn assert_refused(case: &str, answer: &crate::savings_plans_lifecycle::Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.code.ends_with("ValidationException"),
        "case {case:?}: expected ValidationException, got {:?}",
        answer.code
    );
    assert_eq!(
        text(&answer.body, "message"),
        REFUSAL,
        "case {case:?}: expected the shared refusal {REFUSAL:?}, got {:?}",
        text(&answer.body, "message")
    );
}

/// Every paginated operation applies the same cursor rule: no cursor starts at
/// the first result, its own cursor resumes past that page, and any other
/// paginator's cursor — or one no paginator mints — is refused identically.
#[tokio::test]
async fn every_paginated_operation_follows_one_cursor_rule() {
    let emulator = Emulator::new();
    let savings_plan_id = two_plans(&emulator).await;

    for (operation, kind, results) in PAGINATED {
        // An absent cursor is offset 0: the first page of the walk.
        let case = format!("{operation} with no cursor");
        let first = emulator
            .call(operation, request(operation, &savings_plan_id, 1))
            .await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            first.status, first.body
        );
        let first_rows = first.list(results).to_vec();
        assert_eq!(
            first_rows.len(),
            1,
            "case {case:?}: expected the first row alone, got {}",
            first_rows.len()
        );
        let own_token = first
            .get("nextToken")
            .as_str()
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected a cursor to resume at, got {:?}", first.body)
            });

        // The cursor this operation itself minted resumes the walk, and serves
        // what the first page did not.
        let case = format!("{operation} resumed by its own cursor");
        let mut resumed_request = request(operation, &savings_plan_id, 1);
        resumed_request["nextToken"] = json!(own_token);
        let resumed = emulator.call(operation, resumed_request).await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: expected 200 for {own_token:?}, got {} {:?}",
            resumed.status, resumed.body
        );
        let resumed_rows = resumed.list(results).to_vec();
        assert_eq!(
            resumed_rows.len(),
            1,
            "case {case:?}: expected one more row, got {}",
            resumed_rows.len()
        );
        assert_ne!(
            resumed_rows, first_rows,
            "case {case:?}: a resumed page must serve past the first, got {resumed_rows:?}"
        );

        // Every other paginator's cursor, and every spelling no paginator
        // mints, is the same refusal out of this operation.
        let mut foreign: Vec<(String, String)> = Vec::new();
        for (_, other, _) in PAGINATED {
            if other == kind {
                continue;
            }
            foreign.push((
                format!("the {other} cursor at its first page"),
                state::encode_page_token(other, 0),
            ));
            foreign.push((
                format!("the {other} cursor past its first page"),
                state::encode_page_token(other, 1),
            ));
        }
        foreign.push((
            "a token that is not hex at all".to_string(),
            "ZZZZ".to_string(),
        ));
        foreign.push((
            "a token carrying no offset".to_string(),
            hex::encode(kind),
        ));
        foreign.push((
            "a token whose offset is not a count".to_string(),
            hex::encode(format!("{kind}:-1")),
        ));
        foreign.push((
            "a token whose offset is not a number".to_string(),
            hex::encode(format!("{kind}:last")),
        ));

        for (label, token) in foreign {
            let case = format!("{operation} handed {label}");
            let mut body = request(operation, &savings_plan_id, 1);
            body["nextToken"] = json!(token);
            let answer = emulator.call(operation, body).await;
            assert_refused(&case, &answer);
        }
    }
}
