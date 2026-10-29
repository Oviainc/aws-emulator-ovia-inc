// SPDX-License-Identifier: CC0-1.0

//! The renewal an auto-renewing agreement's end date makes due, end to end over
//! HTTP.
//!
//! Every report here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the
//! same path a client takes.
//!
//! The rule under test: at an agreement's end date the clock materialises the
//! behaviour the agreement's own accepted terms define for that date, and only
//! that behaviour. `EndTimeBehaviorType` names the two ends — `EXPIRE` is "the
//! agreement ends and isn't renewed or replaced", `RENEW` is "a new agreement is
//! created from the accepted terms of this agreement" — and `AgreementStatus`
//! names the status each leaves behind: "EXPIRED – The agreement ended on the
//! defined end date" and "RENEWED – The agreement was renewed into a new
//! agreement (for example, an auto-renewal)"
//! (`smithy_query(query="shape", shape="EndTimeBehavior")` and
//! `…shape="AgreementStatus"`). So an agreement whose accepted renewal term
//! enables auto-renewal is, once its stored `endTime` has passed, reported
//! `RENEWED` beside a new agreement that continues its chain: the same
//! `initialAgreementId` ("it stays the same across all agreements in that chain,
//! which lets you trace an agreement back to the original"), the same accepted
//! terms, a `startTime` at the source's end date and an `endTime` a further
//! contract length on, with charges, an invoice and entitlements of its own.
//! `RenewalTerm$maxRenewals` bounds the chain — "the maximum number of times the
//! agreement can be renewed… After the agreement reaches this limit, it expires
//! on its end date instead of renewing" — which is the
//! `RENEWAL_LIMIT_EXHAUSTED` reason the behaviour then gives.
//!
//! The rows differ along what the rule generalises over: which contract length
//! the acceptor purchased (every length the rate card publishes), how many whole
//! lengths have passed since it was signed (none, one, several, more than the
//! renewal limit allows), whether the end date is reached exactly or passed, what
//! the terms say at that date (auto-renewal enabled, opted out of, no renewal
//! term at all) and whether the terms commit an end date at all.
//!
//! The clock cannot be wound forward 30 days inside a test, so the agreement's
//! own stored window is wound BACK instead: the harness that does it
//! (`aged_agreement`, `days`) is the one `agreement_expiry` owns, the wire
//! harness (`call`, `describe`, `emulator_over_engine`, `invoices_of`,
//! `renewal_term`, `search`, `term`, `upfront_term`, `with_agreement_type`) is
//! `agreement_lifecycle`'s, and the charge report (`charges`, `items`) is
//! `purchase_order_updates`'.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_expiry::{aged_agreement, days};
use crate::agreement_lifecycle::{
    call, describe, emulator_over_engine, invoices_of, renewal_term, search, term, upfront_term,
    with_agreement_type, Answer,
};
use crate::purchase_order_updates::{charges, items};

/// `AgreementStatus` values this suite expects, from
/// `smithy_query(query="shape", shape="AgreementStatus")`: "ACTIVE – The terms of
/// the agreement are active", "EXPIRED – The agreement ended on the defined end
/// date" and "RENEWED – The agreement was renewed into a new agreement (for
/// example, an auto-renewal)". Spelled out here rather than imported from the
/// crate, so a handler reporting another value fails these rows instead of
/// agreeing with them.
const ACTIVE: &str = "ACTIVE";
/// See [`ACTIVE`].
const EXPIRED: &str = "EXPIRED";
/// See [`ACTIVE`].
const RENEWED: &str = "RENEWED";

/// `EndTimeBehaviorType` and `EndTimeBehaviorReasonCode` values a row expects,
/// from `smithy_query(query="shape", shape="EndTimeBehaviorType")` and
/// `…shape="EndTimeBehaviorReasonCode"`.
const RENEW: &str = "RENEW";
/// See [`RENEW`].
const EXPIRE: &str = "EXPIRE";
/// See [`RENEW`].
const RENEWAL_LIMIT_EXHAUSTED: &str = "RENEWAL_LIMIT_EXHAUSTED";
/// See [`RENEW`].
const ACCEPTOR_RENEW_OPTED_OUT: &str = "ACCEPTOR_RENEW_OPTED_OUT";
/// See [`RENEW`].
const NO_RENEWAL_TERM: &str = "NO_RENEWAL_TERM";

/// How many renewals the renewal term of the offer this emulator publishes
/// allows (`RenewalTerm$maxRenewals`, `@range(min: 1)`).
///
/// SOURCE: the `maxRenewals` the accepted renewal term reports, which
/// [`the_renewal_term_publishes_the_limit_these_rows_count_against`] reads back
/// over the wire — spelled out here so a chain that renews a different number of
/// times fails these rows rather than agreeing with them.
const PUBLISHED_MAX_RENEWALS: i64 = 3;

/// The contract lengths the published rate card prices a configured prepaid
/// purchase at, each with how long the agreement then runs, in days.
///
/// SOURCE: the selectors the emulator's rate card publishes
/// (`ConfigurableUpfrontRateCardItem$selector` "differentiates between the
/// mutually exclusive rate cards in the same pricing term to be selected by the
/// buyer"), spelled out here rather than imported so a handler publishing a
/// different set fails these rows.
const CONTRACT_LENGTHS: &[(&str, u64)] = &[
    ("P1M", 30),
    ("P12M", 365),
    ("P24M", 730),
    ("P36M", 1095),
];

/// How close two instants the wire spells in epoch seconds have to be to count
/// as the same: a double carries an agreement's instants to well under a
/// millisecond, and a window is asserted in whole days.
const INSTANT_TOLERANCE: f64 = 1e-3;

/// One row: the terms an acceptor accepted, how long ago the first agreement of
/// the chain was signed, and the chain the clock therefore owes.
struct Renewing {
    case: &'static str,
    requested: Vec<Value>,
    /// How long before now the FIRST agreement of the chain was signed.
    signed_ago: Duration,
    /// How long each agreement of the chain runs for, or `None` for terms that
    /// commit no end date at all.
    window: Option<Duration>,
    /// The status of every agreement of the chain, oldest first: one entry for a
    /// chain whose end date made no renewal due, and one more per renewal the
    /// clock owes.
    statuses: Vec<&'static str>,
    /// The `(type, reasonCode)` the LAST agreement of the chain reports for its
    /// own end date, or `None` when it has no end date to reach.
    behavior: Option<(&'static str, Option<&'static str>)>,
}

/// A prepaid purchase of `length` whose acceptor asked for auto-renewal.
///
/// `pub(crate)` because the suite over the listings that report on the store as
/// the clock has left it (`tests/all/unsettled_store_listings.rs`) ages the same
/// kind of purchase rather than spelling a second copy of the request shape.
pub(crate) fn renewing_terms(length: &str) -> Vec<Value> {
    vec![
        upfront_term("cfg", length, &[("Users", 2)]),
        renewal_term("renew", true),
    ]
}

/// A chain of `renewals` renewals: every agreement but the last is `RENEWED`,
/// and the last carries `last`.
fn chain(renewals: usize, last: &'static str) -> Vec<&'static str> {
    let mut statuses = vec![RENEWED; renewals];
    statuses.push(last);
    statuses
}

fn renewing_cases() -> Vec<Renewing> {
    // One row per contract length the rate card publishes: a purchase of that
    // length signed a day more than that long ago is one renewal in.
    let mut cases: Vec<Renewing> = CONTRACT_LENGTHS
        .iter()
        .map(|(length, length_days)| Renewing {
            case: "a renewing purchase a day past the end of the length it bought",
            requested: renewing_terms(length),
            signed_ago: days(length_days + 1),
            window: Some(days(*length_days)),
            statuses: chain(1, ACTIVE),
            behavior: Some((RENEW, None)),
        })
        .collect();
    cases.extend([
        Renewing {
            case: "signed exactly its contract length ago: the renewal is due at that instant",
            requested: renewing_terms("P1M"),
            signed_ago: days(30),
            window: Some(days(30)),
            statuses: chain(1, ACTIVE),
            behavior: Some((RENEW, None)),
        },
        Renewing {
            case: "one second short of its end date: nothing is due yet",
            requested: renewing_terms("P1M"),
            signed_ago: days(30) - Duration::from_secs(1),
            window: Some(days(30)),
            statuses: chain(0, ACTIVE),
            behavior: Some((RENEW, None)),
        },
        Renewing {
            case: "two whole lengths past its start: the renewal renews in turn",
            requested: renewing_terms("P1M"),
            signed_ago: days(61),
            window: Some(days(30)),
            statuses: chain(2, ACTIVE),
            behavior: Some((RENEW, None)),
        },
        Renewing {
            case: "as many lengths past as the renewal term allows renewals",
            requested: renewing_terms("P1M"),
            signed_ago: days(91),
            window: Some(days(30)),
            statuses: chain(3, ACTIVE),
            behavior: Some((EXPIRE, Some(RENEWAL_LIMIT_EXHAUSTED))),
        },
        Renewing {
            case: "past the end of the last renewal the term allows, which then expires",
            requested: renewing_terms("P1M"),
            signed_ago: days(121),
            window: Some(days(30)),
            statuses: chain(3, EXPIRED),
            behavior: Some((EXPIRE, Some(RENEWAL_LIMIT_EXHAUSTED))),
        },
        Renewing {
            case: "a year's purchase past the limit of its renewals, at another length",
            requested: renewing_terms("P12M"),
            signed_ago: days(4 * 365 + 1),
            window: Some(days(365)),
            statuses: chain(3, EXPIRED),
            behavior: Some((EXPIRE, Some(RENEWAL_LIMIT_EXHAUSTED))),
        },
        Renewing {
            case: "auto-renewal opted out of: the end date expires the agreement instead",
            requested: vec![
                upfront_term("cfg", "P1M", &[("Users", 2)]),
                renewal_term("renew", false),
            ],
            signed_ago: days(31),
            window: Some(days(30)),
            statuses: chain(0, EXPIRED),
            behavior: Some((EXPIRE, Some(ACCEPTOR_RENEW_OPTED_OUT))),
        },
        Renewing {
            case: "no renewal term at all, which is required for an agreement to renew",
            requested: vec![upfront_term("cfg", "P1M", &[("Users", 2)])],
            signed_ago: days(31),
            window: Some(days(30)),
            statuses: chain(0, EXPIRED),
            behavior: Some((EXPIRE, Some(NO_RENEWAL_TERM))),
        },
        Renewing {
            case: "metered usage: a pay-as-you-go agreement has no end date to renew at",
            requested: vec![term("UsageBasedPricingTerm"), renewal_term("renew", true)],
            signed_ago: days(10 * 365),
            window: None,
            statuses: chain(0, ACTIVE),
            behavior: None,
        },
    ]);
    cases
}

/// The `AgreementViewSummary` of every agreement of the chain `initial` is the
/// first link of, oldest first.
///
/// The chain is read through the `InitialAgreementId` filter, which is what the
/// member documents it for: "you can also use it as the `InitialAgreementId`
/// filter value to return every agreement in the same chain".
async fn chain_of(router: &MultiServiceRouter, initial: &str) -> Vec<Value> {
    let answer = search(
        router,
        json!({
            "filters": with_agreement_type(
                &json!([{"name": "InitialAgreementId", "values": [initial]}]),
            ),
            "maxResults": 50,
        }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "searching the chain of {initial} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let mut summaries: Vec<Value> = answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    summaries.sort_by(|left, right| {
        instant(left, "startTime")
            .partial_cmp(&instant(right, "startTime"))
            .expect("every agreement reports a start time")
    });
    summaries
}

/// The instant `member` of one reported agreement, in epoch seconds.
fn instant(reported: &Value, member: &str) -> f64 {
    reported[member]
        .as_f64()
        .unwrap_or_else(|| panic!("{member} must be reported, got {reported:?}"))
}

/// The ids of the agreements a search filtered on `status` selects.
async fn with_status(router: &MultiServiceRouter, status: &str) -> BTreeSet<String> {
    let answer = search(
        router,
        json!({"filters": with_agreement_type(&json!([{"name": "Status", "values": [status]}]))}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "a search filtered on {status} must be served, got {} with {:?}",
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

/// The `(union member, id)` of every accepted term `agreement` reports, in the
/// order `GetAgreementTerms` reports them.
async fn terms_of(router: &MultiServiceRouter, agreement: &str) -> Vec<(String, String)> {
    let answer = call(
        router,
        "GetAgreementTerms",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reporting the terms of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["acceptedTerms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|accepted| {
            let object = accepted
                .as_object()
                .unwrap_or_else(|| panic!("an accepted term is a union object, got {accepted:?}"));
            let (member, payload) = object
                .iter()
                .next()
                .unwrap_or_else(|| panic!("an accepted term carries one member, got {accepted:?}"));
            (
                member.clone(),
                payload["id"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The ids of the charges `agreement` holds.
async fn charge_ids(router: &MultiServiceRouter, agreement: &str) -> BTreeSet<String> {
    let answer = charges(router, json!({"agreementId": agreement})).await;
    assert_eq!(
        answer.status, 200,
        "listing the charges of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    items(&answer)
        .iter()
        .map(|charge| charge["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `(status, statusReasonCode, licenseArn)` of every entitlement
/// `agreement` granted.
async fn entitlements_of(
    router: &MultiServiceRouter,
    agreement: &str,
) -> Vec<(String, String, String)> {
    let answer = call(
        router,
        "GetAgreementEntitlements",
        json!({"agreementId": agreement}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reading the entitlements of {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementEntitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entitlement| {
            let member = |name: &str| {
                entitlement[name]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            };
            (
                member("status"),
                member("statusReasonCode"),
                member("licenseArn"),
            )
        })
        .collect()
}

/// What `DescribeAgreement` reports for `agreement`, which must succeed.
async fn described(router: &MultiServiceRouter, agreement: &str) -> Answer {
    let answer = describe(router, agreement).await;
    assert_eq!(
        answer.status, 200,
        "describing {agreement} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

/// The id of one reported agreement.
fn reported_id(reported: &Value) -> String {
    reported["agreementId"]
        .as_str()
        .unwrap_or_else(|| panic!("an agreement id must be reported, got {reported:?}"))
        .to_string()
}

/// Every agreement of a chain carries the status its own end date made due, and
/// each renewal the clock owed exists, continuing the chain from where its
/// source ended.
#[tokio::test]
async fn the_clock_renews_an_agreement_whose_terms_renew_it_at_its_end_date() {
    for row in renewing_cases() {
        let case = format!("{} ({:?} ago)", row.case, row.signed_ago);
        // One emulator per row, so the searches below report this row's chain
        // only.
        let (router, engine) = emulator_over_engine();
        let source = aged_agreement(
            &router,
            &engine,
            "ap-renewal",
            row.requested.clone(),
            row.signed_ago,
        )
        .await;

        let links = chain_of(&router, &source).await;
        assert_eq!(
            links.len(),
            row.statuses.len(),
            "case {case:?}: expected a chain of {} agreement(s), got {links:?}",
            row.statuses.len()
        );
        assert_eq!(
            reported_id(&links[0]),
            source,
            "case {case:?}: the chain starts at the agreement that was accepted, got {links:?}"
        );

        for (index, (link, expected)) in links.iter().zip(&row.statuses).enumerate() {
            let id = reported_id(link);
            let case = format!("{case}: link {index} ({id})");
            assert_eq!(
                link["status"].as_str(),
                Some(*expected),
                "case {case:?}: expected status {expected}, got {link:?}"
            );
            // "It stays the same across all agreements in that chain, which lets
            // you trace an agreement back to the original."
            assert_eq!(
                link["initialAgreementId"].as_str(),
                Some(source.as_str()),
                "case {case:?}: every link carries the chain's first id, got {link:?}"
            );

            let described = described(&router, &id).await;
            assert_eq!(
                described.body["status"].as_str(),
                Some(*expected),
                "case {case:?}: the describe must report the status the summary does, got {:?}",
                described.body
            );
            assert_eq!(
                described.body["initialAgreementId"].as_str(),
                Some(source.as_str()),
                "case {case:?}: the describe must report the chain's first id, got {:?}",
                described.body
            );

            let start = instant(&described.body, "startTime");
            if index > 0 {
                // "A new agreement is created from the accepted terms of this
                // agreement": it is created AT the source's end date, so that
                // is where it starts and when it was accepted.
                let previous_end = instant(&links[index - 1], "endTime");
                assert!(
                    (start - previous_end).abs() < INSTANT_TOLERANCE,
                    "case {case:?}: a renewal starts where its source ended ({previous_end}), got \
                     {start}"
                );
                assert!(
                    (instant(&described.body, "acceptanceTime") - start).abs()
                        < INSTANT_TOLERANCE,
                    "case {case:?}: a renewal was accepted when it started ({start}), got {:?}",
                    described.body
                );
            }
            // The transition out of `ACTIVE` is the clock's, so a link that has
            // left it is dated at the end date it reached rather than at the
            // instant somebody looked; one still inside its window carries the
            // instant it was created at.
            let due = if *expected == ACTIVE {
                start
            } else {
                instant(&described.body, "endTime")
            };
            assert!(
                (instant(link, "lastUpdateTime") - due).abs() < INSTANT_TOLERANCE,
                "case {case:?}: the link is dated at the instant it was due ({due}), got {link:?}"
            );
            match row.window {
                Some(window) => {
                    let end = instant(&described.body, "endTime");
                    assert!(
                        (end - start - window.as_secs_f64()).abs() < INSTANT_TOLERANCE,
                        "case {case:?}: every link runs for {window:?}, got {start} to {end}"
                    );
                }
                None => assert!(
                    described.body["endTime"].is_null(),
                    "case {case:?}: terms that commit no duration give no end date, got {:?}",
                    described.body
                ),
            }
        }

        // What the LAST link of the chain does at its own end date: the renewal
        // limit is what turns a renewing chain into an expiring agreement.
        let last = links.last().expect("a chain has at least one link");
        assert_eq!(
            last["endTimeBehaviorType"].as_str(),
            row.behavior.map(|(kind, _)| kind),
            "case {case:?}: expected endTimeBehaviorType {:?}, got {last:?}",
            row.behavior.map(|(kind, _)| kind)
        );
        assert_eq!(
            last["endTimeBehaviorReasonCode"].as_str(),
            row.behavior.and_then(|(_, reason)| reason),
            "case {case:?}: expected endTimeBehaviorReasonCode {:?}, got {last:?}",
            row.behavior.and_then(|(_, reason)| reason)
        );

        // The `Status` filter selects on the status the clock left each link in.
        let renewed: BTreeSet<String> = links
            .iter()
            .zip(&row.statuses)
            .filter(|(_, status)| **status == RENEWED)
            .map(|(link, _)| reported_id(link))
            .collect();
        assert_eq!(
            with_status(&router, RENEWED).await,
            renewed,
            "case {case:?}: a search for {RENEWED} selects exactly the links that were renewed"
        );

        // Nothing here is caused by the reading: the chain a second round of the
        // same reads reports is the same chain, link for link.
        let again = chain_of(&router, &source).await;
        assert_eq!(
            again, links,
            "case {case:?}: reading the chain must not change it, got {again:?}"
        );
    }
}

/// A renewal is a whole agreement: it re-states the accepted terms of the
/// agreement it continues, and holds charges, an invoice and entitlements of its
/// own rather than the source's.
#[tokio::test]
async fn a_renewal_holds_the_accepted_terms_with_its_own_charges_invoice_and_entitlements() {
    let (router, engine) = emulator_over_engine();
    let source = aged_agreement(
        &router,
        &engine,
        "ap-renewalContents",
        renewing_terms("P1M"),
        days(31),
    )
    .await;

    let links = chain_of(&router, &source).await;
    assert_eq!(
        links.len(),
        2,
        "a purchase a day past its first month is one renewal in, got {links:?}"
    );
    let renewal = reported_id(&links[1]);

    // "A new agreement is created from the accepted terms of this agreement":
    // the same terms, each under the `TermID` that is "common between offer and
    // agreement".
    let accepted = terms_of(&router, &source).await;
    assert_eq!(
        terms_of(&router, &renewal).await,
        accepted,
        "the renewal must hold the accepted terms of the agreement it renews ({accepted:?})"
    );

    // Its charges are its own: a charge is "the unique identifier of the charge
    // for a given agreement", so no charge of the source is reported under the
    // renewal.
    let source_charges = charge_ids(&router, &source).await;
    let renewal_charges = charge_ids(&router, &renewal).await;
    assert!(
        !renewal_charges.is_empty(),
        "the renewal's prepaid term is charged, so it holds charges, got {renewal_charges:?}"
    );
    assert!(
        renewal_charges.is_disjoint(&source_charges),
        "the renewal holds charges of its own, got {renewal_charges:?} against {source_charges:?}"
    );

    let source_invoices: BTreeSet<String> = invoices_of(&router, &source).await.into_iter().collect();
    let renewal_invoices: BTreeSet<String> =
        invoices_of(&router, &renewal).await.into_iter().collect();
    assert!(
        !renewal_invoices.is_empty(),
        "a prepaid charge is billed on acceptance, so the renewal was invoiced, got \
         {renewal_invoices:?}"
    );
    assert!(
        renewal_invoices.is_disjoint(&source_invoices),
        "the renewal's invoices are its own, got {renewal_invoices:?} against {source_invoices:?}"
    );

    // An entitlement follows the agreement that granted it: the source's are
    // deprovisioned once it is no longer active, and the renewal granted
    // licences of its own.
    let source_entitlements = entitlements_of(&router, &source).await;
    assert!(
        !source_entitlements.is_empty(),
        "the source granted entitlements, got {source_entitlements:?}"
    );
    for (status, reason_code, _) in &source_entitlements {
        assert_eq!(
            (status.as_str(), reason_code.as_str()),
            ("DEPROVISIONED", "AGREEMENT_INACTIVE"),
            "a renewed agreement entitles its acceptor to nothing, got \
             {source_entitlements:?}"
        );
    }
    let renewal_entitlements = entitlements_of(&router, &renewal).await;
    assert_eq!(
        renewal_entitlements.len(),
        source_entitlements.len(),
        "the renewal is entitled to the same resources, got {renewal_entitlements:?}"
    );
    for (status, reason_code, _) in &renewal_entitlements {
        assert_eq!(
            (status.as_str(), reason_code.as_str()),
            ("PROVISIONED", "AGREEMENT_ACTIVE"),
            "the renewal is active, so its entitlements are provisioned, got \
             {renewal_entitlements:?}"
        );
    }
    let licenses = |entitlements: &[(String, String, String)]| -> BTreeSet<String> {
        entitlements
            .iter()
            .map(|(_, _, license)| license.clone())
            .collect()
    };
    let renewal_licenses = licenses(&renewal_entitlements);
    assert!(
        renewal_licenses.is_disjoint(&licenses(&source_entitlements)),
        "the renewal granted licences of its own, got {renewal_licenses:?}"
    );

    // The source's own record is untouched but for the status and the instant
    // the transition was due at: its end date is still the date it reached.
    let source_summary = &links[0];
    assert!(
        (instant(source_summary, "lastUpdateTime") - instant(source_summary, "endTime")).abs()
            < INSTANT_TOLERANCE,
        "the renewal is dated at the end date it was due at, got {source_summary:?}"
    );
    // `DescribeAgreementOutput$endTimeBehavior` "is also not present for an
    // agreement that is no longer active".
    let described = described(&router, &source).await;
    assert!(
        described.body["endTimeBehavior"].is_null(),
        "a renewed agreement is no longer active, so it reports no endTimeBehavior, got {:?}",
        described.body
    );
}

/// The renewal the clock owes is materialised by a reader that was never handed
/// the agreement's id, and only once however many times it is read.
#[tokio::test]
async fn the_renewal_is_the_clocks_and_happens_once() {
    let (router, engine) = emulator_over_engine();
    let source = aged_agreement(
        &router,
        &engine,
        "ap-renewalOnce",
        renewing_terms("P1M"),
        days(31),
    )
    .await;

    // Read first through the SEARCH, so the renewal is materialised by a
    // reporting operation that was never handed the agreement's id.
    let renewed = with_status(&router, RENEWED).await;
    assert!(
        renewed.contains(&source),
        "a search must report the agreement the clock renewed, got {renewed:?}"
    );

    let links = chain_of(&router, &source).await;
    assert_eq!(
        links.len(),
        2,
        "the search materialised the renewal the clock owed, got {links:?}"
    );

    // Every operation that reports or guards on the status settles the same end
    // date, and none of them mints a second renewal.
    for attempt in 1..=2 {
        let described = described(&router, &source).await;
        assert_eq!(
            described.body["status"].as_str(),
            Some(RENEWED),
            "attempt {attempt}: expected {RENEWED}, got {:?}",
            described.body
        );
        let _ = entitlements_of(&router, &source).await;
        let _ = charge_ids(&router, &source).await;
        let again = chain_of(&router, &source).await;
        assert_eq!(
            again, links,
            "attempt {attempt}: the renewal happens once, got {again:?}"
        );
    }

    // The agreement the renewal left behind is no longer one an acceptor can
    // cancel: `CancelAgreement` "allows an acceptor to cancel an active
    // agreement".
    let cancelled = call(
        &router,
        "CancelAgreement",
        json!({"agreementId": source.clone()}),
    )
    .await;
    assert_eq!(
        cancelled.status, 400,
        "cancelling a renewed agreement is refused, got {} with {:?}",
        cancelled.status, cancelled.body
    );
    assert_eq!(
        cancelled.reason(),
        "INACTIVE_AGREEMENT",
        "the refusal names the status, got {:?}",
        cancelled.body
    );
    let again = chain_of(&router, &source).await;
    assert_eq!(
        again, links,
        "a refused call changes nothing about the chain, got {again:?}"
    );
}

/// The limit these rows count renewals against is the one the accepted renewal
/// term publishes, read back over the wire.
#[tokio::test]
async fn the_renewal_term_publishes_the_limit_these_rows_count_against() {
    let (router, engine) = emulator_over_engine();
    let source = aged_agreement(
        &router,
        &engine,
        "ap-renewalLimit",
        renewing_terms("P1M"),
        Duration::from_secs(1),
    )
    .await;

    let answer = call(
        &router,
        "GetAgreementTerms",
        json!({"agreementId": source}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reporting the terms of {source} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    let published = answer.body["acceptedTerms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .find_map(|accepted| accepted.pointer("/renewalTerm/maxRenewals").cloned())
        .unwrap_or_else(|| {
            panic!(
                "the accepted renewal term reports its maxRenewals, got {:?}",
                answer.body
            )
        });
    assert_eq!(
        published.as_i64(),
        Some(PUBLISHED_MAX_RENEWALS),
        "the rows count against the limit the renewal term publishes, got {published:?}"
    );
}
