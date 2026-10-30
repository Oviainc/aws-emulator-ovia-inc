// SPDX-License-Identifier: CC0-1.0

//! A listing selector that names a RESOURCE and matches nothing, end to end over
//! HTTP.
//!
//! The rule under test: `ListAgreementCharges`' `agreementId` is an optional
//! FILTER — the member is not `@required`, it targets the generic `ResourceId`
//! rather than this service's own `AgreementId` shape, and the operation models
//! no error for a resource that is not there (`AccessDeniedException`,
//! `ThrottlingException`, `ValidationException`, `InternalServerException` only,
//! `smithy_query(query="operation", operation="ListAgreementCharges")`). Its
//! constraints are therefore `ResourceId`'s length and pattern —
//! well-formedness, not resolvability — and `ValidationException` is "the input
//! fails to satisfy the constraints specified by the service"
//! (`smithy_query(query="shape", shape="ValidationException")`), so a value
//! INSIDE those constraints that the store holds no agreement for violates
//! nothing: it selects nothing, and the report is HTTP 200 with its `items`
//! member PRESENT and empty and no `nextToken`. A value OUTSIDE those
//! constraints is still refused, which is the other half of the same rule.
//!
//! The three sibling listings whose `agreementId` targets `AgreementId`
//! (`ListAgreementPaymentRequests`, `ListAgreementCancellationRequests`,
//! `ListBillingAdjustmentRequests`) RESOLVE the agreement that member names
//! instead — the behavioural corpus records one of them refusing an id naming no
//! agreement — so they are rows here too, asserting that refusal: this service
//! does not apply one answer across its listings, and which of the two a listing
//! gives is what these rows keep apart.
//!
//! The rows differ in what the rule generalises over:
//!
//! * which listing (all four that carry the member);
//! * the form of the id that matches nothing — a readable name, a path-shaped
//!   id, the id of another kind of record, a stored id with a character appended,
//!   a stored id with its first character dropped, a stored id in another case,
//!   and the published bounds of the member (one character, 64 characters), with
//!   one character past the maximum as a refused row;
//! * what the store holds while the id matches nothing: nothing at all, and an
//!   agreement that DOES carry records of every kind listed, so an empty page is
//!   never the accident of an empty store;
//! * which admitted selectors accompany it (`catalog`, `agreementType`, the
//!   listing's own `status`) and at which page size (both published bounds of
//!   `MaxResults`, `@range(min: 1, max: 50)`).
//!
//! The wire harness (`call`, `emulator`) is `agreement_lifecycle`'s, and the
//! records each listing reports are stored through the suites that own them
//! (`purchase_order_updates`, `payment_requests`, `agreement_cancellation_requests`,
//! `billing_adjustments`), so this suite stands up no second harness. Every
//! stored id is read back from the call that minted it.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_cancellation_requests::cancellation_over;
use crate::agreement_lifecycle::{call, emulator, invoiced_agreement, Answer};
use crate::billing_adjustments::{entry, submit};
use crate::payment_requests::{agreement_with_variable_term, sent};
use crate::purchase_order_updates::agreement_holding;
use crate::selector_resolution::{assert_empty_page, assert_refused, resolves_agreement_id};

/// The one catalog AWS Marketplace exposes, which every agreement of this
/// service is created in — an admitted selector that selects everything.
const CATALOG: &str = "AWSMarketplace";

/// The `agreementType` every agreement of this service carries.
const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// The published bounds of the member under test: `agreementId` targets
/// `ResourceId`/`AgreementId`, `@length(min: 1, max: 64)` with
/// `^[A-Za-z0-9_/-]+$`
/// (`smithy_query(query="operation", operation="ListAgreementCharges")`).
const ID_MIN: usize = 1;
/// See [`ID_MIN`].
const ID_MAX: usize = 64;

/// `MaxResults` is `@range(min: 1, max: 50)`, so these are the page sizes at the
/// published bounds.
const PAGE_SIZE_MIN: i64 = 1;
/// See [`PAGE_SIZE_MIN`].
const PAGE_SIZE_MAX: i64 = 50;

/// One listing whose `agreementId` is a filter: its wire name, the members every
/// call of it must carry (`partyType` is `@required` on the two listings of
/// submitted requests), and a `status` value the listing admits, where it has
/// that member.
struct Listing {
    operation: &'static str,
    required: Value,
    /// A value of the listing's own `status` filter that selects the records
    /// stored below, or `None` where the listing has no such member.
    status: Option<&'static str>,
}

fn listings() -> Vec<Listing> {
    vec![
        Listing {
            operation: "ListAgreementCharges",
            required: json!({}),
            status: None,
        },
        Listing {
            operation: "ListAgreementPaymentRequests",
            required: json!({"partyType": "Acceptor"}),
            // `SendAgreementPaymentRequest` documents a new request is created
            // in this status.
            status: Some("PENDING_APPROVAL"),
        },
        Listing {
            operation: "ListAgreementCancellationRequests",
            required: json!({"partyType": "Acceptor"}),
            // `AgreementCancellationRequestStatus`' first value, which is the
            // status a request this service stores is created in
            // (`smithy_query(query="shape",
            // shape="AgreementCancellationRequestStatus")`).
            status: Some("PENDING_APPROVAL"),
        },
        Listing {
            operation: "ListBillingAdjustmentRequests",
            required: json!({}),
            // `BillingAdjustmentStatus`' first value.
            status: Some("PENDING"),
        },
    ]
}

/// The store every row over an unmatched id is answered on: the agreement whose
/// id those rows are derived from, and the agreement that actually HOLDS the
/// records each listing reports, keyed by that listing's operation.
///
/// Each record is stored through the operation that creates it, so none of these
/// ids is written by hand.
async fn populated(router: &MultiServiceRouter) -> (String, BTreeMap<&'static str, String>) {
    let agreement = agreement_with_variable_term(router, "ap-unmatchedfull", "vpt-unmatched").await;
    sent(router, &agreement, "vpt-unmatched", "Unmatched probe", "12").await;
    cancellation_over(router, &agreement, "OTHER").await;
    // The adjustment is made against an invoice, which its own agreement has.
    let (invoiced, invoice) = invoiced_agreement(router, "ap-unmatchedinvoiced").await;
    let submitted = submit(
        router,
        vec![entry(&invoiced, &invoice, "1", "tok-unmatched")],
    )
    .await;
    assert_eq!(
        submitted.status, 200,
        "submitting one billing adjustment request must succeed, got {} with {:?}",
        submitted.status, submitted.body
    );
    // And an agreement holding charges, so the charge report has rows too.
    let (holding, charge_ids) = agreement_holding(router, "ap-unmatchedcharges", 2).await;
    assert_eq!(
        charge_ids.len(),
        2,
        "the quote must mint the charges the report carries, got {charge_ids:?}"
    );

    let holders = BTreeMap::from([
        ("ListAgreementCharges", holding),
        ("ListAgreementPaymentRequests", agreement.clone()),
        ("ListAgreementCancellationRequests", agreement.clone()),
        ("ListBillingAdjustmentRequests", invoiced),
    ]);
    (agreement, holders)
}

/// The ids that match nothing, derived from `stored` so the rows differ along
/// HOW the id misses, with the two published bounds of the member among them.
fn unmatched_ids(stored: &str) -> Vec<(String, String)> {
    assert!(
        stored.len() > 1,
        "a minted agreement id is longer than one character, got {stored:?}"
    );
    let mut rows = vec![
        ("a readable name".to_string(), "agmt-does-not-exist".to_string()),
        (
            "a path-shaped id".to_string(),
            "agreement/0000000000000001".to_string(),
        ),
        (
            "the id of another kind of record".to_string(),
            "ar-0000000000000001".to_string(),
        ),
        (
            "a stored id with a character appended".to_string(),
            format!("{stored}0"),
        ),
        (
            "a stored id with its first character dropped".to_string(),
            stored[1..].to_string(),
        ),
        (
            "a stored id in another case".to_string(),
            stored.to_uppercase(),
        ),
        (
            format!("the shortest id the member admits ({ID_MIN} character)"),
            "a".to_string(),
        ),
        (
            format!("the longest id the member admits ({ID_MAX} characters)"),
            "A".repeat(ID_MAX),
        ),
    ];
    rows.retain(|(_, id)| id != stored);
    rows
}

/// The call `listing` makes with `selector` added to its required members.
async fn listed(router: &MultiServiceRouter, listing: &Listing, selector: &Value) -> Answer {
    let mut request = listing.required.clone();
    for (member, value) in selector.as_object().expect("a selector is an object") {
        request[member] = value.clone();
    }
    call(router, listing.operation, request).await
}

/// Assert one answer is what `listing` gives for an `agreementId` the store
/// holds no agreement for: the empty page where the member is a filter, and the
/// modelled refusal naming that member where the listing resolves it.
///
/// Both assertions are `selector_resolution`'s, so this suite and the one that
/// pins the selector classes cannot drift on what either answer looks like.
fn assert_unmatched(case: &str, listing: &Listing, answer: &Answer) {
    if resolves_agreement_id(listing.operation) {
        assert_refused(case, "agreementId", "INVALID_AGREEMENT_ID", answer);
    } else {
        assert_empty_page(case, answer);
    }
}

/// Each listing answers a well-formed `agreementId` it holds no agreement for
/// the one way its own member is read — the empty page where that member
/// filters, the refusal where the listing resolves it — on an empty store and on
/// one holding records of every kind these listings report, whichever admitted
/// selectors and page size the call carries.
#[tokio::test]
async fn an_agreement_id_that_matches_nothing_is_served_as_an_empty_page_where_it_filters() {
    for (store, with_records) in [("an empty store", false), ("a populated store", true)] {
        let router = emulator();
        let (stored, holders) = if with_records {
            populated(&router).await
        } else {
            // A well-formed id of the shape this service mints, so the rows
            // below are derived the same way on both stores.
            ("agmt-0000000000000001".to_string(), BTreeMap::new())
        };

        // What accompanies the unmatched id: nothing, the admitted value of each
        // other selector, and the published page-size bounds.
        let extras: Vec<(&str, Value)> = vec![
            ("alone", json!({})),
            ("beside the published catalog", json!({"catalog": CATALOG})),
            (
                "beside the agreement type every agreement carries",
                json!({"agreementType": AGREEMENT_TYPE}),
            ),
            (
                "beside the smallest page the model admits",
                json!({"maxResults": PAGE_SIZE_MIN}),
            ),
            (
                "beside the largest page the model admits",
                json!({"maxResults": PAGE_SIZE_MAX}),
            ),
            (
                "beside every admitted selector at once",
                json!({
                    "catalog": CATALOG,
                    "agreementType": AGREEMENT_TYPE,
                    "maxResults": PAGE_SIZE_MAX,
                }),
            ),
        ];

        for listing in listings() {
            for (id_case, id) in unmatched_ids(&stored) {
                for (extra_case, extra) in &extras {
                    let mut selector = extra.clone();
                    selector["agreementId"] = json!(id);
                    let case = format!(
                        "{} on {store}: {id_case} ({id:?}) {extra_case}",
                        listing.operation
                    );
                    let answer = listed(&router, &listing, &selector).await;
                    assert_unmatched(&case, &listing, &answer);

                    // The same id beside the listing's own status filter, where
                    // it has one: an AND of two filters, one of which matches
                    // nothing, selects nothing — and where the listing resolves
                    // the agreement, the second filter does not change the
                    // refusal either.
                    if let Some(status) = listing.status {
                        let mut with_status = selector.clone();
                        with_status["status"] = json!(status);
                        let case = format!("{case}, and the status {status:?}");
                        let answer = listed(&router, &listing, &with_status).await;
                        assert_unmatched(&case, &listing, &answer);
                    }
                }
            }
        }

        // The complement, on the store that holds records: the id of the
        // agreement that DOES carry them reports them, so the answers above are
        // about the id that matched nothing and not the listing reporting
        // nothing whatever it is asked.
        if !with_records {
            continue;
        }
        for listing in listings() {
            let holder = &holders[listing.operation];
            let case = format!("{} over the agreement holding its records", listing.operation);
            let answer = listed(&router, &listing, &json!({"agreementId": holder})).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let reported = answer.body["items"].as_array().map(Vec::len).unwrap_or(0);
            assert!(
                reported >= 1,
                "case {case:?}: expected the records {holder} carries, got {:?}",
                answer.body
            );
        }
    }
}

/// The other half of the rule: a value OUTSIDE the member's published
/// constraints fails one, so it is refused — 400 `ValidationException` naming
/// `agreementId` — rather than served as an empty page. One character past the
/// maximum length and one value outside the pattern.
#[tokio::test]
async fn an_agreement_id_outside_the_published_constraints_is_still_refused() {
    let router = emulator();
    let (stored, _holders) = populated(&router).await;
    assert!(
        !stored.is_empty(),
        "the populated store must mint an agreement, got {stored:?}"
    );

    let malformed: Vec<(String, String)> = vec![
        (
            format!("one character past the maximum length ({ID_MAX})"),
            "A".repeat(ID_MAX + 1),
        ),
        (
            "below the minimum length".to_string(),
            String::new(),
        ),
        (
            "characters the pattern does not admit".to_string(),
            "agmt not an id!".to_string(),
        ),
    ];

    for listing in listings() {
        for (case, id) in &malformed {
            let case = format!("{}: {case} ({id:?})", listing.operation);
            let answer = listed(&router, &listing, &json!({"agreementId": id})).await;
            assert_refused(&case, "agreementId", "INVALID_AGREEMENT_ID", &answer);
        }
    }
}

/// Answering an `agreementId` that matches nothing changes nothing a later
/// caller sees: the records of the agreement that holds them are reported
/// identically before and after, and the unmatched call answers the same thing
/// every time it is made.
#[tokio::test]
async fn an_unmatched_page_leaves_what_a_later_caller_sees_unchanged() {
    let router = emulator();
    let (_stored, holders) = populated(&router).await;

    for listing in listings() {
        let holder = &holders[listing.operation];
        let before = listed(&router, &listing, &json!({"agreementId": holder})).await;
        assert_eq!(
            before.status, 200,
            "{}: reporting the agreement holding its records must succeed, got {} with {:?}",
            listing.operation, before.status, before.body
        );

        for round in 0..2 {
            let case = format!("{} unmatched, round {round}", listing.operation);
            let answer = listed(
                &router,
                &listing,
                &json!({"agreementId": "agmt-does-not-exist"}),
            )
            .await;
            assert_unmatched(&case, &listing, &answer);
        }

        let after = listed(&router, &listing, &json!({"agreementId": holder})).await;
        assert_eq!(
            after.body, before.body,
            "{}: the unmatched reads must leave the report unchanged, expected {:?}, got {:?}",
            listing.operation, before.body, after.body
        );
    }
}
