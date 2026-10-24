// SPDX-License-Identifier: CC0-1.0

//! `ListAgreementCharges`, end to end over HTTP.
//!
//! The rule under test: the report "allows acceptors to view charges and
//! purchase orders that are associated with an agreement" and "includes details
//! about all charges regardless of whether a purchase order is linked to each
//! charge", and it is `@readonly`
//! (`smithy_query(query="operation", operation="ListAgreementCharges")`). So it
//! is computed from the stored agreements and the quotes they were accepted
//! from: it carries every charge of every agreement its selectors
//! (`agreementId`, `agreementType`, `catalog`) admit and no charge of anything
//! else, each charge carrying the identity its quote minted and the agreement
//! that resulted in it (`Charge$agreementId`, `Charge$agreementType`) and a
//! `purchaseOrderReference` only while an association records one ("if any",
//! `smithy_query(query="shape", shape="Charge")`), in one stable order bounded
//! by `maxResults` (`@range(min: 1, max: 50)`,
//! `smithy_query(query="shape", shape="MaxResults")`) — and reading it leaves
//! what a later caller sees unchanged.
//!
//! The rows differ in what the rule generalises over: how many agreements the
//! store holds, how many charges each holds (none, one, several), whether the
//! quote behind a charge was ever accepted, which of an agreement's charges
//! carry a purchase order and in what form, and the page size the report is
//! asked for. Every agreement id and charge id is read back from the call that
//! minted it, so no row can agree with a handler that invents ids.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{call, emulator, is_next_token, Answer};
use crate::purchase_order_associations::{order, quote};
use crate::purchase_order_updates::{agreement_holding, charges, items};

/// The `agreementType` every agreement this service mints carries, and so the
/// one `Charge$agreementType` reports ("for example, `PurchaseAgreement`").
const AGREEMENT_TYPE: &str = "PurchaseAgreement";

/// The other value `DescribeAgreementOutput$agreementType` documents, which no
/// agreement of this service carries.
const OTHER_AGREEMENT_TYPE: &str = "VendorInsightsAgreement";

/// The one catalog AWS Marketplace exposes, which every agreement of this
/// service is created in.
const CATALOG: &str = "AWSMarketplace";

/// A catalog name no agreement of this service is created in.
const OTHER_CATALOG: &str = "AWSMarketplace.test";

/// A charge is minted once and never revised here, so every reported charge sits
/// at `ChargeRevision`'s first value (`@range(min: 1)`,
/// `smithy_query(query="shape", shape="ChargeRevision")`).
const INITIAL_REVISION: i64 = 1;

/// `MaxResults`' published bounds, spelled out here rather than imported so a
/// handler that serves a different range fails these rows.
const MAX_RESULTS_MIN: i64 = 1;
/// See [`MAX_RESULTS_MIN`].
const MAX_RESULTS_MAX: i64 = 50;

/// The charge ids the report carries, in the order it reported them.
fn reported_ids(answer: &Answer) -> Vec<String> {
    items(answer)
        .iter()
        .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The report for `request`, asserted to have succeeded.
async fn reported(router: &MultiServiceRouter, request: Value) -> Answer {
    let answer = charges(router, request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {request:?}: the report must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// Associate `reference` with `charge_id` through `UpdatePurchaseOrders`.
async fn link(router: &MultiServiceRouter, charge_id: &str, reference: &str) {
    let updated = call(
        router,
        "UpdatePurchaseOrders",
        json!({"purchaseOrders": [order(charge_id, reference)]}),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "associating {reference:?} with {charge_id} must succeed, got {} with {:?}",
        updated.status, updated.body
    );
}

/// One store to report on: how many charges each accepted agreement holds, plus
/// a quote that is never accepted, whose charges therefore belong to no
/// agreement.
struct Store {
    case: &'static str,
    /// One entry per agreement, each the number of charges its quote mints.
    agreements: &'static [usize],
    /// How many charges the quote that is never accepted mints.
    unaccepted: usize,
}

fn stores() -> Vec<Store> {
    vec![
        Store {
            case: "no agreement at all",
            agreements: &[],
            unaccepted: 1,
        },
        Store {
            case: "one agreement holding no charge",
            agreements: &[0],
            unaccepted: 0,
        },
        Store {
            case: "one agreement holding one charge",
            agreements: &[1],
            unaccepted: 2,
        },
        Store {
            case: "two agreements holding two and three charges",
            agreements: &[2, 3],
            unaccepted: 1,
        },
        Store {
            case: "three agreements, one of them holding no charge",
            agreements: &[4, 0, 2],
            unaccepted: 3,
        },
    ]
}

/// The report carries every charge of every agreement its selectors admit, each
/// naming the agreement that resulted in it — and no charge of a quote that was
/// never accepted.
#[tokio::test]
async fn the_report_carries_every_charge_of_every_agreement_it_selects() {
    for (index, store) in stores().into_iter().enumerate() {
        let case = store.case;
        let router = emulator();

        let mut held: Vec<(String, Vec<String>)> = Vec::new();
        for (position, count) in store.agreements.iter().copied().enumerate() {
            held.push(
                agreement_holding(&router, &format!("ap-charges{index}of{position}"), count).await,
            );
        }
        let (_, unaccepted) =
            quote(&router, &format!("ap-charges{index}pending"), store.unaccepted).await;

        // One agreement at a time: exactly its own charges, in the order its
        // quote minted them, each naming it and the type it carries.
        for (agreement, minted) in &held {
            let answer = reported(&router, json!({"agreementId": agreement})).await;
            assert_eq!(
                reported_ids(&answer),
                *minted,
                "case {case:?}: expected the charges {minted:?} agreement {agreement} holds, got \
                 {:?}",
                answer.body
            );
            for charge in items(&answer) {
                assert_eq!(
                    charge["agreementId"].as_str(),
                    Some(agreement.as_str()),
                    "case {case:?}: expected charge {charge:?} to name agreement {agreement:?}"
                );
                assert_eq!(
                    charge["agreementType"].as_str(),
                    Some(AGREEMENT_TYPE),
                    "case {case:?}: expected charge {charge:?} to name type {AGREEMENT_TYPE:?}"
                );
                assert_eq!(
                    charge["revision"].as_i64(),
                    Some(INITIAL_REVISION),
                    "case {case:?}: expected charge {charge:?} at revision {INITIAL_REVISION}"
                );
                assert!(
                    charge["purchaseOrderReference"].is_null(),
                    "case {case:?}: a charge no association names carries no reference, got \
                     {charge:?}"
                );
            }
            assert!(
                answer.body["nextToken"].is_null(),
                "case {case:?}: {} charges fit in the default page, so the report ends, got {:?}",
                minted.len(),
                answer.body
            );
        }

        // Every agreement at once, and every selector every agreement of this
        // service matches: the union of what the agreements hold, each charge
        // once.
        let mut union: Vec<String> = held.iter().flat_map(|(_, ids)| ids.clone()).collect();
        union.sort();
        for selector in [
            json!({}),
            json!({"agreementType": AGREEMENT_TYPE}),
            json!({"catalog": CATALOG}),
            json!({"agreementType": AGREEMENT_TYPE, "catalog": CATALOG}),
        ] {
            let answer = reported(&router, selector.clone()).await;
            let mut ids = reported_ids(&answer);
            ids.sort();
            assert_eq!(
                ids, union,
                "case {case:?} with {selector:?}: expected every charge {union:?} once, got {:?}",
                answer.body
            );
            for pending in &unaccepted {
                assert!(
                    !ids.contains(pending),
                    "case {case:?} with {selector:?}: charge {pending} of a quote that was never \
                     accepted belongs to no agreement, got {ids:?}"
                );
            }
        }

        // A selector no agreement matches selects no charge at all — an empty
        // report, not a refusal.
        for selector in [
            json!({"agreementType": OTHER_AGREEMENT_TYPE}),
            json!({"catalog": OTHER_CATALOG}),
            json!({"agreementType": AGREEMENT_TYPE, "catalog": OTHER_CATALOG}),
        ] {
            let answer = reported(&router, selector.clone()).await;
            assert!(
                reported_ids(&answer).is_empty(),
                "case {case:?} with {selector:?}: expected no charge, got {:?}",
                answer.body
            );
            assert!(
                answer.body["nextToken"].is_null(),
                "case {case:?} with {selector:?}: an empty report ends, got {:?}",
                answer.body
            );
        }
    }
}

/// One row of the purchase-order table: how many charges the agreement holds,
/// and which of them an association names, with the reference it records.
struct Linked {
    case: &'static str,
    charges: usize,
    /// The position of the charge in the order its quote minted them, and the
    /// reference associated with it. `PurchaseOrderReference` is modelled
    /// `@length(min: 1)` with no pattern
    /// (`smithy_query(query="shape", shape="PurchaseOrderReference")`), so any
    /// non-empty string is a reference.
    links: &'static [(usize, &'static str)],
}

fn linkages() -> Vec<Linked> {
    vec![
        Linked {
            case: "no charge carries a purchase order",
            charges: 3,
            links: &[],
        },
        Linked {
            case: "the first charge carries one",
            charges: 3,
            links: &[(0, "PO-1")],
        },
        Linked {
            case: "the last charge carries one, with a path-like reference",
            charges: 3,
            links: &[(2, "po/2026/0042")],
        },
        Linked {
            case: "every charge carries one",
            charges: 2,
            links: &[(0, "PO-A"), (1, "PO-B")],
        },
        Linked {
            case: "two of four carry one, one of them a bare number",
            charges: 4,
            links: &[(1, "4500000001"), (3, "purchase order with spaces")],
        },
    ]
}

/// Every charge is reported whether or not a purchase order is linked to it, and
/// the reference a charge carries is the one its own association recorded.
#[tokio::test]
async fn every_charge_is_reported_whether_or_not_a_purchase_order_is_linked() {
    for (index, row) in linkages().into_iter().enumerate() {
        let case = row.case;
        let router = emulator();
        let (agreement, minted) =
            agreement_holding(&router, &format!("ap-chargesPO{index}"), row.charges).await;

        for (position, reference) in row.links {
            link(&router, &minted[*position], reference).await;
        }

        let answer = reported(&router, json!({"agreementId": agreement})).await;
        assert_eq!(
            reported_ids(&answer),
            minted,
            "case {case:?}: linking a purchase order changes which charges are reported in no \
             way, expected {minted:?}, got {:?}",
            answer.body
        );
        for (position, charge) in items(&answer).iter().enumerate() {
            let expected = row
                .links
                .iter()
                .find(|(linked, _)| *linked == position)
                .map(|(_, reference)| *reference);
            assert_eq!(
                charge["purchaseOrderReference"].as_str(),
                expected,
                "case {case:?}: expected charge {position} to carry reference {expected:?}, got \
                 {charge:?}"
            );
        }
    }
}

/// `maxResults` bounds every page, both published bounds are served, a value
/// past either is refused, and the pages concatenate to the one order the
/// unpaged report carries.
#[tokio::test]
async fn the_page_size_bounds_the_report_within_the_modelled_range() {
    let router = emulator();
    let (first, first_ids) = agreement_holding(&router, "ap-chargesPage1", 4).await;
    let (_, second_ids) = agreement_holding(&router, "ap-chargesPage2", 2).await;
    let total = first_ids.len() + second_ids.len();

    let unpaged = reported_ids(&reported(&router, json!({})).await);
    assert_eq!(
        unpaged.len(),
        total,
        "the unpaged report carries every stored charge, got {unpaged:?}"
    );

    for (case, selector, expected) in [
        ("every agreement", json!({}), unpaged.clone()),
        (
            "one agreement",
            json!({"agreementId": first}),
            first_ids.clone(),
        ),
    ] {
        for page_size in [MAX_RESULTS_MIN, 2, 5, 25, MAX_RESULTS_MAX] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let mut request = selector.clone();
                request["maxResults"] = json!(page_size);
                if let Some(token) = token.as_deref() {
                    request["nextToken"] = json!(token);
                }
                let page = reported(&router, request).await;
                let ids = reported_ids(&page);
                assert!(
                    i64::try_from(ids.len()).is_ok_and(|len| len <= page_size),
                    "case {case:?} at maxResults {page_size}: a page carries at most {page_size} \
                     charges, got {:?}",
                    page.body
                );
                seen.extend(ids);
                pages += 1;
                assert!(
                    pages <= total + 1,
                    "case {case:?} at maxResults {page_size}: paging must terminate, {pages} \
                     pages so far"
                );
                match page.body["nextToken"].as_str() {
                    Some(next) => {
                        assert!(
                            is_next_token(next),
                            "case {case:?} at maxResults {page_size}: {next:?} is not a modelled \
                             nextToken"
                        );
                        token = Some(next.to_string());
                    }
                    None => break,
                }
            }
            assert_eq!(
                seen, expected,
                "case {case:?} at maxResults {page_size}: expected {expected:?}, got {seen:?}"
            );
        }
    }

    for refused in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let answer = charges(&router, json!({"maxResults": refused})).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults {refused}: outside the published range, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case maxResults {refused}: expected a ValidationException, got {:?}",
            answer.body
        );
    }
}

/// Reading the report — once, or page by page at every page size — changes
/// nothing a later caller sees: the same request answers the same report, and
/// the agreements it reports on are untouched.
#[tokio::test]
async fn reading_the_report_leaves_what_a_later_caller_sees_unchanged() {
    let router = emulator();
    let (agreement, minted) = agreement_holding(&router, "ap-chargesReadonly", 3).await;
    link(&router, &minted[1], "PO-readonly").await;

    let before = reported(&router, json!({"agreementId": agreement})).await;
    let described_before = call(
        &router,
        "DescribeAgreement",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        described_before.status, 200,
        "describing {agreement} must succeed, got {} with {:?}",
        described_before.status, described_before.body
    );

    for page_size in 1..=minted.len() {
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({"agreementId": agreement, "maxResults": page_size});
            if let Some(token) = token.as_deref() {
                request["nextToken"] = json!(token);
            }
            let page = reported(&router, request).await;
            match page.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
    }

    let after = reported(&router, json!({"agreementId": agreement})).await;
    assert_eq!(
        after.body, before.body,
        "case {agreement}: reading the report must not change it, expected {:?}, got {:?}",
        before.body, after.body
    );

    let described_after = call(
        &router,
        "DescribeAgreement",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        described_after.body, described_before.body,
        "case {agreement}: reading charges must not change the agreement, expected {:?}, got {:?}",
        described_before.body, described_after.body
    );
}
