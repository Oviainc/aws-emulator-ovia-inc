// SPDX-License-Identifier: CC0-1.0

//! The store a freshly started emulator starts with, and what it reports.
//!
//! The rule under test:
//!
//! > The emulated account owns exactly what something created for it: every resource map
//! > starts empty, every counter at zero, and the account-level onboarding record — its
//! > Incident Manager replication set, which only Incident Manager mints — starts absent.
//! > A read of that account reports the empty account; nothing is invented to make it look
//! > used or onboarded.
//!
//! `tests/all/cold_account_gate.rs` is the suite about what the gated operations ANSWER on
//! such an account; this one is about the state they answer from, asserted both through the
//! store and over the wire.

use serde_json::json;

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::service_state;

use crate::contact_lifecycle::Emulator;

/// The store a cold emulator starts with: no onboarding record, and no resource of any
/// kind.
#[tokio::test]
async fn the_initial_store_holds_nothing_at_all() {
    let emulator = Emulator::fresh();
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();

    assert!(
        state.replication_set.is_none(),
        "case \"the account's onboarding record\": a replication set is Incident Manager's \
         own resource, minted for an account rather than invented here, got {:?}",
        state.replication_set
    );
    assert!(
        state.active_replication_set().is_none(),
        "case \"the account's ACTIVE onboarding record\": expected none, got {:?}",
        state.active_replication_set()
    );

    for (what, count) in [
        ("contacts", state.contacts.len()),
        ("alias index entries", state.alias_index.len()),
        ("contact channels", state.contact_channels.len()),
        ("rotations", state.rotations.len()),
        ("rotation overrides", state.rotation_overrides.len()),
        ("engagements", state.engagements.len()),
        ("pages", state.pages.len()),
        ("idempotency ledger entries", state.idempotency_tokens.len()),
    ] {
        assert_eq!(
            count, 0,
            "case {what:?}: a fresh account owns no resources, got {count}"
        );
    }
    for (what, counter) in [
        ("channels_created", state.channels_created),
        ("rotations_created", state.rotations_created),
        ("rotation_overrides_created", state.rotation_overrides_created),
        ("activation_codes_issued", state.activation_codes_issued),
        ("engagements_started", state.engagements_started),
        ("pages_sent", state.pages_sent),
    ] {
        assert_eq!(
            counter, 0,
            "case {what:?}: a fresh account has minted nothing, got {counter}"
        );
    }
}

/// And over the wire: the listings a cold emulator serves report that empty account, page
/// after page, with no `NextToken` to follow.
#[tokio::test]
async fn the_cold_account_reports_itself_empty() {
    // The account-wide listings that resolve no replication-set-scoped ARN, over the filter
    // shapes each admits: an unfiltered page, a filtered one, and a bounded one.
    let cases = [
        ("ListContacts", json!({}), "Contacts"),
        ("ListContacts", json!({"AliasPrefix": "oncall"}), "Contacts"),
        ("ListContacts", json!({"MaxResults": 1}), "Contacts"),
        ("ListEngagements", json!({}), "Engagements"),
        ("ListEngagements", json!({"MaxResults": 1}), "Engagements"),
    ];

    for (operation, request, collection) in cases {
        let emulator = Emulator::fresh();
        let case = format!("{operation} with {request}");

        let (status, body) = emulator.call(operation, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a served listing on a cold emulator, got {status} with \
             {body:?}"
        );
        assert_eq!(
            body[collection].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: expected an empty page, got {body:?}"
        );
        assert!(
            body["NextToken"].is_null(),
            "case {case:?}: an empty page is the last page, so it carries no NextToken, got \
             {body:?}"
        );
    }
}
