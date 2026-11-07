// SPDX-License-Identifier: CC0-1.0

//! A listing whose input declares no required member, asked with members that
//! all satisfy their modelled constraints, end to end over HTTP.
//!
//! The rule under test: a `@paginated` listing of this service whose input
//! declares NO `@required` member accepts every request whose members satisfy
//! their modelled constraints — including the request that carries none of them
//! at all — and answers HTTP 200 with the member the model paginates
//! (`paginated.items`) PRESENT and empty and no `nextToken` when nothing it
//! reports matches; `ValidationException` ("the input fails to satisfy the
//! constraints specified by the service", `@error("client")`, `@httpError(400)`)
//! is reserved for a member that fails a modelled constraint or whose value lies
//! outside the fixed value set this service publishes for it. An empty match set
//! is the first page of a listing, never a client error, so a caller that
//! paginates can conclude there is nothing to page.
//!
//! The two operations in that class are the rows
//! (`smithy_query(query="operation",
//! operation="ListAgreementCharges,ListBillingAdjustmentRequests")`: every
//! member of either input is optional, both are `@paginated` with
//! `items=items`), so the rule is pinned over the class rather than on the one
//! operation it was reported against. The sibling listings are not rows: their
//! `partyType` (`ListAgreementPaymentRequests`,
//! `ListAgreementCancellationRequests`) and their `agreementId`/`groupBy`
//! (`ListAgreementInvoiceLineItems`) ARE `@required`, so a request carrying no
//! member is a constraint violation there and refusing it is correct.
//!
//! The rows differ in what the rule generalises over:
//!
//! * which listing of the class answers (both of them);
//! * which admitted members the request carries — none at all, each one alone,
//!   and every one at once — at both published bounds of `maxResults`
//!   (`@range(min: 1, max: 50)`);
//! * what the store holds while nothing matches: nothing at all, an agreement
//!   holding none of the records either listing reports, and a store holding
//!   charges and a billing adjustment request that the request's own admitted
//!   `agreementType` selects none of — so an empty page is never the accident of
//!   an empty store;
//! * and the complement, so the rows above are about an accepted request and not
//!   about a listing that answers a page whatever it is asked: a member OUTSIDE
//!   its modelled constraints is still refused with 400.
//!
//! The wire harness (`call`, `emulator`) is `agreement_lifecycle`'s and every
//! record is stored through the operation that creates it, so this suite stands
//! up no second harness and writes no id by hand.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{agreement_over, call, emulator, invoiced_agreement, term, Answer};
use crate::billing_adjustments::{entry, submit};
use crate::purchase_order_updates::agreement_holding;

/// The one catalog AWS Marketplace exposes, which every agreement of this
/// service is created in — an admitted value of the `catalog` member.
const CATALOG: &str = "AWSMarketplace";

/// The `agreementType` every agreement this service mints carries.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// The other `agreementType` the model documents ("Values are
/// `PurchaseAgreement` or `VendorInsightsAgreement`", `smithy_query(query="doc",
/// shape="DescribeAgreementOutput", member="agreementType")`): a value the
/// member admits that no agreement of this service carries, so it selects
/// nothing on a store that holds records.
const UNHELD_AGREEMENT_TYPE: &str = "VendorInsightsAgreement";

/// `MaxResults`' published bounds, `@range(min: 1, max: 50)`
/// (`smithy_query(query="shape", shape="MaxResults")`).
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 50;

/// The `agreementId` member of either listing is `@length(min: 1, max: 64)`, so
/// one character past this is outside the constraint.
const ID_MAX: usize = 64;

/// `NextToken` is `@length(max: 8192)` with `^[a-zA-Z0-9+/=_-]+$`.
const NEXT_TOKEN_MAX: usize = 8192;

/// One listing whose input declares no required member: its wire name and the
/// member the model paginates (`paginated.items`), which is the member an
/// accepted call must carry.
struct Listing {
    operation: &'static str,
    page: &'static str,
}

fn listings() -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListAgreementCharges",
            page: "items",
        },
        Listing {
            operation: "ListBillingAdjustmentRequests",
            page: "items",
        },
    ]
}

/// The requests every member of which satisfies its modelled constraints, the
/// first of them carrying no member at all — which is a well-formed request
/// precisely because the input declares none as required.
///
/// `holds_records` drops the rows whose `agreementType` every stored agreement
/// carries: on a store that holds charges and adjustment requests those rows
/// select something, and this suite is about the answer when nothing matches.
fn well_formed_requests(holds_records: bool) -> Vec<(String, Value)> {
    let mut rows = vec![
        ("no member at all".to_string(), json!({})),
        (
            format!("the published catalog {CATALOG:?} alone"),
            json!({"catalog": CATALOG}),
        ),
        (
            format!("the agreement type {AGREEMENT_TYPE:?} alone"),
            json!({"agreementType": AGREEMENT_TYPE}),
        ),
        (
            format!("the agreement type {UNHELD_AGREEMENT_TYPE:?} alone"),
            json!({"agreementType": UNHELD_AGREEMENT_TYPE}),
        ),
        (
            format!("the smallest page the model admits ({MAX_RESULTS_MIN})"),
            json!({"maxResults": MAX_RESULTS_MIN}),
        ),
        (
            format!("the largest page the model admits ({MAX_RESULTS_MAX})"),
            json!({"maxResults": MAX_RESULTS_MAX}),
        ),
        (
            "every admitted member at once".to_string(),
            json!({
                "catalog": CATALOG,
                "agreementType": UNHELD_AGREEMENT_TYPE,
                "maxResults": MAX_RESULTS_MAX,
            }),
        ),
    ];
    if holds_records {
        rows.retain(|(_, request)| {
            request["agreementType"].as_str() == Some(UNHELD_AGREEMENT_TYPE)
        });
    }
    rows
}

/// Assert one answer is the first page of a listing that selects nothing: HTTP
/// 200, the paginated member present and empty, and no `nextToken` to resume a
/// listing that has nothing left.
fn assert_empty_first_page(case: &str, listing: &Listing, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    let page = answer.body.get(listing.page).unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the {} member to be present, got {:?}",
            listing.page, answer.body
        )
    });
    let page = page.as_array().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected the {} member to be a list, got {:?}",
            listing.page, answer.body
        )
    });
    assert!(
        page.is_empty(),
        "case {case:?}: expected an empty page, got {:?}",
        answer.body
    );
    assert!(
        answer.body.get("nextToken").is_none(),
        "case {case:?}: expected no nextToken on the last page, got {:?}",
        answer.body
    );
}

/// Assert one answer is the modelled refusal of a member outside its
/// constraints: 400 `ValidationException`, and no page, so the refusal cannot be
/// mistaken for a served listing.
fn assert_refused(case: &str, listing: &Listing, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {:?}",
        answer.body
    );
    assert!(
        answer.body.get(listing.page).is_none(),
        "case {case:?}: a refused listing reports no page, got {:?}",
        answer.body
    );
}

/// Store the records either listing reports, through the operations that create
/// them: an agreement holding charges, and an agreement whose invoice carries a
/// submitted billing adjustment request.
async fn store_records(router: &MultiServiceRouter, tag: &str) -> String {
    let (holding, charge_ids) = agreement_holding(router, &format!("ap-{tag}charges"), 2).await;
    assert_eq!(
        charge_ids.len(),
        2,
        "case {tag:?}: the quote must mint the charges the report carries, got {charge_ids:?}"
    );
    let (invoiced, invoice) = invoiced_agreement(router, &format!("ap-{tag}invoiced")).await;
    let submitted = submit(
        router,
        vec![entry(&invoiced, &invoice, "1", &format!("tok-{tag}"))],
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "case {tag:?}: submitting one billing adjustment request must succeed, got {} with {:?}",
        submitted.status, submitted.body
    );
    holding
}

/// Every listing of the class answers a well-formed request that selects nothing
/// with the empty first page — including the request that carries no member at
/// all — whatever the store holds.
#[tokio::test]
async fn a_well_formed_request_that_selects_nothing_is_served_as_an_empty_page() {
    for (store, holds_records) in [
        ("an empty store", false),
        ("a store holding no record either listing reports", false),
        ("a store holding charges and an adjustment request", true),
    ] {
        let router = emulator();
        if store == "a store holding no record either listing reports" {
            agreement_over(&router, "ap-emptypage", vec![term("eula.v1")]).await;
        }
        if holds_records {
            store_records(&router, "emptypagefull").await;
        }

        for listing in listings() {
            for (request_case, request) in well_formed_requests(holds_records) {
                let case = format!("{} on {store}: {request_case}", listing.operation);
                let answer = call(&router, listing.operation, request).await;
                assert_empty_first_page(&case, &listing, &answer);
            }
        }
    }
}

/// The complement: a member whose value lies OUTSIDE its modelled constraints
/// still fails one, so it is refused rather than served as an empty page.
#[tokio::test]
async fn a_member_outside_its_modelled_constraints_is_still_refused() {
    let router = emulator();
    store_records(&router, "emptypagerefused").await;

    let malformed: Vec<(String, Value)> = vec![
        (
            format!("one below the smallest page the model admits ({MAX_RESULTS_MIN})"),
            json!({"maxResults": MAX_RESULTS_MIN - 1}),
        ),
        (
            format!("one past the largest page the model admits ({MAX_RESULTS_MAX})"),
            json!({"maxResults": MAX_RESULTS_MAX + 1}),
        ),
        (
            format!("an agreementId one character past its maximum length ({ID_MAX})"),
            json!({"agreementId": "A".repeat(ID_MAX + 1)}),
        ),
        (
            "an agreementId below its minimum length".to_string(),
            json!({"agreementId": ""}),
        ),
        (
            "a nextToken carrying characters its pattern does not admit".to_string(),
            json!({"nextToken": "not a token!"}),
        ),
        (
            format!("a nextToken one character past its maximum length ({NEXT_TOKEN_MAX})"),
            json!({"nextToken": "a".repeat(NEXT_TOKEN_MAX + 1)}),
        ),
    ];

    for listing in listings() {
        for (case, request) in &malformed {
            let case = format!("{}: {case}", listing.operation);
            let answer = call(&router, listing.operation, request.clone()).await;
            assert_refused(&case, &listing, &answer);
        }
    }
}

/// Answering the empty page changes nothing a later caller sees: the same
/// request answers the same page every time, and the records the store holds are
/// reported identically before and after.
#[tokio::test]
async fn answering_an_empty_page_leaves_what_a_later_caller_sees_unchanged() {
    let router = emulator();
    let holding = store_records(&router, "emptypagereadonly").await;

    let before = call(
        &router,
        "ListAgreementCharges",
        json!({"agreementId": holding}),
    )
    .await;
    assert_eq!(
        before.status, 200,
        "reporting the charges of {holding} must succeed, got {} with {:?}",
        before.status, before.body
    );

    for listing in listings() {
        for (request_case, request) in well_formed_requests(true) {
            let mut answers: Vec<Value> = Vec::new();
            for round in 0..2 {
                let case =
                    format!("{} round {round}: {request_case}", listing.operation);
                let answer = call(&router, listing.operation, request.clone()).await;
                assert_empty_first_page(&case, &listing, &answer);
                answers.push(answer.body);
            }
            assert_eq!(
                answers[0], answers[1],
                "case {request_case:?}: {} must answer the same empty page every time, got {:?} \
                 then {:?}",
                listing.operation, answers[0], answers[1]
            );
        }
    }

    let after = call(
        &router,
        "ListAgreementCharges",
        json!({"agreementId": holding}),
    )
    .await;
    assert_eq!(
        after.body, before.body,
        "case {holding:?}: the empty-page reads must leave the report unchanged, expected {:?}, \
         got {:?}",
        before.body, after.body
    );
}
