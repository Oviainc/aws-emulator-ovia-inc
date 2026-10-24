// SPDX-License-Identifier: CC0-1.0

//! Agreement cancellation requests, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: `ListAgreementCancellationRequests` reports the stored
//! cancellation requests the caller is the named party of — the ones it
//! submitted as the seller, and the ones whose agreement it accepted as the
//! buyer — selected by
//! every optional filter the request names against the member of the STORED
//! record it names, ordered and paged so each request is reported once — and a
//! `partyType` that names neither documented party is refused with a 400
//! `ValidationException`. Nothing in an answer is echoed from the listing call's
//! own input: every member comes from what `SendAgreementCancellationRequest`
//! persisted, or from the agreement that request belongs to.

use serde_json::{json, Value};
use service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::state::AGREEMENT_CANCELLATION_REQUEST_QUOTA as QUOTA;
use smithy_server_runtime::MultiServiceRouter;

use super::agreement_lifecycle::{
    agreement_over, cancel, call, emulator, is_next_token, term, Answer,
};

/// `AgreementCancellationRequestReasonCode`'s values, from
/// `smithy_query(query="shape", shape="AgreementCancellationRequestReasonCode")`
/// (also listed in the operation header of
/// `src/ops/send_agreement_cancellation_request.rs`).
///
/// `pub(crate)` because the approval suite beside this one spans the same class
/// of reason codes rather than keeping a second copy of the catalog.
pub(crate) const REASON_CODES: &[&str] = &[
    "ALTERNATIVE_PROCUREMENT_CHANNEL",
    "BUYER_DISSATISFACTION",
    "INCORRECT_TERMS_ACCEPTED",
    "OTHER",
    "PRODUCT_DISCONTINUED",
    "REPLACING_AGREEMENT",
    "TEST_AGREEMENT",
    "UNINTENDED_RENEWAL",
];

/// The `agreementType` and `catalog` an agreement of this service carries, spelled
/// out here rather than imported so a summary that reports something else fails
/// this test.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";
/// See [`AGREEMENT_TYPE`].
const CATALOG: &str = "AWSMarketplace";

/// `AgreementCancellationRequestStatus`'s documented initial value: "The
/// cancellation request is created in PENDING_APPROVAL status".
pub(crate) const PENDING_APPROVAL: &str = "PENDING_APPROVAL";

/// `AgreementCancellationRequestId`'s `^acr-[a-zA-Z0-9]+$`, checked here rather
/// than through the crate's own code so a wrong minter cannot make this test
/// pass.
pub(crate) fn is_cancellation_request_id(value: &str) -> bool {
    value
        .strip_prefix("acr-")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

pub(crate) async fn send(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "SendAgreementCancellationRequest", body).await
}

pub(crate) async fn list(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "ListAgreementCancellationRequests", body).await
}

/// Send one cancellation request over `agreement_id` and answer its id.
pub(crate) async fn cancellation_over(
    router: &MultiServiceRouter,
    agreement_id: &str,
    reason_code: &str,
) -> String {
    let sent = send(
        router,
        json!({"agreementId": agreement_id, "reasonCode": reason_code}),
    )
    .await;
    assert_eq!(
        sent.status, 200,
        "sending a cancellation request for {agreement_id} must succeed, got {} with {:?}",
        sent.status, sent.body
    );
    sent.str_at("/agreementCancellationRequestId")
}

/// The `agreementCancellationRequestId` of every item a listing answered, in the
/// order it answered them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing answers an items array, got {:?}", answer.body))
        .iter()
        .map(|item| {
            item["agreementCancellationRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// One agreement, created and accepted, with no cancellation request yet.
///
/// `proposal` is an `AgreementProposalIdentifier`, whose pattern is
/// `^(at-|ap-)[A-Za-z0-9]+$`.
pub(crate) async fn agreement(router: &MultiServiceRouter, proposal: &str) -> String {
    agreement_over(router, proposal, vec![term("eula.v1")]).await
}

// ---------------------------------------------------------------------------
// The round trip
// ---------------------------------------------------------------------------

/// What a stored cancellation request is reported as: the create's own answer and
/// the listing's summary are both computed from the one record, so they agree —
/// and the two members the create was never given (`agreementType`, `catalog`)
/// come from the agreement the request belongs to.
///
/// The rows differ along what the rule generalises over: the reason code (every
/// value of `AgreementCancellationRequestReasonCode`, so an entry nothing else
/// cites is covered too) and whether the optional description was supplied.
#[tokio::test]
async fn a_cancellation_request_is_reported_from_the_record_it_created() {
    let router = emulator();

    for (index, reason_code) in REASON_CODES.iter().enumerate() {
        // Every other row supplies the optional description.
        let description = (index % 2 == 0).then(|| format!("row {index} cancels for a reason"));
        let agreement_id = agreement(&router, &format!("ap-offer{index}")).await;

        let mut request = json!({"agreementId": agreement_id, "reasonCode": reason_code});
        if let Some(description) = description.as_deref() {
            request["description"] = json!(description);
        }
        let sent = send(&router, request).await;
        let case = format!("reasonCode {reason_code}, description {description:?}");
        assert_eq!(
            sent.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            sent.status, sent.body
        );

        let id = sent.str_at("/agreementCancellationRequestId");
        assert!(
            is_cancellation_request_id(&id),
            "case {case}: expected an ^acr-[a-zA-Z0-9]+$ id, got {id:?}"
        );
        assert_eq!(
            sent.str_at("/agreementId"),
            agreement_id,
            "case {case}: the answer must name the agreement it was sent for"
        );
        assert_eq!(
            sent.str_at("/status"),
            PENDING_APPROVAL,
            "case {case}: a new cancellation request is created in {PENDING_APPROVAL}"
        );
        assert_eq!(
            sent.str_at("/reasonCode"),
            *reason_code,
            "case {case}: the stored reason code must be the one that was sent"
        );
        assert_eq!(
            sent.body.get("description").and_then(Value::as_str),
            description.as_deref(),
            "case {case}: the description must be reported exactly when one was sent"
        );
        let created_at = sent.body["createdAt"].as_f64();
        let updated_at = sent.body["updatedAt"].as_f64();
        assert!(
            created_at.is_some() && created_at == updated_at,
            "case {case}: a request nothing has changed yet carries equal \
             createdAt/updatedAt, got {created_at:?} and {updated_at:?}"
        );

        // The listing reports the same record, with the agreement's own type and
        // catalog — neither of which the create call carried.
        let listed = list(
            &router,
            json!({"partyType": "Acceptor", "agreementId": agreement_id}),
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected 200 from the listing, got {} with {:?}",
            listed.status, listed.body
        );
        let items = listed.body["items"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case}: the agreement has exactly one cancellation request, got {items:?}"
        );
        let item = &items[0];
        assert_eq!(
            item["agreementCancellationRequestId"].as_str(),
            Some(id.as_str()),
            "case {case}: the summary must name the stored request, got {item:?}"
        );
        assert_eq!(
            item["status"].as_str(),
            Some(PENDING_APPROVAL),
            "case {case}: the summary reports the stored status, got {item:?}"
        );
        assert_eq!(
            item["reasonCode"].as_str(),
            Some(*reason_code),
            "case {case}: the summary reports the stored reason code, got {item:?}"
        );
        assert_eq!(
            item["agreementType"].as_str(),
            Some(AGREEMENT_TYPE),
            "case {case}: the summary reports the agreement's type, got {item:?}"
        );
        assert_eq!(
            item["catalog"].as_str(),
            Some(CATALOG),
            "case {case}: the summary reports the agreement's catalog, got {item:?}"
        );
        assert_eq!(
            item["createdAt"].as_f64(),
            created_at,
            "case {case}: the summary reports the stored createdAt, got {item:?}"
        );
        assert_eq!(
            item["updatedAt"].as_f64(),
            updated_at,
            "case {case}: the summary reports the stored updatedAt, got {item:?}"
        );
        assert!(
            listed.body.get("nextToken").is_none_or(Value::is_null),
            "case {case}: a listing with nothing left carries no nextToken, got {:?}",
            listed.body
        );
    }
}

/// A replayed `clientToken` reports the first cancellation request rather than
/// asking the buyer to review a second: the listing still holds exactly one.
#[tokio::test]
async fn a_replayed_client_token_sends_one_cancellation_request() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-idempotent").await;
    let request = json!({
        "agreementId": agreement_id,
        "reasonCode": "TEST_AGREEMENT",
        "clientToken": "token-1",
    });

    let first = send(&router, request.clone()).await;
    let second = send(&router, request).await;
    assert_eq!(
        (first.status, second.status),
        (200, 200),
        "both sends must succeed, got {:?} and {:?}",
        first.body,
        second.body
    );
    assert_eq!(
        first.str_at("/agreementCancellationRequestId"),
        second.str_at("/agreementCancellationRequestId"),
        "the same clientToken names the same cancellation request"
    );
    assert_eq!(
        first.body["createdAt"].as_f64(),
        second.body["createdAt"].as_f64(),
        "the replay reports the first request's creation time"
    );

    let listed = list(&router, json!({"partyType": "Acceptor"})).await;
    assert_eq!(
        listed_ids(&listed),
        vec![first.str_at("/agreementCancellationRequestId")],
        "a replayed token must not have stored a second request"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// One row of the filter table: the listing request, and which of the stored
/// cancellation requests it selects.
struct FilterCase {
    case: &'static str,
    /// The input members beyond `partyType`.
    filters: Value,
    /// Indices into the fixture's cancellation requests, in creation order.
    selects: Vec<usize>,
}

/// Each optional input member filters on the member of the STORED record it
/// names, and several of them are an AND.
///
/// The rows differ along which member is filtered, whether the value is one the
/// store holds or one it does not, and how many members are combined. The member
/// names and their meanings are
/// `ListAgreementCancellationRequests`'s input, from
/// `smithy_query(query="operation", operation="ListAgreementCancellationRequests")`.
#[tokio::test]
async fn every_optional_filter_selects_on_the_stored_member() {
    let router = emulator();
    // Two agreements, three cancellation requests: the first agreement carries
    // two of them, so a filter on it selects a subset rather than everything.
    let first_agreement = agreement(&router, "ap-filters1").await;
    let second_agreement = agreement(&router, "ap-filters2").await;
    let stored = [
        cancellation_over(&router, &first_agreement, "OTHER").await,
        cancellation_over(&router, &second_agreement, "BUYER_DISSATISFACTION").await,
        cancellation_over(&router, &first_agreement, "PRODUCT_DISCONTINUED").await,
    ];

    let cases = vec![
        FilterCase {
            case: "no filter selects every request the caller is a party to",
            filters: json!({}),
            selects: vec![0, 1, 2],
        },
        FilterCase {
            case: "agreementId selects the requests of that agreement",
            filters: json!({"agreementId": first_agreement}),
            selects: vec![0, 2],
        },
        FilterCase {
            case: "agreementId selects the single request of the other agreement",
            filters: json!({"agreementId": second_agreement}),
            selects: vec![1],
        },
        FilterCase {
            case: "an agreementId the store holds no request for selects nothing",
            filters: json!({"agreementId": "agmt-does-not-exist"}),
            selects: vec![],
        },
        FilterCase {
            case: "the status every stored request carries selects all of them",
            filters: json!({"status": PENDING_APPROVAL}),
            selects: vec![0, 1, 2],
        },
        FilterCase {
            case: "a status no stored request carries selects nothing",
            filters: json!({"status": "APPROVED"}),
            selects: vec![],
        },
        FilterCase {
            case: "another status no stored request carries selects nothing",
            filters: json!({"status": "VALIDATION_FAILED"}),
            selects: vec![],
        },
        FilterCase {
            case: "the agreement type of the stored agreements selects all of them",
            filters: json!({"agreementType": AGREEMENT_TYPE}),
            selects: vec![0, 1, 2],
        },
        FilterCase {
            case: "another agreement type selects nothing",
            filters: json!({"agreementType": "VendorInsightsAgreement"}),
            selects: vec![],
        },
        FilterCase {
            case: "the catalog the agreements were created in selects all of them",
            filters: json!({"catalog": CATALOG}),
            selects: vec![0, 1, 2],
        },
        FilterCase {
            case: "another catalog selects nothing",
            filters: json!({"catalog": "AWSMarketplace.Other"}),
            selects: vec![],
        },
        FilterCase {
            case: "agreementId and status together are an AND that holds",
            filters: json!({"agreementId": first_agreement, "status": PENDING_APPROVAL}),
            selects: vec![0, 2],
        },
        FilterCase {
            case: "agreementId and status together are an AND that does not hold",
            filters: json!({"agreementId": first_agreement, "status": "REJECTED"}),
            selects: vec![],
        },
        FilterCase {
            case: "every filter at once",
            filters: json!({
                "agreementId": second_agreement,
                "status": PENDING_APPROVAL,
                "agreementType": AGREEMENT_TYPE,
                "catalog": CATALOG,
            }),
            selects: vec![1],
        },
    ];

    for FilterCase {
        case,
        filters,
        selects,
    } in cases
    {
        let mut request = json!({"partyType": "Acceptor"});
        for (name, value) in filters.as_object().expect("the filters are an object") {
            request[name] = value.clone();
        }
        let answer = list(&router, request.clone()).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200 for {request}, got {} with {:?}",
            answer.status, answer.body
        );
        let expected: Vec<String> = selects.iter().map(|index| stored[*index].clone()).collect();
        assert_eq!(
            listed_ids(&answer),
            expected,
            "case {case:?}: {request} selected the wrong requests"
        );
    }
}

/// A party type selects by the account the record names for that side: the
/// caller accepted the agreement, so it is the acceptor of its cancellation
/// requests, and it SENT the request, so it is their proposer (seller) too —
/// `SendAgreementCancellationRequest` is the seller's own call. Both views of
/// the one record therefore report it.
///
/// The seller's view is covered across a population of inputs in
/// `seller_submitted_request_views`.
#[tokio::test]
async fn a_party_type_selects_the_requests_the_caller_is_that_party_of() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-party").await;
    let id = cancellation_over(&router, &agreement_id, "OTHER").await;

    let as_acceptor = list(&router, json!({"partyType": "Acceptor"})).await;
    assert_eq!(
        listed_ids(&as_acceptor),
        vec![id.clone()],
        "the caller accepted the agreement, so it is the acceptor of its cancellation requests"
    );

    let as_proposer = list(&router, json!({"partyType": "Proposer"})).await;
    assert_eq!(
        as_proposer.status, 200,
        "a listing as the proposer is a valid request, got {} with {:?}",
        as_proposer.status, as_proposer.body
    );
    assert_eq!(
        listed_ids(&as_proposer),
        vec![id],
        "the caller sent the cancellation request, so it is the seller (proposer) that \
         submitted it and its own listing reports it, got {:?}",
        as_proposer.body
    );
}

/// A `partyType` that names neither documented party is refused with a 400
/// `ValidationException` whose `reason` is `INVALID_PARTY_TYPE`, before the store
/// is read.
///
/// The rows differ along how the value misses: a different word, the right word
/// in the wrong case, the plain-language name of the party. `PartyType` is a
/// pattern-constrained string (`^[A-Za-z]+$`), not an enum, so every row reaches
/// the handler rather than the boundary validator.
#[tokio::test]
async fn a_party_type_that_names_no_party_is_refused() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-badparty").await;
    cancellation_over(&router, &agreement_id, "OTHER").await;

    for party_type in [
        "Seller",
        "Buyer",
        "proposer",
        "ACCEPTOR",
        "Both",
        "Partyrtype",
    ] {
        let answer = list(&router, json!({"partyType": party_type})).await;
        assert_eq!(
            answer.status, 400,
            "case {party_type:?}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {party_type:?}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_PARTY_TYPE",
            "case {party_type:?}: expected the INVALID_PARTY_TYPE reason, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.str_at("/fields/0/name"),
            "partyType",
            "case {party_type:?}: the refusal names the member it is about, got {:?}",
            answer.body
        );
        assert!(
            answer.body.get("items").is_none(),
            "case {party_type:?}: a refused listing reports no items, got {:?}",
            answer.body
        );
    }

    for party_type in ["Proposer", "Acceptor"] {
        let answer = list(&router, json!({"partyType": party_type})).await;
        assert_eq!(
            answer.status, 200,
            "case {party_type:?}: a documented party type is accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `maxResults` bounds every page and the `nextToken` resumes the same listing,
/// so paging reports each stored request exactly once, in creation order.
///
/// The page sizes span `MaxResults`'s `@range(min: 1, max: 50)`: both published
/// bounds, sizes below and at the number of stored requests, and one above it.
#[tokio::test]
async fn paging_the_cancellation_requests_yields_each_of_them_once() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-paged").await;
    let mut stored = Vec::new();
    for reason_code in REASON_CODES {
        stored.push(cancellation_over(&router, &agreement_id, reason_code).await);
    }

    for page_size in [1, 2, 3, stored.len(), stored.len() + 1, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"partyType": "Acceptor", "maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let answer = list(&router, request.clone()).await;
            assert_eq!(
                answer.status, 200,
                "case page size {page_size}: expected 200 for {request}, got {} with {:?}",
                answer.status, answer.body
            );
            let page = listed_ids(&answer);
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
            "case page size {page_size}: paging reports every stored request once, in \
             creation order"
        );
        let expected_pages = stored.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case page size {page_size}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// A `maxResults` outside `MaxResults`'s published range is refused, and each
/// bound itself is accepted.
#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let router = emulator();

    for max_results in [1, 50] {
        let request = json!({"partyType": "Acceptor", "maxResults": max_results});
        let answer = list(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults {max_results}: a published bound is accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }

    for max_results in [0, -1, 51] {
        let request = json!({"partyType": "Acceptor", "maxResults": max_results});
        let answer = list(&router, request).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {max_results}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case maxResults {max_results}: expected a ValidationException, got {:?}",
            answer.body
        );
    }
}

/// A `nextToken` this listing never issued is refused rather than served: a token
/// is bound to the query it was minted for, so it cannot resume a different one.
#[tokio::test]
async fn a_token_from_another_listing_is_refused() {
    let router = emulator();
    let first_agreement = agreement(&router, "ap-token1").await;
    let second_agreement = agreement(&router, "ap-token2").await;
    cancellation_over(&router, &first_agreement, "OTHER").await;
    cancellation_over(&router, &second_agreement, "OTHER").await;

    let page = list(&router, json!({"partyType": "Acceptor", "maxResults": 1})).await;
    let token = page.str_at("/nextToken");
    assert!(
        is_next_token(&token),
        "the first page of two requests must carry a token, got {:?}",
        page.body
    );

    // The same token, offered to a listing that selects a different sequence.
    let cases = vec![
        json!({
            "partyType": "Acceptor",
            "maxResults": 1,
            "agreementId": first_agreement,
            "nextToken": token,
        }),
        json!({"partyType": "Proposer", "maxResults": 1, "nextToken": token}),
        json!({"partyType": "Acceptor", "maxResults": 1, "nextToken": "not-a-token"}),
    ];
    for request in cases {
        let answer = list(&router, request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {request}: expected 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_NEXT_TOKEN",
            "case {request}: expected the INVALID_NEXT_TOKEN reason, got {:?}",
            answer.body
        );
    }

    // The token the listing DID issue resumes it.
    let resumed = list(
        &router,
        json!({"partyType": "Acceptor", "maxResults": 1, "nextToken": token}),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "the token this listing issued resumes it, got {} with {:?}",
        resumed.status, resumed.body
    );
    assert_eq!(
        listed_ids(&resumed).len(),
        1,
        "the second page carries the second request, got {:?}",
        resumed.body
    );
}

// ---------------------------------------------------------------------------
// What a cancellation request cannot be sent for
// ---------------------------------------------------------------------------

/// The store stops growing at the emulator's own ceiling: past it a send is
/// refused with the modelled `ThrottlingException` and stores nothing, while a
/// retried `clientToken` is still answered from what is already stored.
#[tokio::test]
async fn the_store_stops_growing_at_its_ceiling() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-ceiling").await;

    // Fill the store to its ceiling, the last record under a client token so the
    // replay path can be exercised once it is full.
    for index in 0..QUOTA {
        let mut request = json!({"agreementId": agreement_id, "reasonCode": "OTHER"});
        if index + 1 == QUOTA {
            request["clientToken"] = json!("token-at-the-ceiling");
        }
        let sent = send(&router, request).await;
        assert!(
            sent.status == 200,
            "case record {index}: expected 200 below the ceiling of {QUOTA}, got {} with {:?}",
            sent.status,
            sent.body
        );
    }

    for case in ["a send with no token", "a send with a fresh token"] {
        let mut request = json!({"agreementId": agreement_id, "reasonCode": "OTHER"});
        if case.contains("fresh") {
            request["clientToken"] = json!("token-past-the-ceiling");
        }
        let refused = send(&router, request).await;
        assert_eq!(
            refused.status, 429,
            "case {case:?}: expected 429 at the ceiling, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ThrottlingException",
            "case {case:?}: expected a ThrottlingException, got {:?}",
            refused.body
        );
    }

    // The ceiling refuses growth, not reads: the token the last send carried
    // still names its own record.
    let replayed = send(
        &router,
        json!({
            "agreementId": agreement_id,
            "reasonCode": "OTHER",
            "clientToken": "token-at-the-ceiling",
        }),
    )
    .await;
    assert_eq!(
        replayed.status, 200,
        "case a replayed token at the ceiling: expected 200, got {} with {:?}",
        replayed.status, replayed.body
    );
    assert!(
        is_cancellation_request_id(&replayed.str_at("/agreementCancellationRequestId")),
        "case a replayed token at the ceiling: expected the stored request, got {:?}",
        replayed.body
    );

    // Paging the whole store reports exactly the records it holds — the refused
    // sends added none.
    let mut counted = 0;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"partyType": "Acceptor", "maxResults": 50});
        if let Some(token) = token.as_deref() {
            request["nextToken"] = json!(token);
        }
        let page = list(&router, request).await;
        counted += listed_ids(&page).len();
        match page.body.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert_eq!(
        counted, QUOTA,
        "the store holds exactly its ceiling of records, got {counted}"
    );
}

/// Sending a cancellation request for an agreement the store does not hold is a
/// not-found refusal, and for one that is no longer active a validation one —
/// both HTTP 400, as this service answers every client error of these classes
/// (`crate::state::NOT_FOUND_STATUS`) — and neither stores anything, so the
/// listing is unchanged.
#[tokio::test]
async fn a_cancellation_request_needs_an_active_agreement() {
    let router = emulator();
    let agreement_id = agreement(&router, "ap-inactive").await;

    let unknown = send(
        &router,
        json!({"agreementId": "agmt-nope", "reasonCode": "OTHER"}),
    )
    .await;
    assert_eq!(
        unknown.status, 400,
        "case unknown agreement: expected 400, got {} with {:?}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "ResourceNotFoundException",
        "case unknown agreement: expected a ResourceNotFoundException, got {:?}",
        unknown.body
    );

    let cancelled = cancel(&router, &agreement_id).await;
    assert_eq!(
        cancelled.status, 200,
        "cancelling the agreement must succeed, got {} with {:?}",
        cancelled.status, cancelled.body
    );

    let inactive = send(
        &router,
        json!({"agreementId": agreement_id, "reasonCode": "OTHER"}),
    )
    .await;
    assert_eq!(
        inactive.status, 400,
        "case cancelled agreement: expected 400, got {} with {:?}",
        inactive.status, inactive.body
    );
    assert_eq!(
        inactive.reason(),
        "INACTIVE_AGREEMENT",
        "case cancelled agreement: expected the INACTIVE_AGREEMENT reason, got {:?}",
        inactive.body
    );

    let listed = list(&router, json!({"partyType": "Acceptor"})).await;
    assert_eq!(
        listed_ids(&listed),
        Vec::<String>::new(),
        "a refused send stores nothing, got {:?}",
        listed.body
    );
}
