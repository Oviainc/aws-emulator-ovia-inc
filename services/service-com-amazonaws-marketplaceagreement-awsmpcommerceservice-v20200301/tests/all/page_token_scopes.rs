// SPDX-License-Identifier: CC0-1.0

//! Page-token scopes, end to end over HTTP.
//!
//! The rule under test is the one every listing of this crate builds its page
//! tokens from: the token a listing mints belongs to the scope of the query that
//! minted it — every parameter of the call that selects what the page indexes
//! into — so it resumes exactly that query and no other, and a call whose
//! parameters differ in any one of those parts refuses it with HTTP 400
//! `ValidationException` / `INVALID_NEXT_TOKEN`.
//!
//! The OPERATION that minted a token is one of those parts, because two
//! operations index into two different sequences even when they take the same
//! selectors: a token one paginated operation minted names no page of any other,
//! whatever their selectors happen to be. The second test drives that over every
//! paginated operation this crate serves, pairwise.
//!
//! `ListAgreementPaymentRequests` and `ListAgreementCharges` are driven here
//! because each used to build that scope its own way; both now go through the
//! single spelling in `state`, so this suite exercises that one path from both
//! sides. The rows differ along what the rule generalises over: which part of
//! the scope differs (the party, the agreement, the status), whether a part is
//! added to the query the token was minted for or removed from it, and a token
//! another listing of the same emulator minted.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_cancellation_requests::{agreement as cancellable, cancellation_over};
use crate::agreement_lifecycle::{
    agreement_over, agreement_type_filter, call, emulator, invoiced_agreement, is_next_token, term,
    Answer,
};
use crate::billing_adjustments::{entry, submit};
use crate::payment_requests::{agreement_with_variable_term, sent};
use crate::purchase_order_updates::agreement_holding;

/// The `agreementType` every agreement this service mints carries, and the one
/// catalog AWS Marketplace exposes — both admitted selector values, spelled out
/// here so a row that adds one to a query is accepted on its merits rather than
/// refused for naming something outside the set.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";
/// See [`AGREEMENT_TYPE`].
const CATALOG: &str = "AWSMarketplace";

/// A `PaymentRequestStatus` value no request stored here carries
/// (`smithy_query(query="shape", shape="PaymentRequestStatus")`), so a row can
/// name a status the token's own query did not.
const APPROVED: &str = "APPROVED";

/// One listing to drive: the operation, the member each of its items carries its
/// id in, the query a token is minted for, and the queries that must refuse that
/// token because their scope differs from it in one part.
struct Listing {
    case: &'static str,
    operation: &'static str,
    id_member: &'static str,
    base: Value,
    neighbours: Vec<(String, Value)>,
}

/// The listing for `request`, asserted to have succeeded.
async fn reported(router: &MultiServiceRouter, operation: &str, request: Value) -> Answer {
    let answer = call(router, operation, request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {operation} {request}: the listing must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// The ids one page reported, in its order.
fn ids(answer: &Answer, id_member: &str) -> Vec<String> {
    answer.body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing carries an items list, got {:?}", answer.body))
        .iter()
        .map(|item| item[id_member].as_str().unwrap_or_default().to_string())
        .collect()
}

/// `operation` must refuse `token` for the query `base` names.
async fn refuses(
    router: &MultiServiceRouter,
    operation: &str,
    base: &Value,
    token: &str,
    case: &str,
) {
    let mut body = base.clone();
    body["nextToken"] = json!(token);
    let answer = call(router, operation, body.clone()).await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: {operation} {body} must refuse a token minted for another query, \
         got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: the refusal is a ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.reason(),
        "INVALID_NEXT_TOKEN",
        "case {case:?}: the reason names the token, got {:?}",
        answer.body
    );
}

/// A token resumes the query whose scope it was minted for, and every query
/// differing from it in one part of that scope refuses it.
#[tokio::test]
async fn a_page_token_is_bound_to_every_parameter_of_the_query_that_minted_it() {
    let router = emulator();

    // Payment requests, over two agreements so a row can name a different one.
    let alpha = agreement_with_variable_term(&router, "ap-scopealpha", "vpt-alpha").await;
    let beta = agreement_with_variable_term(&router, "ap-scopebeta", "vpt-beta").await;
    for index in 0..2 {
        sent(
            &router,
            &alpha,
            "vpt-alpha",
            &format!("Alpha payment {index}"),
            &format!("{}.25", index + 1),
        )
        .await;
    }
    sent(&router, &beta, "vpt-beta", "Beta payment", "3.50").await;

    // Charges, over two agreements for the same reason.
    let (charged, _) = agreement_holding(&router, "ap-scopecharged", 3).await;
    let (other_charged, _) = agreement_holding(&router, "ap-scopeother", 2).await;

    let listings = vec![
        Listing {
            case: "ListAgreementPaymentRequests over every request of the acceptor",
            operation: "ListAgreementPaymentRequests",
            id_member: "paymentRequestId",
            base: json!({"partyType": "Acceptor"}),
            neighbours: vec![
                (
                    "the other party type".to_string(),
                    json!({"partyType": "Proposer"}),
                ),
                (
                    "one agreement instead of all".to_string(),
                    json!({"partyType": "Acceptor", "agreementId": beta}),
                ),
                (
                    "a status the token's query did not name".to_string(),
                    json!({"partyType": "Acceptor", "status": APPROVED}),
                ),
                (
                    "one more selector: the agreement type".to_string(),
                    json!({"partyType": "Acceptor", "agreementType": AGREEMENT_TYPE}),
                ),
                (
                    "one more selector: the catalog".to_string(),
                    json!({"partyType": "Acceptor", "catalog": CATALOG}),
                ),
            ],
        },
        Listing {
            case: "ListAgreementCharges over one agreement",
            operation: "ListAgreementCharges",
            id_member: "id",
            base: json!({"agreementId": charged}),
            neighbours: vec![
                (
                    "a different agreement".to_string(),
                    json!({"agreementId": other_charged}),
                ),
                (
                    "the selector removed: every agreement".to_string(),
                    json!({}),
                ),
                (
                    "one more selector: the agreement type".to_string(),
                    json!({"agreementId": charged, "agreementType": AGREEMENT_TYPE}),
                ),
                (
                    "one more selector: the catalog".to_string(),
                    json!({"agreementId": charged, "catalog": CATALOG}),
                ),
            ],
        },
    ];

    // Each listing mints a token over its own query and resumes that query with
    // it: the second item of the list it already reported the first of.
    let mut minted: Vec<(&str, String)> = Vec::new();
    for listing in &listings {
        let case = listing.case;
        let full = ids(
            &reported(&router, listing.operation, listing.base.clone()).await,
            listing.id_member,
        );
        assert!(
            full.len() >= 2,
            "case {case:?}: a bounded page leaves a token only while items remain, \
             so the store must hold at least two, got {full:?}"
        );

        let mut bounded = listing.base.clone();
        bounded["maxResults"] = json!(1);
        let first = reported(&router, listing.operation, bounded.clone()).await;
        assert_eq!(
            ids(&first, listing.id_member),
            full[..1].to_vec(),
            "case {case:?}: expected the first item {:?}, got {:?}",
            full[..1].to_vec(),
            first.body
        );
        let token = first.body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a bounded page over more items leaves a token, got {:?}",
                    first.body
                )
            })
            .to_string();
        assert!(
            is_next_token(&token),
            "case {case:?}: the token {token:?} must satisfy NextToken's pattern and bound"
        );

        let mut resumed_body = bounded.clone();
        resumed_body["nextToken"] = json!(token.clone());
        let resumed = reported(&router, listing.operation, resumed_body).await;
        assert_eq!(
            ids(&resumed, listing.id_member),
            full[1..2].to_vec(),
            "case {case:?}: expected the token to resume at {:?}, got {:?}",
            full[1..2].to_vec(),
            resumed.body
        );

        minted.push((case, token));
    }

    // No neighbouring query takes it, and neither listing takes the other's.
    for (listing, (_, token)) in listings.iter().zip(minted.iter()) {
        for (difference, body) in &listing.neighbours {
            let case = format!("{}: {difference}", listing.case);
            refuses(&router, listing.operation, body, token, &case).await;
        }
        for (other_case, foreign) in &minted {
            if foreign == token {
                continue;
            }
            let case = format!("{}: a token minted by {other_case:?}", listing.case);
            refuses(&router, listing.operation, &listing.base, foreign, &case).await;
        }
    }
}

/// One paginated operation of this crate, to drive against every other one: the
/// operation, the output member its page of items arrives in — the `items`
/// member of its `@paginated` trait,
/// `smithy_query(query="operation", operation="<Operation>")` — and a query it
/// serves at least two items of.
struct Paginated {
    operation: &'static str,
    items_member: &'static str,
    query: Value,
}

/// The page `answer` carries, as the items of the member it reports them in.
fn items_of(answer: &Answer, items_member: &str) -> Vec<Value> {
    answer.body[items_member]
        .as_array()
        .unwrap_or_else(|| {
            panic!("a page arrives in {items_member}, got {:?}", answer.body)
        })
        .clone()
}

/// A token one paginated operation minted names no page of any OTHER paginated
/// operation, whatever selectors the two calls happen to carry — and it still
/// resumes the query that minted it.
///
/// The rows span every paginated operation this crate serves except
/// `ListAgreementInvoiceLineItems`, whose groups are one per invoice of one
/// agreement and so never make a second page here (its own suite pages it); they
/// differ along what the rule generalises over: the kind of record paginated,
/// how many selectors the query names (none, one, a party), and whether the
/// pagination belongs to a listing or to a read of one agreement.
#[tokio::test]
async fn a_token_one_paginated_operation_minted_names_no_page_of_another() {
    let router = emulator();

    // Two payment requests under one agreement, and two cancellation requests
    // over two more — the pair whose scopes used to coincide.
    let paying = agreement_with_variable_term(&router, "ap-crosspay", "vpt-cross").await;
    for index in 0..2 {
        sent(
            &router,
            &paying,
            "vpt-cross",
            &format!("Cross payment {index}"),
            &format!("{}.75", index + 1),
        )
        .await;
    }
    let first_cancelled = cancellable(&router, "ap-crosscancelone").await;
    let second_cancelled = cancellable(&router, "ap-crosscanceltwo").await;
    cancellation_over(&router, &first_cancelled, "OTHER").await;
    cancellation_over(&router, &second_cancelled, "BUYER_DISSATISFACTION").await;

    // An agreement holding several charges, one carrying two accepted terms (so
    // both its terms and its entitlements page), and an invoiced one carrying
    // two billing adjustment requests.
    let (charged, _) = agreement_holding(&router, "ap-crosscharged", 3).await;
    let termed = agreement_over(
        &router,
        "ap-crosstermed",
        vec![term("eula.v1"), term("SupportTerm")],
    )
    .await;
    let (invoiced, invoice) = invoiced_agreement(&router, "ap-crossinvoiced").await;
    let adjusted = submit(
        &router,
        vec![
            entry(&invoiced, &invoice, "1", "tok-cross-one"),
            entry(&invoiced, &invoice, "2", "tok-cross-two"),
        ],
    )
    .await;
    assert_eq!(
        adjusted.status, 200,
        "submitting two billing adjustment requests must succeed, got {} with {:?}",
        adjusted.status, adjusted.body
    );

    let paginated = vec![
        Paginated {
            operation: "ListAgreementPaymentRequests",
            items_member: "items",
            query: json!({"partyType": "Acceptor"}),
        },
        Paginated {
            operation: "ListAgreementCancellationRequests",
            items_member: "items",
            query: json!({"partyType": "Acceptor"}),
        },
        Paginated {
            operation: "ListAgreementCharges",
            items_member: "items",
            query: json!({"agreementId": charged}),
        },
        Paginated {
            operation: "ListBillingAdjustmentRequests",
            items_member: "items",
            query: json!({}),
        },
        Paginated {
            operation: "SearchAgreements",
            items_member: "agreementViewSummaries",
            query: json!({"filters": [agreement_type_filter()]}),
        },
        Paginated {
            operation: "GetAgreementTerms",
            items_member: "acceptedTerms",
            query: json!({"agreementId": termed}),
        },
        Paginated {
            operation: "GetAgreementEntitlements",
            items_member: "agreementEntitlements",
            query: json!({"agreementId": termed}),
        },
    ];

    // Each operation mints a token over its own query, with its first item in
    // hand so the resume below can be told from a repeat of that page.
    let mut minted: Vec<(String, Value)> = Vec::new();
    for listing in &paginated {
        let case = listing.operation;
        let full = items_of(
            &reported(&router, listing.operation, listing.query.clone()).await,
            listing.items_member,
        );
        assert!(
            full.len() >= 2,
            "case {case}: a bounded page leaves a token only while items remain, so this \
             query must serve at least two, got {full:?}"
        );

        let mut bounded = listing.query.clone();
        bounded["maxResults"] = json!(1);
        let first = reported(&router, listing.operation, bounded).await;
        let page = items_of(&first, listing.items_member);
        assert_eq!(
            page.len(),
            1,
            "case {case}: expected a page of one, got {:?}",
            first.body
        );
        let token = first.body["nextToken"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: a bounded page over more items leaves a token, got {:?}",
                    first.body
                )
            })
            .to_string();
        assert!(
            is_next_token(&token),
            "case {case}: the token {token:?} must satisfy NextToken's pattern and bound"
        );
        minted.push((token, page[0].clone()));
    }

    for (minter, (token, first_item)) in paginated.iter().zip(minted.iter()) {
        // No OTHER paginated operation takes it, however its selectors line up
        // with the minting query's.
        for receiver in &paginated {
            if receiver.operation == minter.operation {
                continue;
            }
            let case = format!(
                "a token {} minted, offered to {} {}",
                minter.operation, receiver.operation, receiver.query
            );
            refuses(&router, receiver.operation, &receiver.query, token, &case).await;
        }

        // The operation that minted it still resumes with it, past the item its
        // first page reported.
        let case = minter.operation;
        let mut resumed_body = minter.query.clone();
        resumed_body["maxResults"] = json!(1);
        resumed_body["nextToken"] = json!(token);
        let resumed = reported(&router, minter.operation, resumed_body).await;
        let page = items_of(&resumed, minter.items_member);
        assert_eq!(
            page.len(),
            1,
            "case {case}: the token resumes a page of one, got {:?}",
            resumed.body
        );
        assert_ne!(
            page.first(),
            Some(first_item),
            "case {case}: the token resumes PAST the item the first page reported, got {:?}",
            resumed.body
        );
    }
}
