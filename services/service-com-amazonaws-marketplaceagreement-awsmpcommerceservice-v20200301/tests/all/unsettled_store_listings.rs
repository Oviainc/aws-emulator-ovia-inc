// SPDX-License-Identifier: CC0-1.0

//! The listings that answer over EVERY agreement the store holds, driven on a
//! store no other call has read, end to end over HTTP.
//!
//! Every report here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: an operation whose answer is over every agreement the
//! store holds reports it as the clock has already left it, so the answer never
//! depends on whether some unrelated call looked first. An agreement reaches its
//! own stored end date at that instant whether or not anybody calls, and
//! `EndTimeBehaviorType` names what that date does — `RENEW` is "a new agreement
//! is created from the accepted terms of this agreement", `EXPIRE` is "the
//! agreement ends and isn't renewed or replaced"
//! (`smithy_query(query="shape", shape="EndTimeBehavior")`). So the renewal an
//! end date created is an agreement of the store from that date on, with the
//! quote and the charges of its own that `ListAgreementCharges` then reports
//! ("details about all charges regardless of whether a purchase order is linked
//! to each charge",
//! `smithy_query(query="operation", operation="ListAgreementCharges")`) and an
//! `agreementId` these listings hold an agreement for — while an id no end date
//! ever created is still refused.
//!
//! The rows differ along what the rule generalises over: which contract length
//! the acceptor purchased, how many whole lengths have passed since it was signed
//! (none, one, several, as many as the renewal term allows, more), what the terms
//! say at that date (auto-renewal enabled, opted out of, no renewal term at all)
//! and whether the terms commit an end date at all — and, for the listings, which
//! of them resolves an `agreementId` against the store.
//!
//! The clock cannot be wound forward 30 days inside a test, so the agreement's
//! own stored window is wound BACK instead: the harness that does it
//! (`aged_agreement`, `days`) is the one `agreement_expiry` owns, the wire
//! harness (`call`, `emulator_over_engine`, `renewal_term`, `search`, `term`,
//! `upfront_term`, `with_agreement_type`) is `agreement_lifecycle`'s, the terms
//! of an auto-renewing purchase (`renewing_terms`) are `agreement_renewals`', and
//! the charge report (`charges`, `items`) is `purchase_order_updates`'.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_expiry::{aged_agreement, days};
use crate::agreement_lifecycle::{
    call, emulator_over_engine, renewal_term, search, term, upfront_term, with_agreement_type,
    Answer,
};
use crate::agreement_renewals::renewing_terms;
use crate::purchase_order_updates::{charges, items};

/// The party a listing of submitted requests is read as, from
/// `smithy_query(query="shape", shape="PartyType")`: "Use `Proposer`… or
/// `Acceptor`". Either value carries these rows; the acceptor is the account that
/// bought the agreements this suite ages.
const ACCEPTOR: &str = "Acceptor";

/// An `agreementId` that matches `ResourceId`'s `^[A-Za-z0-9_/-]+$` and names no
/// agreement any end date of this store could have created, so it stays an
/// invalid value for the member.
const UNKNOWN_AGREEMENT: &str = "agmt-ffffffffffffffff";

/// The `ValidationExceptionReason` an `agreementId` naming no agreement is
/// refused with, which is what a client branches on.
const INVALID_AGREEMENT_ID: &str = "INVALID_AGREEMENT_ID";

/// The contract length the single-chain rows buy, and how long after signing a
/// purchase of it is one renewal in.
const MONTH: &str = "P1M";
/// See [`MONTH`]: a month's purchase is one renewal in a day past its end date.
const A_MONTH_IN_DAYS: u64 = 31;

/// One store to report on: the terms an acceptor accepted, how long before now
/// the first agreement of the chain was signed, and how many agreements the chain
/// therefore holds once the clock has settled it.
struct Aged {
    case: &'static str,
    requested: Vec<Value>,
    signed_ago: Duration,
    /// One for a chain whose end date made no renewal due, and one more per
    /// renewal the clock owes.
    links: usize,
}

fn aged_stores() -> Vec<Aged> {
    vec![
        Aged {
            case: "a month's purchase a day past its end date",
            requested: renewing_terms(MONTH),
            signed_ago: days(A_MONTH_IN_DAYS),
            links: 2,
        },
        Aged {
            case: "two whole months on: the renewal renews in turn",
            requested: renewing_terms(MONTH),
            signed_ago: days(61),
            links: 3,
        },
        Aged {
            case: "as many months past as the renewal term allows renewals",
            requested: renewing_terms(MONTH),
            signed_ago: days(91),
            links: 4,
        },
        Aged {
            case: "past the end of the last renewal the term allows, which then expires",
            requested: renewing_terms(MONTH),
            signed_ago: days(121),
            links: 4,
        },
        Aged {
            case: "a year's purchase a day past its end date, at another length",
            requested: renewing_terms("P12M"),
            signed_ago: days(366),
            links: 2,
        },
        Aged {
            case: "a three-year purchase a day past its end date",
            requested: renewing_terms("P36M"),
            signed_ago: days(1096),
            links: 2,
        },
        Aged {
            case: "one second short of its end date: nothing is due yet",
            requested: renewing_terms(MONTH),
            signed_ago: days(30) - Duration::from_secs(1),
            links: 1,
        },
        Aged {
            case: "auto-renewal opted out of: the end date expires the agreement instead",
            requested: vec![
                upfront_term("cfg", MONTH, &[("Users", 2)]),
                renewal_term("renew", false),
            ],
            signed_ago: days(A_MONTH_IN_DAYS),
            links: 1,
        },
        Aged {
            case: "no renewal term at all, which is required for an agreement to renew",
            requested: vec![upfront_term("cfg", MONTH, &[("Users", 2)])],
            signed_ago: days(A_MONTH_IN_DAYS),
            links: 1,
        },
        Aged {
            case: "metered usage: a pay-as-you-go agreement has no end date to renew at",
            requested: vec![term("UsageBasedPricingTerm"), renewal_term("renew", true)],
            signed_ago: days(10 * 365),
            links: 1,
        },
    ]
}

/// The listings whose `agreementId` is resolved against every agreement the
/// store holds, each beside the rest of the request it requires: `partyType` is
/// `@required` on the two listings of submitted requests
/// (`smithy_query(query="operation", operation="ListAgreementPaymentRequests")`).
fn whole_store_listings(agreement: &str) -> Vec<(&'static str, Value)> {
    vec![
        ("ListAgreementCharges", json!({"agreementId": agreement})),
        (
            "ListAgreementPaymentRequests",
            json!({"partyType": ACCEPTOR, "agreementId": agreement}),
        ),
        (
            "ListAgreementCancellationRequests",
            json!({"partyType": ACCEPTOR, "agreementId": agreement}),
        ),
        (
            "ListBillingAdjustmentRequests",
            json!({"agreementId": agreement}),
        ),
    ]
}

/// The charge report for `request`, asserted to have succeeded.
async fn reported(router: &MultiServiceRouter, request: Value) -> Answer {
    let answer = charges(router, request.clone()).await;
    assert_eq!(
        answer.status, 200,
        "case {request:?}: the report must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// The ids of the charges a report carries.
fn reported_ids(answer: &Answer) -> BTreeSet<String> {
    items(answer)
        .iter()
        .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The agreements the charges of a report name (`Charge$agreementId`, "the
/// unique identifier of the agreement that resulted in the charge").
fn agreements_named(answer: &Answer) -> BTreeSet<String> {
    items(answer)
        .iter()
        .map(|charge| {
            charge["agreementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every agreement the store holds, as one search reports them — the unrelated
/// read whose answer this suite checks the listings against.
async fn held_agreements(router: &MultiServiceRouter) -> BTreeSet<String> {
    let answer = search(
        router,
        json!({"filters": with_agreement_type(&json!([])), "maxResults": 50}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "searching the stored agreements must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| {
            summary["agreementId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The charge report names every agreement the clock has already created, on a
/// store whose ageing no other call has read — and an unrelated read of the same
/// store changes the report in no way.
#[tokio::test]
async fn the_charge_report_covers_every_agreement_the_clock_has_already_created() {
    for (index, row) in aged_stores().into_iter().enumerate() {
        let case = row.case;
        // One emulator per row, so the report below is over this row's chain
        // only.
        let (router, engine) = emulator_over_engine();
        let source = aged_agreement(
            &router,
            &engine,
            &format!("ap-unsettled{index}"),
            row.requested.clone(),
            row.signed_ago,
        )
        .await;

        // The FIRST call the aged store sees: nothing else has settled it.
        let first = reported(&router, json!({})).await;
        let named = agreements_named(&first);
        assert!(
            named.contains(&source),
            "case {case:?}: the purchase that was accepted holds charges, got {named:?}"
        );
        assert_eq!(
            named.len(),
            row.links,
            "case {case:?}: expected the charges of all {} agreement(s) the clock owes, got \
             {named:?}",
            row.links
        );

        // An unrelated read of the same store, which settles the same end
        // dates: it reports exactly the agreements the charge report already
        // named, and the identical report answers the identical body.
        let held = held_agreements(&router).await;
        assert_eq!(
            held, named,
            "case {case:?}: expected the charges to name every agreement the store holds \
             {held:?}, got {named:?}"
        );
        let again = reported(&router, json!({})).await;
        assert_eq!(
            again.body, first.body,
            "case {case:?}: an unrelated read must not change the report, expected {:?}, got {:?}",
            first.body, again.body
        );

        // Each agreement of the chain is an `agreementId` the report selects on,
        // and its charges are its own: together they are exactly the report over
        // the whole store, each charge once.
        let mut per_agreement: BTreeSet<String> = BTreeSet::new();
        for agreement in &held {
            let scoped = reported(&router, json!({"agreementId": agreement})).await;
            let ids = reported_ids(&scoped);
            assert!(
                !ids.is_empty(),
                "case {case:?}: agreement {agreement} was quoted charges of its own, got {:?}",
                scoped.body
            );
            for id in ids {
                assert!(
                    per_agreement.insert(id.clone()),
                    "case {case:?}: charge {id} belongs to one agreement, got it under \
                     {agreement} as well"
                );
            }
        }
        assert_eq!(
            per_agreement,
            reported_ids(&first),
            "case {case:?}: expected the per-agreement reports to carry the same charges as the \
             report over the whole store, got {per_agreement:?}"
        );
    }
}

/// The renewal an end date created is an `agreementId` every listing that
/// resolves one against the store holds an agreement for, even when that call is
/// the first the store sees.
#[tokio::test]
async fn the_renewal_the_clock_owes_is_an_agreement_these_listings_hold() {
    // One store, read only through the report under test: the agreements its
    // charges name are the chain the clock settled.
    let (router, engine) = emulator_over_engine();
    let source = aged_agreement(
        &router,
        &engine,
        "ap-unsettledRenewal",
        renewing_terms(MONTH),
        days(A_MONTH_IN_DAYS),
    )
    .await;
    let named = agreements_named(&reported(&router, json!({})).await);
    let renewals: Vec<String> = named.iter().filter(|id| *id != &source).cloned().collect();
    assert_eq!(
        renewals.len(),
        1,
        "a month's purchase a day past its end date is one renewal in, got {named:?} beside \
         {source}"
    );
    let renewal = renewals[0].clone();
    let renewal_charges = reported_ids(&reported(&router, json!({"agreementId": &renewal})).await);
    assert!(
        !renewal_charges.is_empty(),
        "the renewal's prepaid term is charged, so it holds charges of its own, got \
         {renewal_charges:?}"
    );

    // Each listing on a store of its OWN, built exactly the same way, where the
    // call under test is the FIRST that store sees. Ids are minted from the
    // store's own counter, so the identical setup mints the identical ones —
    // which the source id and the report below pin.
    for (listing, request) in whole_store_listings(&renewal) {
        let (router, engine) = emulator_over_engine();
        let same = aged_agreement(
            &router,
            &engine,
            "ap-unsettledRenewal",
            renewing_terms(MONTH),
            days(A_MONTH_IN_DAYS),
        )
        .await;
        assert_eq!(
            same, source,
            "case {listing}: the identical setup must mint the identical agreement id"
        );

        let answer = call(&router, listing, request.clone()).await;
        let holds = agreements_named(&reported(&router, json!({})).await);
        assert!(
            holds.contains(&renewal),
            "case {listing}: the identical setup must owe the renewal {renewal}, got {holds:?}"
        );
        assert_eq!(
            answer.status, 200,
            "case {listing}: the renewal the end date created is an agreement this listing holds, \
             got {} with {:?}",
            answer.status, answer.body
        );
        if listing == "ListAgreementCharges" {
            assert_eq!(
                reported_ids(&answer),
                renewal_charges,
                "case {listing}: expected the renewal's own charges {renewal_charges:?}, got {:?}",
                answer.body
            );
        }
    }
}

/// An `agreementId` no end date of the store could have created is still not a
/// valid value for the member: settling the clock admits the agreements it owed
/// and nothing else.
#[tokio::test]
async fn an_agreement_id_no_end_date_created_is_still_refused() {
    let (router, engine) = emulator_over_engine();
    let _source = aged_agreement(
        &router,
        &engine,
        "ap-unsettledUnknown",
        renewing_terms(MONTH),
        days(A_MONTH_IN_DAYS),
    )
    .await;

    for (listing, request) in whole_store_listings(UNKNOWN_AGREEMENT) {
        let answer = call(&router, listing, request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {listing}: {UNKNOWN_AGREEMENT} names no agreement, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case {listing}: expected a ValidationException, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.reason(),
            INVALID_AGREEMENT_ID,
            "case {listing}: expected the reason {INVALID_AGREEMENT_ID}, got {:?}",
            answer.body
        );
    }
}
