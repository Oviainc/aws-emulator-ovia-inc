// SPDX-License-Identifier: CC0-1.0

//! An agreement that has reached its own end date, end to end over HTTP.
//!
//! Every report and every refusal here goes through the router, so each case
//! exercises routing, deserialization, the handler, the persisted state and
//! serialization — the same path a client takes.
//!
//! The rule under test: a status transition an agreement's own stored instant has
//! already made due is materialised by the operations that report or guard on
//! that status, so the status a caller reads is the one the clock owed the
//! agreement rather than the one the last call left behind. For the end date the
//! model names both halves: `endTime` is "the date and time when the agreement
//! ends" — absent, and so never due, "for pay-as-you-go agreements, which don't
//! have end dates" — and the status reaching it carries is "EXPIRED – The
//! agreement ended on the defined end date"
//! (`smithy_query(query="doc", shape="DescribeAgreementOutput", member="status")`).
//! Which end a date is depends on the behaviour at it: `EXPIRE` is "the agreement
//! ends and isn't renewed or replaced", while `RENEW` creates "a new agreement …
//! from the accepted terms of this agreement" and leaves this one `RENEWED`, a
//! renewal this service does not create — so an agreement whose acceptor enabled
//! auto-renewal is not ended by its end date here.
//!
//! Everything the status decides follows from it in the same call: the
//! `endTimeBehavior` `DescribeAgreement` reports only "for every active
//! agreement", the `Status` filter of `SearchAgreements` and the `status` of its
//! `AgreementViewSummary`, the state of the entitlements the agreement granted
//! ("DEPROVISIONED … AGREEMENT_INACTIVE"), and the `INACTIVE_AGREEMENT` refusal
//! every operation that acts only on an active agreement answers.
//!
//! The rows differ along what the rule generalises over: which contract length
//! the acceptor purchased (every length the rate card publishes), how far past
//! its end date the agreement is (a day, the exact instant it is due, one second
//! past it), how far short of it (one second, a day, not aged at all), whether it
//! has an end date at all, and what its terms do at that date (no renewal term,
//! auto-renewal enabled, auto-renewal opted out).
//!
//! The clock cannot be wound forward 30 days inside a test, so the agreement's
//! own stored window is wound BACK instead ([`signed_ago`]): a record whose
//! instants all sit `d` earlier is exactly what a handler sees once `d` has
//! passed, and the handlers read the wall clock. The harness
//! (`emulator_over_engine`, `accept`, `call`, `cancel`, `create`, `describe`,
//! `search`, `term`, `upfront_term`, `renewal_term`, `variable_payment_term`,
//! `with_agreement_type`) is the one `agreement_lifecycle` owns, the refusal
//! envelope is the one `error_bodies` owns, and the operations that require an
//! active agreement are the set `inactive_agreement_refusals` owns.

use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::state::service_state;
use smithy_server_runtime::MultiServiceRouter;

use crate::agreement_lifecycle::{
    accept, agreement_type_filter, call, cancel, create, describe, emulator_over_engine,
    new_request, renewal_term, search, term, upfront_term, variable_payment_term,
    with_agreement_type,
};
use crate::error_bodies::assert_input_fault;
use crate::inactive_agreement_refusals::{
    request_for, AGREEMENT_ID, INACTIVE_AGREEMENT, NEEDS_AN_ACTIVE_AGREEMENT, PAYMENT_TERM,
};

/// `AgreementStatus` values this suite expects, from
/// `smithy_query(query="shape", shape="AgreementStatus")`: "ACTIVE – The terms of
/// the agreement are active", "EXPIRED – The agreement ended on the defined end
/// date" and "CANCELLED – The acceptor ended the agreement before the defined end
/// date". Spelled out here rather than imported from the crate, so a handler
/// reporting another value fails these rows instead of agreeing with them.
const ACTIVE: &str = "ACTIVE";
/// See [`ACTIVE`].
const EXPIRED: &str = "EXPIRED";
/// See [`ACTIVE`].
const CANCELLED: &str = "CANCELLED";

/// `AgreementEntitlementStatus` and `AgreementEntitlementStatusReasonCode` values
/// an entitlement of an agreement reports, from
/// `smithy_query(query="shape", shape="AgreementEntitlementStatus")` and
/// `…="AgreementEntitlementStatusReasonCode"`.
const PROVISIONED: &str = "PROVISIONED";
/// See [`PROVISIONED`].
const DEPROVISIONED: &str = "DEPROVISIONED";
/// See [`PROVISIONED`].
const AGREEMENT_ACTIVE: &str = "AGREEMENT_ACTIVE";
/// See [`PROVISIONED`].
const AGREEMENT_INACTIVE: &str = "AGREEMENT_INACTIVE";

/// The contract lengths the published rate card prices a configured prepaid
/// purchase at, each with how long the agreement then runs, in days.
///
/// SOURCE: the selectors the emulator's rate card publishes
/// (`ConfigurableUpfrontRateCardItem$selector` "differentiates between the
/// mutually exclusive rate cards in the same pricing term to be selected by the
/// buyer"), spelled out here rather than imported so a handler publishing a
/// different set fails these rows — the same table `agreement_end_dates` pins the
/// end dates themselves with.
const CONTRACT_LENGTHS: &[(&str, u64)] = &[
    ("P1M", 30),
    ("P12M", 365),
    ("P24M", 730),
    ("P36M", 1095),
];

/// One day, the unit [`CONTRACT_LENGTHS`] measures a purchase in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// `days` of a contract length, as a duration.
fn days(days: u64) -> Duration {
    DAY * u32::try_from(days).expect("a published contract length fits in days")
}

/// Move every instant the agreement `agreement_id` stored `ago` earlier, so the
/// agreement reads as one signed that long before now.
///
/// This is the only in-process step in this suite: the handlers read the wall
/// clock, which a test cannot wind forward past a 30-day purchase, and a record
/// whose own instants all sit `ago` earlier is the same thing seen from the
/// handler's side. Nothing about the agreement's status, its behaviour or its
/// entitlements is touched — those are what the rows then read back over HTTP.
fn signed_ago(engine: &engine::InMemoryEngine, agreement_id: &str, ago: Duration) {
    let state = service_state(engine).expect("the emulator's own engine carries this store");
    let mut store = state.lock();
    let agreement = store
        .agreements
        .get_mut(agreement_id)
        .unwrap_or_else(|| panic!("the accepted agreement {agreement_id} must be stored"));
    agreement.start_time -= ago;
    agreement.acceptance_time -= ago;
    agreement.updated_at -= ago;
    agreement.end_time = agreement.end_time.map(|end| end - ago);
    for entitlement in &mut agreement.entitlements {
        entitlement.provisioned_at -= ago;
        entitlement.registration_token_expires_at -= ago;
    }
}

/// Now, in epoch seconds, as the wire spells an instant — for checking that an
/// end date a row calls past really is.
fn now_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is past the epoch")
        .as_secs_f64()
}

/// Create and accept one agreement over `terms`, then wind its window back by
/// `ago`, answering its agreement id.
async fn aged_agreement(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    proposal: &str,
    terms: Vec<Value>,
    ago: Duration,
) -> String {
    let created = create(router, new_request(proposal, terms)).await;
    assert_eq!(
        created.status, 200,
        "creating the agreement request must succeed, got {} with {:?}",
        created.status, created.body
    );
    let accepted = accept(
        router,
        json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "accepting the agreement request must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    let agreement_id = accepted.str_at("/agreementId");
    signed_ago(engine, &agreement_id, ago);
    agreement_id
}

/// The entitlement states one agreement's entitlements report, as
/// `(status, statusReasonCode)` pairs in the order they are reported.
async fn entitlement_states(
    router: &MultiServiceRouter,
    agreement_id: &str,
) -> Vec<(String, String)> {
    let answer = call(
        router,
        "GetAgreementEntitlements",
        json!({"agreementId": agreement_id}),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "reading the entitlements of {agreement_id} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementEntitlements"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entitlement| {
            (
                entitlement["status"].as_str().unwrap_or_default().to_string(),
                entitlement["statusReasonCode"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// The `AgreementViewSummary` one search reports for `agreement_id`, which must
/// be reported.
async fn summary_of(router: &MultiServiceRouter, agreement_id: &str) -> Value {
    let answer = search(router, json!({"filters": with_agreement_type(&json!([]))})).await;
    assert_eq!(
        answer.status, 200,
        "searching for {agreement_id} must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["agreementViewSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|summary| summary["agreementId"].as_str() == Some(agreement_id))
        .unwrap_or_else(|| {
            panic!("the search must report {agreement_id}, got {:?}", answer.body)
        })
}

/// The agreement ids a search filtered on `status` selects.
async fn with_status(router: &MultiServiceRouter, status: &str) -> BTreeSet<String> {
    let answer = search(
        router,
        json!({"filters": [{"name": "Status", "values": [status]}, agreement_type_filter()]}),
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

/// One row: the terms an acceptor accepted, how long ago the agreement was
/// signed, and the status the clock has therefore left it in.
struct Aged {
    case: &'static str,
    requested: Vec<Value>,
    /// How long before now the agreement's own window sits.
    signed_ago: Duration,
    /// The `AgreementStatus` it reports, given that window.
    status: &'static str,
    /// Whether its accepted terms gave it an end date at all.
    end_dated: bool,
}

/// The `VariablePaymentTerm` every row accepts beside its own terms, so the
/// payment-request row of [`NEEDS_AN_ACTIVE_AGREEMENT`] is answered about the
/// agreement's status rather than about a term the agreement does not carry.
fn payment_term() -> Value {
    variable_payment_term(PAYMENT_TERM)
}

fn aged_cases() -> Vec<Aged> {
    // One row per contract length the rate card publishes: a purchase of that
    // length signed a day more than that long ago is past its own end date.
    let mut cases: Vec<Aged> = CONTRACT_LENGTHS
        .iter()
        .map(|(length, length_days)| Aged {
            case: "a purchase signed a day past the end of the length it bought",
            requested: vec![upfront_term("cfg", length, &[("Users", 2)]), payment_term()],
            signed_ago: days(length_days + 1),
            status: EXPIRED,
            end_dated: true,
        })
        .collect();
    cases.extend([
        Aged {
            case: "signed exactly its contract length ago: the end date is due at that instant",
            requested: vec![upfront_term("cfg", "P1M", &[("Users", 1)]), payment_term()],
            signed_ago: days(30),
            status: EXPIRED,
            end_dated: true,
        },
        Aged {
            case: "one second past its end date",
            requested: vec![upfront_term("cfg", "P1M", &[("Users", 1)]), payment_term()],
            signed_ago: days(30) + Duration::from_secs(1),
            status: EXPIRED,
            end_dated: true,
        },
        Aged {
            case: "one second short of its end date",
            requested: vec![upfront_term("cfg", "P1M", &[("Users", 1)]), payment_term()],
            signed_ago: days(30) - Duration::from_secs(1),
            status: ACTIVE,
            end_dated: true,
        },
        Aged {
            case: "a day short of the end of the year it bought",
            requested: vec![upfront_term("cfg", "P12M", &[("Users", 1)]), payment_term()],
            signed_ago: days(364),
            status: ACTIVE,
            end_dated: true,
        },
        Aged {
            case: "signed a moment ago, with its whole window ahead of it",
            requested: vec![upfront_term("cfg", "P36M", &[("Users", 1)]), payment_term()],
            // A second, rather than nothing at all, so the entitlements the
            // acceptance provisioned are past the provisioning span the service
            // holds them `PENDING` for (`state::ENTITLEMENT_PROVISIONING`,
            // 500 ms) and this row is about the agreement's end date alone.
            signed_ago: Duration::from_secs(1),
            status: ACTIVE,
            end_dated: true,
        },
        Aged {
            case: "metered usage: a pay-as-you-go agreement has no end date to reach",
            requested: vec![term("UsageBasedPricingTerm"), payment_term()],
            signed_ago: days(10 * 365),
            status: ACTIVE,
            end_dated: false,
        },
        Aged {
            case: "auto-renewal enabled: its end date renews the agreement rather than ending it",
            requested: vec![
                upfront_term("cfg", "P1M", &[("Users", 1)]),
                renewal_term("renew", true),
                payment_term(),
            ],
            signed_ago: days(31),
            status: ACTIVE,
            end_dated: true,
        },
        Aged {
            case: "auto-renewal opted out of: its end date ends the agreement",
            requested: vec![
                upfront_term("cfg", "P1M", &[("Users", 1)]),
                renewal_term("renew", false),
                payment_term(),
            ],
            signed_ago: days(31),
            status: EXPIRED,
            end_dated: true,
        },
    ]);
    cases
}

/// An agreement reports the status its own stored end date has already made due:
/// `EXPIRED` once that date has passed and the agreement's terms do not renew it
/// at it, and `ACTIVE` while it has not — and everything the status decides is
/// reported from the same state in the same call.
#[tokio::test]
async fn an_agreement_reports_the_status_its_end_date_has_made_due() {
    for row in aged_cases() {
        let case = format!("{} ({:?} ago)", row.case, row.signed_ago);
        // One emulator per row, so the searches below report this row's
        // agreement only.
        let (router, engine) = emulator_over_engine();
        let agreement_id = aged_agreement(
            &router,
            &engine,
            "ap-expiry",
            row.requested.clone(),
            row.signed_ago,
        )
        .await;

        let described = describe(&router, &agreement_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: describing the agreement must succeed, got {} with {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.body["status"].as_str(),
            Some(row.status),
            "case {case:?}: expected status {:?}, got {:?}",
            row.status,
            described.body
        );

        // The end date itself is untouched by the transition it caused: the
        // agreement still reports the instant its terms committed it until.
        let end_time = described.body["endTime"].as_f64();
        assert_eq!(
            end_time.is_some(),
            row.end_dated,
            "case {case:?}: expected an end date present={}, got {:?}",
            row.end_dated,
            described.body
        );
        if row.status == EXPIRED {
            let end = end_time.unwrap_or_else(|| {
                panic!("case {case:?}: an expired agreement reached an end date, got {:?}", described.body)
            });
            assert!(
                end <= now_epoch_secs(),
                "case {case:?}: the end date an expired agreement reached is past, got {end} in \
                 {:?}",
                described.body
            );
        }

        // "This field is present for every active agreement that has an end
        // date… It is also not present for an agreement that is no longer
        // active."
        let behaves = described.body["endTimeBehavior"].is_null();
        assert_eq!(
            behaves,
            row.status != ACTIVE || !row.end_dated,
            "case {case:?}: endTimeBehavior is reported for an active, end-dated agreement and \
             for no other, got {:?}",
            described.body
        );

        // The search projection reports the same status, and the `Status`
        // filter selects on it.
        let summary = summary_of(&router, &agreement_id).await;
        assert_eq!(
            summary["status"].as_str(),
            Some(row.status),
            "case {case:?}: the summary must report the status the describe does, got {summary:?}"
        );
        let selected = with_status(&router, row.status).await;
        assert!(
            selected.contains(&agreement_id),
            "case {case:?}: a search for {:?} must select {agreement_id}, got {selected:?}",
            row.status
        );
        for other in [ACTIVE, EXPIRED, CANCELLED] {
            if other == row.status {
                continue;
            }
            let selected = with_status(&router, other).await;
            assert!(
                !selected.contains(&agreement_id),
                "case {case:?}: a search for {other:?} must not select {agreement_id}, got \
                 {selected:?}"
            );
        }

        // An entitlement follows the agreement that granted it: "DEPROVISIONED"
        // / "AGREEMENT_INACTIVE" once the agreement is no longer active, and
        // "PROVISIONED" / "AGREEMENT_ACTIVE" while it is and provisioning is
        // behind it.
        let expected_state = if row.status == ACTIVE {
            (PROVISIONED, AGREEMENT_ACTIVE)
        } else {
            (DEPROVISIONED, AGREEMENT_INACTIVE)
        };
        let states = entitlement_states(&router, &agreement_id).await;
        assert!(
            !states.is_empty(),
            "case {case:?}: accepting the agreement provisioned entitlements to read, got \
             {states:?}"
        );
        for (status, reason_code) in &states {
            assert_eq!(
                (status.as_str(), reason_code.as_str()),
                expected_state,
                "case {case:?}: expected the entitlements in {expected_state:?}, got {states:?}"
            );
        }

        // What the status DECIDES: every operation that acts only on an active
        // agreement answers the modelled input fault for one the clock has
        // ended, and serves one it has not.
        //
        // `CancelAgreement` is called LAST, because on an agreement still
        // inside its window it is what takes it out — the sort puts the one
        // destructive call after the others, whatever order the shared list is
        // written in.
        let mut operations: Vec<&str> = NEEDS_AN_ACTIVE_AGREEMENT.to_vec();
        operations.sort_by_key(|operation| *operation == "CancelAgreement");
        for operation in operations {
            let answer = call(&router, operation, request_for(operation, &agreement_id)).await;
            let case = format!("{operation} on an agreement that is {}: {case}", row.status);
            if row.status == ACTIVE {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: an agreement still inside its window must be served, got {} \
                     with {:?}",
                    answer.status, answer.body
                );
            } else {
                assert_input_fault(&case, INACTIVE_AGREEMENT, AGREEMENT_ID, &answer);
            }
        }
    }
}

/// The transition is the clock's, not the reader's: it is dated at the end date
/// the agreement reached rather than at the instant somebody looked, two reads
/// report the same thing, and a read is not what causes it — a store nobody
/// describes still answers the search with the expired agreement.
#[tokio::test]
async fn the_expiry_is_dated_at_the_end_date_and_is_stable_across_reads() {
    let (router, engine) = emulator_over_engine();
    let agreement_id = aged_agreement(
        &router,
        &engine,
        "ap-dated",
        vec![upfront_term("cfg", "P1M", &[("Users", 1)]), payment_term()],
        days(45),
    )
    .await;

    // Read first through the SEARCH, so the expiry is materialised by a
    // reporting operation that was never handed the agreement's id.
    let selected = with_status(&router, EXPIRED).await;
    assert!(
        selected.contains(&agreement_id),
        "a search must report the agreement the clock expired, got {selected:?}"
    );

    let summary = summary_of(&router, &agreement_id).await;
    let end_time = summary["endTime"].as_f64().unwrap_or_else(|| {
        panic!("the purchase has an end date, got {summary:?}");
    });
    assert_eq!(
        summary["lastUpdateTime"].as_f64(),
        Some(end_time),
        "the expiry is dated at the end date it was due at, got {summary:?}"
    );

    for attempt in 1..=2 {
        let described = describe(&router, &agreement_id).await;
        assert_eq!(
            described.body["status"].as_str(),
            Some(EXPIRED),
            "attempt {attempt}: expected {EXPIRED}, got {:?}",
            described.body
        );
        assert_eq!(
            described.body["endTime"].as_f64(),
            Some(end_time),
            "attempt {attempt}: a read must not move the end date, got {:?}",
            described.body
        );
        let summary = summary_of(&router, &agreement_id).await;
        assert_eq!(
            summary["lastUpdateTime"].as_f64(),
            Some(end_time),
            "attempt {attempt}: a read must not re-date the expiry, got {summary:?}"
        );
    }
}

/// An agreement that left `ACTIVE` before its end date keeps the status it
/// reached: the clock does not move a terminal status, whichever one it is, so
/// the end date of a cancelled or replaced agreement expires nothing.
#[tokio::test]
async fn a_terminal_status_reached_before_the_end_date_is_kept() {
    let (router, engine) = emulator_over_engine();
    let terms = vec![upfront_term("cfg", "P1M", &[("Users", 1)]), payment_term()];

    // Cancelled by the acceptor while it was still inside its window, then aged
    // past its end date.
    let cancelled = aged_agreement(&router, &engine, "ap-cancelled", terms.clone(), Duration::ZERO)
        .await;
    let answer = cancel(&router, &cancelled).await;
    assert_eq!(
        answer.status, 200,
        "cancelling the active agreement must succeed, got {} with {:?}",
        answer.status, answer.body
    );
    signed_ago(&engine, &cancelled, days(45));

    // Superseded by a replacement while it was still inside its window, then
    // aged past its end date.
    let replaced = aged_agreement(&router, &engine, "ap-replaced", terms, Duration::ZERO).await;
    let created = create(
        &router,
        json!({
            "intent": "REPLACE",
            "agreementProposalIdentifier": "ap-replacement",
            "sourceAgreementIdentifier": replaced,
            "requestedTerms": [term("eula.v2")],
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the replacement request must be created, got {} with {:?}",
        created.status, created.body
    );
    let accepted = accept(
        &router,
        json!({"agreementRequestId": created.str_at("/agreementRequestId")}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "accepting the replacement must succeed, got {} with {:?}",
        accepted.status, accepted.body
    );
    signed_ago(&engine, &replaced, days(45));

    for (case, agreement_id, status) in [
        ("cancelled before its end date", &cancelled, CANCELLED),
        ("replaced before its end date", &replaced, "REPLACED"),
    ] {
        let described = describe(&router, agreement_id).await;
        assert_eq!(
            described.body["status"].as_str(),
            Some(status),
            "case {case:?}: expected the status it reached ({status}), got {:?}",
            described.body
        );
        let selected = with_status(&router, EXPIRED).await;
        assert!(
            !selected.contains(agreement_id),
            "case {case:?}: a search for {EXPIRED} must not select {agreement_id}, got \
             {selected:?}"
        );
    }
}
