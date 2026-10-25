// SPDX-License-Identifier: CC0-1.0

//! The account gate on the account a client actually meets, end to end over the wire.
//!
//! The rule under test:
//!
//! > An account-level prerequisite is a fact about the account held in this service's
//! > store, and an emulated account starts without the ones nothing has minted for it. The
//! > account's Incident Manager replication set is such a prerequisite, so on a freshly
//! > started emulator EVERY replication-set-scoped operation is refused with its own
//! > modelled `ValidationException` (HTTP 400) BEFORE the subject the request names is
//! > resolved — never with the 404 that would say the account is fine and only the resource
//! > is missing — and writing that record into the store is the one change that makes the
//! > same requests answer about their own subjects.
//!
//! The sibling suites all install the record themselves (`contact_updates::onboard`) and
//! then take it away, so they say nothing about the account a cold emulator serves: they
//! drive `contact_lifecycle::Emulator::new`, whose fixture onboards. This suite drives
//! `Emulator::fresh` — no fixture and no in-process write before the call — so each row's
//! answer is what an SDK or CLI gets from an emulator nobody has touched.
//!
//! The table is `data_plane_precondition::GATED_OPERATIONS`, the same gated surface the
//! refusal suite spans, so the two cannot drift apart; one FRESH emulator per row means no
//! earlier row can have onboarded the account for it.

use serde_json::json;

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    invalid_resource_arn_message, service_state, AccountReplicationSet, ACCOUNT_NOT_FOUND_MESSAGE,
};

use crate::contact_lifecycle::{error_code, Emulator};
use crate::contact_updates::set_replication_set;
use crate::data_plane_precondition::{Targets, GATED_OPERATIONS};

/// How many resources of each kind this emulator's store holds, as `(what, count)` pairs.
///
/// Read straight from the store rather than over the wire, so "the refused call wrote
/// nothing" is asserted about the state itself and not about what a reader chooses to
/// report.
fn resource_counts(emulator: &Emulator) -> Vec<(&'static str, usize)> {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    vec![
        ("contacts", state.contacts.len()),
        ("alias index entries", state.alias_index.len()),
        ("contact channels", state.contact_channels.len()),
        ("rotations", state.rotations.len()),
        ("rotation overrides", state.rotation_overrides.len()),
        ("engagements", state.engagements.len()),
        ("pages", state.pages.len()),
        ("idempotency ledger entries", state.idempotency_tokens.len()),
    ]
}

/// Whether `message` is the ACCOUNT-level refusal, in either wording the gate answers with.
///
/// Read through the crate's own message helpers rather than copied text, so a reworded
/// refusal cannot slip past this suite.
fn refuses_the_account(message: &str) -> bool {
    // `invalid_resource_arn_message("")` is that refusal's prefix, whatever ARN follows it.
    message.contains(ACCOUNT_NOT_FOUND_MESSAGE) || message.contains(&invalid_resource_arn_message(""))
}

/// Every gated operation, as the FIRST request a freshly started emulator ever serves, is
/// refused about the ACCOUNT — not about the resource it names — and writes nothing.
#[tokio::test]
async fn every_gated_operation_is_refused_on_a_cold_emulator() {
    for row in GATED_OPERATIONS {
        let emulator = Emulator::fresh();
        let targets = Targets::absent();
        let case = format!("{} as the first call on a cold emulator", row.operation);

        let (status, body) = emulator.call(row.operation, (row.body)(&targets)).await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected the account-level precondition to answer HTTP 400, got \
             {status} with {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException — the account is not onboarded, \
             so the answer may not be about the resource — got {:?} in {body:?}",
            error_code(&body)
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            refuses_the_account(message),
            "case {case:?}: expected the account-level refusal, got {message:?} in {body:?}"
        );
        for (what, count) in resource_counts(&emulator) {
            assert_eq!(
                count, 0,
                "case {case:?}: a refused call must write nothing, but the store holds \
                 {count} {what}"
            );
        }
        let onboarded = {
            let state =
                service_state(&*emulator.engine).expect("the engine holds this service's state");
            let onboarded = state.lock().replication_set.is_some();
            onboarded
        };
        assert!(
            !onboarded,
            "case {case:?}: a refused call must not onboard the account it refused"
        );
    }
}

/// Installing the account's onboarding record — the one thing no request can do — is what
/// flips each of those same requests to an answer about its OWN subject.
///
/// One fresh emulator per row again, and the request bytes are identical on both sides of
/// the install, so what changed the answer is the account's state and nothing in the
/// request.
#[tokio::test]
async fn the_same_requests_answer_about_their_subject_once_the_account_is_onboarded() {
    for row in GATED_OPERATIONS {
        let emulator = Emulator::fresh();
        let targets = Targets::absent();
        let case = format!("{} before and after the account is onboarded", row.operation);

        let (refused, body) = emulator.call(row.operation, (row.body)(&targets)).await;
        assert_eq!(
            refused, 400,
            "case {case:?}: expected HTTP 400 while the account holds no replication set, \
             got {refused} with {body:?}"
        );

        // The record Incident Manager mints for an onboarded account, and nothing else.
        set_replication_set(&emulator, Some(AccountReplicationSet::onboarded_account()));

        let (status, body) = emulator.call(row.operation, (row.body)(&targets)).await;
        assert_eq!(
            status, row.absent_status,
            "case {case:?}: expected HTTP {} — the answer about the subject the request \
             names — got {status} with {body:?}",
            row.absent_status
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            !refuses_the_account(message),
            "case {case:?}: an onboarded account must not be refused by the account gate, \
             got {message:?} in {body:?}"
        );
        if row.absent_status == 404 {
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected the answer to be about the absent resource, got \
                 {:?} in {body:?}",
                error_code(&body)
            );
        }
    }
}

/// A set that EXISTS but has not finished replicating is not onboarding: the same requests
/// are refused exactly as they are for an account with no set at all.
///
/// This is the third state the account's record can be in, and the reason the gate reads
/// `active` rather than mere presence.
#[tokio::test]
async fn a_replication_set_that_is_not_active_yet_leaves_the_gate_closed() {
    for row in GATED_OPERATIONS {
        let emulator = Emulator::fresh();
        let targets = Targets::absent();
        let case = format!("{} with a set that has not finished replicating", row.operation);

        set_replication_set(
            &emulator,
            Some(AccountReplicationSet {
                active: false,
                ..AccountReplicationSet::onboarded_account()
            }),
        );

        let (status, body) = emulator.call(row.operation, (row.body)(&targets)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 — a set that is not ACTIVE opens nothing — \
             got {status} with {body:?}"
        );
        let message = body["Message"].as_str().unwrap_or_default();
        assert!(
            refuses_the_account(message),
            "case {case:?}: expected the account-level refusal, got {message:?} in {body:?}"
        );
    }
}

/// The precondition-exempt account-wide listings are SERVED on a cold emulator, each with an
/// empty page, while the gated listing beside them is refused.
///
/// That contrast is what makes every 400 above a statement about the account gate rather
/// than about the service being unmounted, unrouted or unimplemented.
#[tokio::test]
async fn the_exempt_listings_are_served_while_the_gated_listing_is_refused() {
    // `ListContacts` and `ListEngagements` resolve no replication-set-scoped ARN and are
    // recorded as served for an account with no set; `ListRotations` is recorded refused.
    let exempt = [
        ("ListContacts", json!({}), "Contacts"),
        (
            "ListContacts",
            json!({"AliasPrefix": "never-created"}),
            "Contacts",
        ),
        ("ListEngagements", json!({}), "Engagements"),
    ];

    for (operation, request, collection) in exempt {
        let emulator = Emulator::fresh();
        let case = format!("{operation} with {request} on a cold emulator");

        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a precondition-exempt listing is served whatever the account's \
             onboarding is, got {status} with {body:?}"
        );
        assert_eq!(
            body[collection].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected an empty page, got {body:?}"
        );
    }

    let emulator = Emulator::fresh();
    let (status, body) = emulator.call("ListRotations", json!({})).await;
    assert_eq!(
        status, 400,
        "case \"ListRotations on a cold emulator\": the rotation listing is NOT exempt, got \
         {status} with {body:?}"
    );
    assert!(
        error_code(&body).contains("ValidationException"),
        "case \"ListRotations on a cold emulator\": expected a ValidationException, got {body:?}"
    );
}
