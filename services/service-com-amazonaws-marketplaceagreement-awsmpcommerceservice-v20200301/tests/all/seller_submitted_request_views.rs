// SPDX-License-Identifier: CC0-1.0

//! The seller's (proposer's) view of the requests a seller submits, end to end
//! over HTTP.
//!
//! The rule under test: a request this service lets a SELLER submit under an
//! agreement records the submitting account as its proposer, so a listing of
//! those requests with `partyType=Proposer` selects exactly the requests this
//! account submitted, and one with `partyType=Acceptor` exactly the requests
//! whose agreement this account accepted. Both seller-submitted request kinds
//! the service models are covered here — the cancellation request
//! (`SendAgreementCancellationRequest` / `ListAgreementCancellationRequests`)
//! and the payment request (`SendAgreementPaymentRequest` /
//! `ListAgreementPaymentRequests`) — because the rule is about the kind of
//! record, not about one operation.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_cancellation_requests::{
    agreement, cancellation_over, list as list_cancellations, PENDING_APPROVAL, REASON_CODES,
};
use super::agreement_lifecycle::{call, emulator, is_next_token, Answer};
use super::payment_requests::{
    agreement_with_variable_term, list as list_payments, sent as send_payment,
};

/// The `agreementType` and `catalog` every agreement of this service carries,
/// spelled out here rather than imported so a listing that reports something
/// else fails this test.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";
/// See [`AGREEMENT_TYPE`].
const CATALOG: &str = "AWSMarketplace";

/// The two values of `PartyType`, as `ListAgreementCancellationRequestsInput$partyType`
/// documents them: "Use `Proposer` to list cancellation requests where you are
/// the seller, or `Acceptor` to list cancellation requests where you are the
/// buyer" (`smithy_query(query="shape", shape="ListAgreementCancellationRequestsInput")`).
const PARTY_TYPES: &[&str] = &["Proposer", "Acceptor"];

/// The ids one listing answered, under the member that names them, in the order
/// it answered them.
fn listed(answer: &Answer, member: &str) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "a listing must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("items is a required member, got {:?}", answer.body))
        .iter()
        .map(|item| item[member].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `agreementCancellationRequestId`s one cancellation listing answered.
fn listed_cancellations(answer: &Answer) -> Vec<String> {
    listed(answer, "agreementCancellationRequestId")
}

/// The `paymentRequestId`s one payment listing answered.
fn listed_payments(answer: &Answer) -> Vec<String> {
    listed(answer, "paymentRequestId")
}

async fn get_cancellation(
    router: &MultiServiceRouter,
    agreement_id: &str,
    request_id: &str,
) -> Answer {
    call(
        router,
        "GetAgreementCancellationRequest",
        json!({"agreementId": agreement_id, "agreementCancellationRequestId": request_id}),
    )
    .await
}

/// One case of the shape of the stored population: how many agreements carry
/// cancellation requests, and how many requests each of them carries.
struct Population {
    case: &'static str,
    /// One entry per agreement, holding the number of requests sent over it.
    requests_per_agreement: &'static [usize],
}

/// Every request the caller submitted is in its OWN (`Proposer`) listing.
///
/// The rows differ along what the rule generalises over: how many agreements
/// carry requests, how many requests each carries, and so how many requests the
/// store holds in total — including the row where nothing was submitted at all,
/// whose listing is the only empty one.
#[tokio::test]
async fn the_proposer_listing_reports_every_request_the_caller_submitted() {
    let populations = [
        Population {
            case: "nothing submitted",
            requests_per_agreement: &[],
        },
        Population {
            case: "one agreement with one request",
            requests_per_agreement: &[1],
        },
        Population {
            case: "one agreement with three requests",
            requests_per_agreement: &[3],
        },
        Population {
            case: "three agreements with one request each",
            requests_per_agreement: &[1, 1, 1],
        },
        Population {
            case: "two agreements with two requests each",
            requests_per_agreement: &[2, 2],
        },
        Population {
            case: "agreements with and without requests",
            requests_per_agreement: &[0, 2, 0, 1],
        },
    ];

    for Population {
        case,
        requests_per_agreement,
    } in populations
    {
        let router = emulator();
        let mut submitted: Vec<String> = Vec::new();
        for (index, count) in requests_per_agreement.iter().enumerate() {
            let agreement_id = agreement(&router, &format!("ap-pop{index}")).await;
            for request in 0..*count {
                // The reason code varies with the request so the population
                // spans the catalog rather than repeating one value.
                let reason_code = REASON_CODES[(index + request) % REASON_CODES.len()];
                submitted.push(cancellation_over(&router, &agreement_id, reason_code).await);
            }
        }

        // The caller SENT every stored request, so it is the proposer of all of
        // them; it also accepted every agreement they were sent under, so it is
        // their acceptor too. Both views report the same population here.
        for party_type in PARTY_TYPES {
            let answer = list_cancellations(&router, json!({"partyType": party_type})).await;
            assert_eq!(
                listed_cancellations(&answer),
                submitted,
                "case {case:?}, partyType {party_type:?}: expected the {} submitted \
                 request(s), got {:?}",
                submitted.len(),
                answer.body
            );
        }
    }
}

/// One case of the `Proposer` listing's optional selectors.
struct Selector {
    case: &'static str,
    /// The members added to `{"partyType": "Proposer"}`, as the request carries
    /// them.
    filters: Value,
    /// Which of the stored requests the listing must select, by index.
    selects: Vec<usize>,
}

/// The seller's listing applies every other selector to the requests it owns:
/// the party type decides availability, and each optional member filters what
/// is left — so a seller sees a filtered slice of its own submissions, not
/// everything and not nothing.
///
/// The rows differ along which selector is used, whether its value matches a
/// stored request, and how many selectors are combined.
#[tokio::test]
async fn the_proposer_listing_filters_the_requests_it_owns() {
    let router = emulator();
    let first_agreement = agreement(&router, "ap-sel1").await;
    let second_agreement = agreement(&router, "ap-sel2").await;
    // A third agreement nothing was submitted under, so an `agreementId` naming
    // it selects nothing — as opposed to one naming no agreement at all, which
    // is refused (`selector_resolution.rs`).
    let empty_agreement = agreement(&router, "ap-sel3").await;
    // Index 0 and 2 belong to the first agreement, index 1 to the second.
    let stored = [
        cancellation_over(&router, &first_agreement, "OTHER").await,
        cancellation_over(&router, &second_agreement, "TEST_AGREEMENT").await,
        cancellation_over(&router, &first_agreement, "PRODUCT_DISCONTINUED").await,
    ];

    let selectors = vec![
        Selector {
            case: "no selector at all",
            filters: json!({}),
            selects: vec![0, 1, 2],
        },
        Selector {
            case: "the agreement two of them were sent under",
            filters: json!({"agreementId": first_agreement}),
            selects: vec![0, 2],
        },
        Selector {
            case: "an agreement no stored request was sent under",
            filters: json!({"agreementId": empty_agreement}),
            selects: vec![],
        },
        Selector {
            case: "the status every stored request carries",
            filters: json!({"status": PENDING_APPROVAL}),
            selects: vec![0, 1, 2],
        },
        Selector {
            case: "a status no stored request carries",
            filters: json!({"status": "VALIDATION_FAILED"}),
            selects: vec![],
        },
        Selector {
            case: "the agreement type of their agreements",
            filters: json!({"agreementType": AGREEMENT_TYPE}),
            selects: vec![0, 1, 2],
        },
        Selector {
            case: "the other agreement type the member admits",
            filters: json!({"agreementType": "VendorInsightsAgreement"}),
            selects: vec![],
        },
        Selector {
            case: "the catalog they were created in",
            filters: json!({"catalog": CATALOG}),
            selects: vec![0, 1, 2],
        },
        Selector {
            case: "every selector at once",
            filters: json!({
                "agreementId": second_agreement,
                "status": PENDING_APPROVAL,
                "agreementType": AGREEMENT_TYPE,
                "catalog": CATALOG,
            }),
            selects: vec![1],
        },
    ];

    for Selector {
        case,
        filters,
        selects,
    } in selectors
    {
        let mut request = json!({"partyType": "Proposer"});
        for (name, value) in filters.as_object().expect("the filters are an object") {
            request[name] = value.clone();
        }
        let answer = list_cancellations(&router, request.clone()).await;
        let expected: Vec<String> = selects.iter().map(|index| stored[*index].clone()).collect();
        assert_eq!(
            listed_cancellations(&answer),
            expected,
            "case {case:?}: {request} selected the wrong requests, got {:?}",
            answer.body
        );
    }
}

/// The seller's listing pages over its own requests the same way the buyer's
/// does: each page is bounded by `maxResults`, the token resumes the same
/// listing, and every request it owns is reported exactly once.
///
/// The page sizes span `MaxResults`'s `@range(min: 1, max: 50)`: both published
/// bounds, sizes under and at the number of stored requests, and one above it.
#[tokio::test]
async fn the_proposer_listing_pages_over_its_own_requests() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-sellerpage").await;
    let mut stored = Vec::new();
    for reason_code in REASON_CODES {
        stored.push(cancellation_over(&router, &agreement_id, reason_code).await);
    }

    for page_size in [1, 2, stored.len(), stored.len() + 1, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"partyType": "Proposer", "maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let answer = list_cancellations(&router, request.clone()).await;
            let page = listed_cancellations(&answer);
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: a page carries at most that many items, got {page:?}"
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case page size {page_size}: paging must terminate, saw {pages} pages"
            );
            match answer.body.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        is_next_token(next),
                        "case page size {page_size}: expected a modelled nextToken, got {next:?}"
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, stored,
            "case page size {page_size}: the seller's listing reports every request it \
             submitted once, in creation order"
        );
        let expected_pages = stored.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page size {page_size}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// The rule holds for BOTH kinds of seller-submitted request this service
/// models, so the row is the kind: a cancellation request and a payment request.
///
/// Each row submits two requests of its kind and asserts that both party-type
/// views report them, so neither listing's seller view is dead.
#[tokio::test]
async fn both_kinds_of_seller_submitted_request_are_in_the_sellers_listing() {
    // Cancellation requests.
    let router = emulator();
    let agreement_id = agreement(&router, "ap-bothkinds").await;
    let cancellations = vec![
        cancellation_over(&router, &agreement_id, "OTHER").await,
        cancellation_over(&router, &agreement_id, "BUYER_DISSATISFACTION").await,
    ];
    for party_type in PARTY_TYPES {
        let answer = list_cancellations(&router, json!({"partyType": party_type})).await;
        assert_eq!(
            listed_cancellations(&answer),
            cancellations,
            "case cancellation requests, partyType {party_type:?}: expected both sent \
             requests, got {:?}",
            answer.body
        );
    }

    // Payment requests, the other seller-submitted kind.
    let router = emulator();
    let agreement_id = agreement_with_variable_term(&router, "ap-bothkinds", "vpt-bothkinds").await;
    let payments = vec![
        send_payment(&router, &agreement_id, "vpt-bothkinds", "January", "10.00").await,
        send_payment(&router, &agreement_id, "vpt-bothkinds", "February", "20.50").await,
    ];
    for party_type in PARTY_TYPES {
        let answer = list_payments(&router, json!({"partyType": party_type})).await;
        assert_eq!(
            listed_payments(&answer),
            payments,
            "case payment requests, partyType {party_type:?}: expected both sent requests, \
             got {:?}",
            answer.body
        );
    }
}

/// What the seller's listing reports is the stored record, not the listing
/// call's own input: the summary it answers agrees, member by member, with what
/// `GetAgreementCancellationRequest` reads back for the same request.
///
/// The rows are the reason codes of `AgreementCancellationRequestReasonCode`, so
/// an entry no other case cites is covered too.
#[tokio::test]
async fn the_proposer_listing_reports_the_stored_record() {
    let router = emulator();

    for (index, reason_code) in REASON_CODES.iter().enumerate() {
        let router = &router;
        let agreement_id = agreement(router, &format!("ap-record{index}")).await;
        let id = cancellation_over(router, &agreement_id, reason_code).await;

        let listing = list_cancellations(
            router,
            json!({"partyType": "Proposer", "agreementId": agreement_id}),
        )
        .await;
        let items = listing.body["items"]
            .as_array()
            .unwrap_or_else(|| panic!("items is a required member, got {:?}", listing.body))
            .clone();
        assert_eq!(
            items.len(),
            1,
            "case {reason_code:?}: the seller's listing of one agreement reports its one \
             request, got {:?}",
            listing.body
        );
        let summary = &items[0];
        let detail = get_cancellation(router, &agreement_id, &id).await;
        assert_eq!(
            detail.status, 200,
            "case {reason_code:?}: the request must read back, got {} with {:?}",
            detail.status, detail.body
        );

        // The members BOTH shapes carry (`AgreementCancellationRequestSummary`
        // and `GetAgreementCancellationRequestOutput`, from
        // `smithy_query(query="shape", shape="AgreementCancellationRequestSummary,GetAgreementCancellationRequestOutput")`):
        // `agreementType`/`catalog` are on the summary only.
        for member in [
            "agreementCancellationRequestId",
            "agreementId",
            "status",
            "reasonCode",
            "createdAt",
            "updatedAt",
        ] {
            assert_eq!(
                summary.get(member),
                detail.body.get(member),
                "case {reason_code:?}: the listed {member} must be the stored one, got {:?} \
                 against {:?}",
                summary.get(member),
                detail.body.get(member)
            );
        }
        // The two members the summary adds come from the agreement the request
        // was sent under, not from the listing call's input.
        assert_eq!(
            summary.get("agreementType"),
            Some(&json!(AGREEMENT_TYPE)),
            "case {reason_code:?}: the summary reports the agreement's own type, got {summary:?}"
        );
        assert_eq!(
            summary.get("catalog"),
            Some(&json!(CATALOG)),
            "case {reason_code:?}: the summary reports the agreement's own catalog, got {summary:?}"
        );
        assert_eq!(
            summary.get("agreementCancellationRequestId"),
            Some(&json!(id)),
            "case {reason_code:?}: the listing names the request the send minted, got {summary:?}"
        );
    }
}

/// A decision the buyer takes moves the request in the seller's view too: both
/// views read one record, so an approved request is selected by the seller's
/// listing under its new status and no longer under the old one.
#[tokio::test]
async fn a_decided_request_moves_in_the_sellers_listing() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-decided").await;
    let pending = cancellation_over(&router, &agreement_id, "OTHER").await;
    let approved = cancellation_over(&router, &agreement_id, "UNINTENDED_RENEWAL").await;

    let decision = call(
        &router,
        "AcceptAgreementCancellationRequest",
        json!({"agreementId": agreement_id, "agreementCancellationRequestId": approved}),
    )
    .await;
    assert_eq!(
        decision.status, 200,
        "accepting the cancellation request must succeed, got {} with {:?}",
        decision.status, decision.body
    );

    for (case, status, expected) in [
        ("still pending", PENDING_APPROVAL, vec![pending.clone()]),
        ("approved", "APPROVED", vec![approved.clone()]),
        ("rejected", "REJECTED", vec![]),
    ] {
        let answer =
            list_cancellations(&router, json!({"partyType": "Proposer", "status": status})).await;
        assert_eq!(
            listed_cancellations(&answer),
            expected,
            "case {case:?}: the seller's listing selects by the stored status, got {:?}",
            answer.body
        );
    }
}
