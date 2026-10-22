// SPDX-License-Identifier: CC0-1.0

//! Payment requests, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises
//! routing, deserialization, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test is the one `ListAgreementPaymentRequests` states:
//! it reports, from the payment requests the service stored, exactly those the
//! caller is the requested party of and that match every optional selector the
//! call named, in one stable order, bounded by `maxResults` with a `nextToken`
//! that resumes that same list — never a value echoed from the listing call's
//! own input. `SendAgreementPaymentRequest` is what stores them, "in
//! `PENDING_APPROVAL` status", so the two are tested as a round trip.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    agreement_over, call, cancel, emulator, is_next_token, term, variable_payment_term, Answer,
};

/// The `agreementType` every agreement this service mints carries, and the
/// catalog it is created in — spelled out here rather than imported so a
/// handler that reported something else would fail this test.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";
/// See [`AGREEMENT_TYPE`].
const CATALOG: &str = "AWSMarketplace";

/// `PaymentRequestStatus`'s values, from
/// `smithy_query(query="shape", shape="PaymentRequestStatus")`. A new payment
/// request carries the first one; `SendAgreementPaymentRequest` documents it as
/// "created in `PENDING_APPROVAL` status".
const PENDING_APPROVAL: &str = "PENDING_APPROVAL";
/// See [`PENDING_APPROVAL`]: the statuses no stored payment request carries
/// yet, because the operations that would move one there are still stubs.
const OTHER_STATUSES: &[&str] = &[
    "VALIDATING",
    "VALIDATION_FAILED",
    "APPROVED",
    "REJECTED",
    "CANCELLED",
];

/// `PaymentRequestId`'s `^pr-[a-zA-Z0-9]+$`, checked here rather than through
/// the crate's own code so a wrong minter cannot make this test pass.
fn is_payment_request_id(value: &str) -> bool {
    value
        .strip_prefix("pr-")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}

async fn send(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "SendAgreementPaymentRequest", request).await
}

async fn list(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "ListAgreementPaymentRequests", request).await
}

/// One agreement whose accepted terms carry the `VariablePaymentTerm`
/// `term_id`, which is the term a payment request is sent under.
async fn agreement_with_variable_term(
    router: &MultiServiceRouter,
    proposal: &str,
    term_id: &str,
) -> String {
    agreement_over(router, proposal, vec![variable_payment_term(term_id)]).await
}

/// Send one payment request and answer the `paymentRequestId` it minted.
async fn sent(
    router: &MultiServiceRouter,
    agreement_id: &str,
    term_id: &str,
    name: &str,
    charge_amount: &str,
) -> String {
    let answer = send(
        router,
        json!({
            "agreementId": agreement_id,
            "termId": term_id,
            "name": name,
            "chargeAmount": charge_amount,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "sending {name:?} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.str_at("/paymentRequestId")
}

/// The `items` one list answered, which the model declares required.
fn items(answer: &Answer) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "listing must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("items is a required member: {:?}", answer.body))
        .clone()
}

/// The `paymentRequestId`s of one page, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    items(answer)
        .iter()
        .map(|item| {
            item["paymentRequestId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The whole list, paged through at `max_results` per call, with the id of every
/// page's size so a caller can check the bound.
async fn paged(
    router: &MultiServiceRouter,
    request: Value,
    max_results: Option<i64>,
) -> (Vec<String>, Vec<usize>) {
    let mut collected: Vec<String> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut next: Option<String> = None;
    loop {
        let mut body = request.clone();
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        if let Some(token) = next.as_deref() {
            body["nextToken"] = json!(token);
        }
        let answer = list(router, body).await;
        let page = listed_ids(&answer);
        sizes.push(page.len());
        collected.extend(page);
        next = answer.body["nextToken"].as_str().map(str::to_string);
        if let Some(token) = next.as_deref() {
            assert!(
                is_next_token(token),
                "the nextToken {token:?} must satisfy NextToken's pattern and bound"
            );
        } else {
            break;
        }
        assert!(
            collected.len() <= 4096,
            "paging must terminate; collected {} ids",
            collected.len()
        );
    }
    (collected, sizes)
}

// ---------------------------------------------------------------------------
// SendAgreementPaymentRequest -> ListAgreementPaymentRequests
// ---------------------------------------------------------------------------

/// One row of the round-trip table: what was sent, and what the list must then
/// report back.
struct Round {
    case: &'static str,
    /// `PaymentRequestName`, `@length(min: 5, max: 64)`.
    name: &'static str,
    /// `PositiveAmountUpto8Decimals`, `^[0-9]*(\.[0-9]{0,8})?$`.
    charge_amount: &'static str,
    /// `PaymentRequestDescription`, optional in the model.
    description: Option<&'static str>,
}

/// Every member a list reports is read from the payment request that was
/// stored, for payment requests differing along what the rule generalises over:
/// the name's length, the shape of the amount (whole, fractional, the eight
/// decimals the pattern admits at most, a trailing point, a long integer part)
/// and whether a description was given.
#[tokio::test]
async fn a_listed_payment_request_is_reported_from_what_was_sent() {
    let cases = [
        Round {
            case: "the shortest name the model admits, and a whole amount",
            name: "Milst",
            charge_amount: "1",
            description: None,
        },
        Round {
            case: "a described request with a two-decimal amount",
            name: "Milestone 1 delivery",
            charge_amount: "100.50",
            description: Some("the first milestone of the statement of work"),
        },
        Round {
            case: "the eight decimals PositiveAmountUpto8Decimals admits at most",
            name: "Metered overage",
            charge_amount: "0.00000001",
            description: Some("a single hundred-millionth of a unit"),
        },
        Round {
            case: "an amount whose decimal point carries no digits",
            name: "Annual true-up",
            charge_amount: "4200.",
            description: None,
        },
        Round {
            case: "the longest name the model admits, and a long integer part",
            name: "Milestone payment request with a sixty-four character long name!",
            charge_amount: "98765432109876",
            description: Some("a large whole amount"),
        },
    ];

    let router = emulator();
    let agreement = agreement_with_variable_term(&router, "ap-roundtrip", "vpt-round").await;

    let mut expected_ids: Vec<String> = Vec::new();
    for case in &cases {
        // `PaymentRequestName` is `@length(min: 5, max: 64)`, and the first and
        // last rows sit on those bounds.
        assert!(
            (5..=64).contains(&case.name.chars().count()),
            "case {:?}: the name {:?} is outside PaymentRequestName's 5..=64",
            case.case,
            case.name
        );
        let mut body = json!({
            "agreementId": &agreement,
            "termId": "vpt-round",
            "name": case.name,
            "chargeAmount": case.charge_amount,
        });
        if let Some(description) = case.description {
            body["description"] = json!(description);
        }
        let answer = send(&router, body).await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: sending must succeed, got {} with {:?}",
            case.case, answer.status, answer.body
        );
        let id = answer.str_at("/paymentRequestId");
        assert!(
            is_payment_request_id(&id),
            "case {:?}: {id:?} must satisfy PaymentRequestId's ^pr-[a-zA-Z0-9]+$",
            case.case
        );
        assert_eq!(
            answer.str_at("/status"),
            PENDING_APPROVAL,
            "case {:?}: a payment request is created in {PENDING_APPROVAL}, got {:?}",
            case.case,
            answer.body
        );
        assert_eq!(
            answer.str_at("/agreementId"),
            agreement,
            "case {:?}: the send must report the agreement it was sent for, got {:?}",
            case.case,
            answer.body
        );
        expected_ids.push(id);
    }

    // The list reports what was stored, and reports it the same way to every
    // later caller: a report changes nothing.
    let first = list(&router, json!({"partyType": "Acceptor"})).await;
    let again = list(&router, json!({"partyType": "Acceptor"})).await;
    assert_eq!(
        listed_ids(&first),
        listed_ids(&again),
        "listing twice must report the same payment requests"
    );
    assert_eq!(
        listed_ids(&first),
        expected_ids,
        "the list must report every stored payment request, in one stable order"
    );
    assert!(
        first.body["nextToken"].is_null(),
        "a list that fits in one page carries no nextToken: {:?}",
        first.body
    );

    let listed = items(&first);
    for (case, item) in cases.iter().zip(listed.iter()) {
        let at = |member: &str| item[member].as_str().unwrap_or_default().to_string();
        assert_eq!(
            at("name"),
            case.name,
            "case {:?}: the listed name must be the one sent, got {item:?}",
            case.case
        );
        assert_eq!(
            at("chargeAmount"),
            case.charge_amount,
            "case {:?}: the listed amount must be the one sent, got {item:?}",
            case.case
        );
        assert_eq!(
            at("status"),
            PENDING_APPROVAL,
            "case {:?}: a stored payment request is {PENDING_APPROVAL}, got {item:?}",
            case.case
        );
        assert_eq!(
            at("agreementId"),
            agreement,
            "case {:?}: the listed agreement must be the one sent, got {item:?}",
            case.case
        );
        // "The unique identifier of the charge created after the payment
        // request is approved. This field is only present for approved payment
        // requests" — nothing has approved one.
        assert!(
            item["chargeId"].is_null(),
            "case {:?}: a {PENDING_APPROVAL} request has no charge, got {item:?}",
            case.case
        );
        // "The currency code for the charge amount" is the currency of the
        // accepted term (`VariablePaymentTerm$currencyCode`), which belongs to
        // the agreement proposal this service does not hold, so none is
        // reported rather than invented.
        assert!(
            item["currencyCode"].is_null(),
            "case {:?}: no currency is known for the term, got {item:?}",
            case.case
        );
        let created = item["createdAt"].as_f64();
        let updated = item["updatedAt"].as_f64();
        assert!(
            created.is_some() && created == updated,
            "case {:?}: a request nothing has changed was last updated when it \
             was created, got {item:?}",
            case.case
        );
    }

    // "Both sellers (proposers) and buyers (acceptors) can use this operation":
    // the seller is the account that submitted the request and the buyer is the
    // acceptor of the agreement — one account in this emulator, so it is both.
    let as_proposer = list(&router, json!({"partyType": "Proposer"})).await;
    assert_eq!(
        listed_ids(&as_proposer),
        expected_ids,
        "the account that sent the requests lists them as the proposer too"
    );
}

/// A retried send is the same payment request: `clientToken` is modelled
/// `@idempotencyToken`, so the second call reports the first record and the
/// list still holds one.
#[tokio::test]
async fn a_replayed_client_token_sends_one_payment_request() {
    let router = emulator();
    let agreement = agreement_with_variable_term(&router, "ap-idempotent", "vpt-once").await;
    let body = json!({
        "agreementId": &agreement,
        "termId": "vpt-once",
        "name": "Milestone paid once",
        "chargeAmount": "250.75",
        "clientToken": "token-abc-123",
    });

    let first = send(&router, body.clone()).await;
    let second = send(&router, body).await;
    assert_eq!(
        first.status, 200,
        "the first send must succeed, got {:?}",
        first.body
    );
    assert_eq!(
        second.status, 200,
        "the replayed send must succeed, got {:?}",
        second.body
    );
    assert_eq!(
        first.str_at("/paymentRequestId"),
        second.str_at("/paymentRequestId"),
        "a replayed clientToken names the first payment request"
    );
    assert_eq!(
        first.str_at("/chargeAmount"),
        second.str_at("/chargeAmount"),
        "a replayed clientToken reports the stored amount"
    );
    assert_eq!(
        listed_ids(&list(&router, json!({"partyType": "Proposer"})).await),
        vec![first.str_at("/paymentRequestId")],
        "one token sent one payment request, so the list holds exactly one"
    );

    // A token reused for DIFFERENT details is a conflict, not an answer about
    // the request it already sent — the same rule the sibling
    // `BatchCreateBillingAdjustmentRequest` applies to its own `clientToken`.
    // The rows differ along which member was changed.
    let mismatches = [
        ("a different amount", json!({"chargeAmount": "99.99"})),
        ("a different name", json!({"name": "Milestone paid twice"})),
        (
            "a description the first send did not carry",
            json!({"description": "a late addition"}),
        ),
    ];
    for (case, change) in mismatches {
        let mut body = json!({
            "agreementId": &agreement,
            "termId": "vpt-once",
            "name": "Milestone paid once",
            "chargeAmount": "250.75",
            "clientToken": "token-abc-123",
        });
        for (member, value) in change.as_object().expect("a change is a JSON object") {
            body[member] = value.clone();
        }
        let answer = send(&router, body.clone()).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: {body} reuses the token for other details, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ConflictException",
            "case {case:?}: the refusal is a ConflictException, got {:?}",
            answer.body
        );
    }
    assert_eq!(
        listed_ids(&list(&router, json!({"partyType": "Proposer"})).await),
        vec![first.str_at("/paymentRequestId")],
        "a refused replay stores nothing, so the list still holds exactly one"
    );
}

// ---------------------------------------------------------------------------
// The bound on the store
// ---------------------------------------------------------------------------

/// The store is bounded: once it holds the emulator's own maximum of payment
/// requests, a further send is refused with the operation's modelled
/// `ThrottlingException` and stores nothing, so no caller can grow this
/// service's state without end.
#[tokio::test]
async fn a_send_past_the_stores_bound_is_refused_and_stores_nothing() {
    /// `crate::state::PAYMENT_REQUEST_QUOTA`, spelled out here so a handler
    /// that stopped checking the bound would fail this test.
    const QUOTA: usize = 1_000;

    let router = emulator();
    let agreement = agreement_with_variable_term(&router, "ap-bounded", "vpt-bound").await;
    for index in 0..QUOTA {
        let answer = send(
            &router,
            json!({
                "agreementId": &agreement,
                "termId": "vpt-bound",
                "name": format!("Bounded payment {index}"),
                "chargeAmount": "1",
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "send {index} is within the bound, got {} with {:?}",
            answer.status, answer.body
        );
    }

    let refused = send(
        &router,
        json!({
            "agreementId": &agreement,
            "termId": "vpt-bound",
            "name": "One payment too many",
            "chargeAmount": "1",
        }),
    )
    .await;
    assert_eq!(
        refused.status, 429,
        "case past the bound: expected HTTP 429, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ThrottlingException",
        "case past the bound: expected ThrottlingException, got {:?}",
        refused.body
    );

    let (listed, _) = paged(&router, json!({"partyType": "Proposer"}), Some(50)).await;
    assert_eq!(
        listed.len(),
        QUOTA,
        "the refused send stored nothing, so the store still holds {QUOTA}"
    );
}

// ---------------------------------------------------------------------------
// The optional selectors
// ---------------------------------------------------------------------------

/// One row of the selector table: the optional members added to the listing
/// call, and which of the stored payment requests they select, by the index at
/// which each was sent.
struct Selector {
    case: &'static str,
    selector: Value,
    expected: &'static [usize],
}

/// Every optional parameter selects on what was stored: the agreement the
/// request names, its status, and the type and catalog of that agreement.
///
/// The rows differ along which parameter selects, whether it names a value the
/// store holds, and whether two parameters are combined — and the statuses are
/// every value of `PaymentRequestStatus`, from
/// `smithy_query(query="shape", shape="PaymentRequestStatus")`.
#[tokio::test]
async fn each_optional_parameter_selects_the_stored_payment_requests_it_names() {
    let router = emulator();
    let alpha = agreement_with_variable_term(&router, "ap-alpha", "vpt-alpha").await;
    let beta = agreement_with_variable_term(&router, "ap-beta", "vpt-beta").await;
    let ids = [
        sent(&router, &alpha, "vpt-alpha", "Alpha first", "10").await,
        sent(&router, &alpha, "vpt-alpha", "Alpha second", "20.5").await,
        sent(&router, &beta, "vpt-beta", "Beta only", "0.125").await,
    ];

    let mut cases = vec![
        Selector {
            case: "no selector reports every stored request",
            selector: json!({}),
            expected: &[0, 1, 2],
        },
        Selector {
            case: "an agreement selects the requests sent under it",
            selector: json!({"agreementId": alpha.clone()}),
            expected: &[0, 1],
        },
        Selector {
            case: "another agreement selects only its own",
            selector: json!({"agreementId": beta.clone()}),
            expected: &[2],
        },
        Selector {
            case: "an agreement the store does not hold selects nothing",
            selector: json!({"agreementId": "agmt-does-not-exist"}),
            expected: &[],
        },
        Selector {
            case: "the status every stored request carries selects all of them",
            selector: json!({"status": PENDING_APPROVAL}),
            expected: &[0, 1, 2],
        },
        Selector {
            case: "the agreement type these agreements carry selects all of them",
            selector: json!({"agreementType": AGREEMENT_TYPE}),
            expected: &[0, 1, 2],
        },
        Selector {
            case: "another agreement type selects nothing",
            selector: json!({"agreementType": "VendorInsightsAgreement"}),
            expected: &[],
        },
        Selector {
            case: "the catalog these agreements were created in selects all of them",
            selector: json!({"catalog": CATALOG}),
            expected: &[0, 1, 2],
        },
        Selector {
            case: "another catalog selects nothing",
            selector: json!({"catalog": "AWSMarketplace.Staging"}),
            expected: &[],
        },
        Selector {
            case: "an agreement and the status it holds are both applied",
            selector: json!({"agreementId": alpha.clone(), "status": PENDING_APPROVAL}),
            expected: &[0, 1],
        },
        Selector {
            case: "an agreement and a catalog it is not in select nothing",
            selector: json!({"agreementId": alpha.clone(), "catalog": "Other"}),
            expected: &[],
        },
    ];
    // No operation that moves a payment request out of PENDING_APPROVAL is
    // implemented yet, so every other status selects nothing.
    for status in OTHER_STATUSES {
        cases.push(Selector {
            case: "a status no stored request carries selects nothing",
            selector: json!({"status": status}),
            expected: &[],
        });
    }

    for case in &cases {
        let mut body = json!({"partyType": "Acceptor"});
        for (member, value) in case
            .selector
            .as_object()
            .expect("a selector is a JSON object")
        {
            body[member] = value.clone();
        }
        let answer = list(&router, body.clone()).await;
        let expected: Vec<String> = case
            .expected
            .iter()
            .map(|index| ids[*index].clone())
            .collect();
        assert_eq!(
            listed_ids(&answer),
            expected,
            "case {:?}: {body} must select {expected:?}, got {:?}",
            case.case,
            answer.body
        );
    }

    // `status` targets the `PaymentRequestStatus` enum, so a value outside it is
    // refused rather than selecting nothing: a 200 with an empty page would tell
    // a caller its spelling was a state no request is in.
    for status in ["pending_approval", "PENDING", "NOT_A_STATUS"] {
        let answer = list(&router, json!({"partyType": "Acceptor", "status": status})).await;
        assert_eq!(
            answer.status, 400,
            "case {status:?}: not a PaymentRequestStatus, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {status:?}: the refusal is a ValidationException, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `maxResults` bounds the page and the token resumes the same list: paging at
/// any size yields every stored payment request exactly once, in the one order
/// the list has.
///
/// The sizes span the published range `MaxResults` declares
/// (`@range(min: 1, max: 50)`): both bounds, sizes below, at and above the
/// number of stored requests, and the absent `maxResults` whose default the
/// operation documents as 50.
#[tokio::test]
async fn the_page_size_bounds_each_page_and_the_token_resumes_the_list() {
    let router = emulator();
    let agreement = agreement_with_variable_term(&router, "ap-paged", "vpt-paged").await;
    let mut all: Vec<String> = Vec::new();
    for index in 0..7 {
        all.push(
            sent(
                &router,
                &agreement,
                "vpt-paged",
                &format!("Payment number {index}"),
                &format!("{}.25", index + 1),
            )
            .await,
        );
    }

    let request = json!({"partyType": "Acceptor"});
    for max_results in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(50)] {
        let (collected, sizes) = paged(&router, request.clone(), max_results).await;
        assert_eq!(
            collected, all,
            "case maxResults={max_results:?}: paging must yield every stored request \
             once, in order, got {collected:?}"
        );
        // Every page but the last is full; the last holds what remains.
        let size = usize::try_from(max_results.unwrap_or(50)).expect("a positive page size");
        let expected_sizes: Vec<usize> = (0..all.len())
            .step_by(size)
            .map(|offset| size.min(all.len() - offset))
            .collect();
        assert_eq!(
            sizes, expected_sizes,
            "case maxResults={max_results:?}: each page is bounded by the page size, \
             got {sizes:?}"
        );
    }

    // One past each published bound is refused before any request is scanned.
    for max_results in [0, 51] {
        let answer = list(
            &router,
            json!({"partyType": "Acceptor", "maxResults": max_results}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={max_results}: outside MaxResults' 1..=50 range, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case maxResults={max_results}: the refusal is a ValidationException, got {:?}",
            answer.body
        );
    }
}

/// A token this list never minted is refused, including one minted for a
/// different list: a token resumes the query it belongs to or nothing.
#[tokio::test]
async fn a_token_this_list_did_not_mint_is_refused() {
    let router = emulator();
    let alpha = agreement_with_variable_term(&router, "ap-tokenalpha", "vpt-a").await;
    let beta = agreement_with_variable_term(&router, "ap-tokenbeta", "vpt-b").await;
    for index in 0..3 {
        sent(
            &router,
            &alpha,
            "vpt-a",
            &format!("Alpha payment {index}"),
            "5",
        )
        .await;
    }
    sent(&router, &beta, "vpt-b", "Beta payment", "5").await;

    let first = list(&router, json!({"partyType": "Acceptor", "maxResults": 1})).await;
    let token = first.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "a bounded page over four requests has a token: {:?}",
                first.body
            )
        })
        .to_string();

    // The same list resumes with it.
    let resumed = list(
        &router,
        json!({"partyType": "Acceptor", "maxResults": 1, "nextToken": token}),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "the token resumes the list it was minted for, got {:?}",
        resumed.body
    );
    assert_eq!(
        listed_ids(&resumed).len(),
        1,
        "the resumed page is bounded too, got {:?}",
        resumed.body
    );

    // Every other list refuses it, and so does a token nothing minted.
    let strangers = [
        (
            "a different agreement",
            json!({"partyType": "Acceptor", "agreementId": beta, "nextToken": token}),
        ),
        (
            "a different status",
            json!({"partyType": "Acceptor", "status": "APPROVED", "nextToken": token}),
        ),
        (
            "a token nothing minted",
            json!({"partyType": "Acceptor", "nextToken": "ZZZZ0000ZZZZ0000ZZZZ0000"}),
        ),
    ];
    for (case, body) in strangers {
        let answer = list(&router, body.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: {body} must be refused, got {} with {:?}",
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
            "case {case:?}: the refusal names the token, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// partyType
// ---------------------------------------------------------------------------

/// `partyType` is required and names one of the two roles the service defines:
/// "Use `Proposer` to list payment requests where you are the seller, or
/// `Acceptor` to list payment requests where you are the buyer."
///
/// The rows are the two values, their case variants, the role names the service
/// does NOT use, and a missing member.
#[tokio::test]
async fn the_party_type_must_name_one_of_the_two_roles() {
    let router = emulator();
    let agreement = agreement_with_variable_term(&router, "ap-party", "vpt-party").await;
    let id = sent(&router, &agreement, "vpt-party", "Party payment", "12.5").await;

    let accepted = ["Proposer", "Acceptor"];
    for party_type in accepted {
        let answer = list(&router, json!({"partyType": party_type})).await;
        assert_eq!(
            listed_ids(&answer),
            vec![id.clone()],
            "case {party_type:?}: the caller is this party of the stored request, got {:?}",
            answer.body
        );
    }

    let refused = [
        "PROPOSER", "proposer", "ACCEPTOR", "acceptor", "Seller", "Buyer", "Both",
    ];
    for party_type in refused {
        let answer = list(&router, json!({"partyType": party_type})).await;
        assert_eq!(
            answer.status, 400,
            "case {party_type:?}: not a party type the service defines, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {party_type:?}: the refusal is a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            "INVALID_PARTY_TYPE",
            "case {party_type:?}: the refusal names the party type, got {:?}",
            answer.body
        );
    }

    // "PartyType is a required parameter. A ValidationException is returned if
    // PartyType is not provided."
    let missing = list(&router, json!({})).await;
    assert_eq!(
        missing.status, 400,
        "a list naming no partyType is refused, got {:?}",
        missing.body
    );
    assert_eq!(
        missing.error_code(),
        "ValidationException",
        "the refusal is a ValidationException, got {:?}",
        missing.body
    );
}

// ---------------------------------------------------------------------------
// What a payment request cannot be sent for
// ---------------------------------------------------------------------------

/// One row of the refusal table: the send that must be refused, the HTTP status
/// and wire code it answers, and the `ValidationException` reason when one
/// applies.
struct Refusal {
    case: &'static str,
    /// Filled in with the agreement the test created, when the row names one.
    body: Value,
    status: u16,
    code: &'static str,
    reason: Option<&'static str>,
}

/// A payment request is sent only under an active agreement's own
/// `VariablePaymentTerm`, for a positive amount — "The agreement must be active
/// and have a `VariablePaymentTerm` to support payment requests", and "A
/// `ValidationException` is returned if the `chargeAmount` exceeds the available
/// balance, if the agreement doesn't have an active `VariablePaymentTerm`, or if
/// the `termId` is invalid."
///
/// The rows differ along which of those holds: an agreement that does not
/// exist, one that is no longer active, a term id naming a term of another kind
/// or none at all, and every spelling of a non-positive amount the member's
/// pattern admits.
#[tokio::test]
async fn a_payment_request_needs_an_active_agreement_with_that_variable_term() {
    let router = emulator();
    let active = agreement_with_variable_term(&router, "ap-refusals", "vpt-live").await;
    // An agreement whose only accepted term is a legal one: it supports no
    // payment request at all.
    let legal_only = agreement_over(&router, "ap-legal", vec![term("LegalTerm")]).await;
    let cancelled = agreement_with_variable_term(&router, "ap-cancelled", "vpt-gone").await;
    let answer = cancel(&router, &cancelled).await;
    assert_eq!(
        answer.status, 200,
        "cancelling the agreement must succeed, got {:?}",
        answer.body
    );

    let cases = [
        Refusal {
            case: "an agreement the store does not hold",
            body: json!({
                "agreementId": "agmt-does-not-exist",
                "termId": "vpt-live",
                "name": "Payment for nothing",
                "chargeAmount": "10",
            }),
            // This operation validates the agreement it is asked to charge
            // under as INPUT, so an `agreementId` it cannot charge under is the
            // modelled input fault naming that member — see
            // `tests/all/error_bodies.rs` for the rule over the whole class.
            status: 400,
            code: "ValidationException",
            reason: Some("INVALID_AGREEMENT_ID"),
        },
        Refusal {
            case: "an agreement that is no longer active",
            body: json!({
                "agreementId": cancelled,
                "termId": "vpt-gone",
                "name": "Payment after cancellation",
                "chargeAmount": "10",
            }),
            status: 400,
            code: "ValidationException",
            reason: Some("INACTIVE_AGREEMENT"),
        },
        Refusal {
            case: "a term id the agreement does not carry",
            body: json!({
                "agreementId": active.clone(),
                "termId": "vpt-not-this-one",
                "name": "Payment on no term",
                "chargeAmount": "10",
            }),
            status: 400,
            code: "ValidationException",
            reason: Some("INVALID_TERM_ID"),
        },
        Refusal {
            case: "a term of the agreement that is not a VariablePaymentTerm",
            body: json!({
                "agreementId": legal_only,
                "termId": "LegalTerm",
                "name": "Payment on a legal term",
                "chargeAmount": "10",
            }),
            status: 400,
            code: "ValidationException",
            reason: Some("INVALID_TERM_ID"),
        },
        Refusal {
            case: "a zero amount",
            body: json!({
                "agreementId": active.clone(),
                "termId": "vpt-live",
                "name": "Payment of zero",
                "chargeAmount": "0",
            }),
            status: 400,
            code: "ValidationException",
            reason: Some("INVALID_CHARGE_AMOUNT"),
        },
        Refusal {
            case: "a zero amount spelled with every decimal the pattern admits",
            body: json!({
                "agreementId": active.clone(),
                "termId": "vpt-live",
                "name": "Payment of zero again",
                "chargeAmount": "0.00000000",
            }),
            status: 400,
            code: "ValidationException",
            reason: Some("INVALID_CHARGE_AMOUNT"),
        },
        Refusal {
            case: "no amount at all, which the pattern admits as the empty string",
            body: json!({
                "agreementId": active.clone(),
                "termId": "vpt-live",
                "name": "Payment of nothing",
                "chargeAmount": "",
            }),
            status: 400,
            code: "ValidationException",
            reason: None,
        },
    ];

    for case in &cases {
        let answer = send(&router, case.body.clone()).await;
        assert_eq!(
            answer.status, case.status,
            "case {:?}: expected HTTP {}, got {} with {:?}",
            case.case, case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            case.code,
            "case {:?}: expected {}, got {:?}",
            case.case,
            case.code,
            answer.body
        );
        if let Some(reason) = case.reason {
            assert_eq!(
                answer.reason(),
                reason,
                "case {:?}: expected reason {reason}, got {:?}",
                case.case,
                answer.body
            );
        }
    }

    // None of those refusals stored anything.
    let listed = list(&router, json!({"partyType": "Proposer"})).await;
    assert!(
        listed_ids(&listed).is_empty(),
        "a refused send stores no payment request, got {:?}",
        listed.body
    );
}
