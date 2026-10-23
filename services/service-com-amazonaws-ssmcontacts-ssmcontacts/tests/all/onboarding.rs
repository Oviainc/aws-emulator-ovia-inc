// SPDX-License-Identifier: CC0-1.0

//! What ONBOARDING changes, end to end over the wire.
//!
//! `tests/all/replication_set_precondition.rs` asserts that every registry write is
//! refused while the account has no ACTIVE Incident Manager replication set. This suite
//! asserts the other half of the same rule: that the account's replication set is the ONLY
//! thing that flips the answer, and that what a serviced write persisted is really there.
//!
//! One unchanged request is sent twice over one engine, with nothing between the two calls
//! but the set, so the difference cannot be attributed to the request; then the contact a
//! serviced create minted is read back through `GetContact`, given a channel, and deleted.

use serde_json::json;

use crate::contact_lifecycle::Emulator;
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet;

/// Onboarding is what flips the answer, and nothing else: one unchanged request is sent
/// over one engine for each state the account's replication set can be in.
#[tokio::test]
async fn one_unchanged_request_is_refused_until_the_account_holds_an_active_set() {
    let request = json!({
        "Alias": "oncall-primary",
        "Type": "PERSONAL",
        "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
    });

    // Every state the account's record can be in, with the status the SAME request gets.
    let states: &[(&str, Option<AccountReplicationSet>, u16)] = &[
        ("no replication set at all", None, 400),
        (
            "a replication set that has not finished replicating",
            Some(AccountReplicationSet {
                active: false,
                ..active_replication_set()
            }),
            400,
        ),
        (
            "an ACTIVE replication set",
            Some(active_replication_set()),
            200,
        ),
    ];

    for (case, replication_set, want) in states {
        let emulator = Emulator::new();
        set_replication_set(&emulator, replication_set.clone());

        let (status, body) = emulator.call("CreateContact", request.clone()).await;
        assert_eq!(
            status, *want,
            "case {case:?}: expected CreateContact to answer HTTP {want}, got {status} \
             with {body:?}"
        );
        if *want == 200 {
            assert!(
                body["ContactArn"]
                    .as_str()
                    .is_some_and(|arn| arn.contains(":ssm-contacts:")),
                "case {case:?}: expected the minted ContactArn, got {body:?}"
            );
        } else {
            assert!(
                body["ContactArn"].is_null(),
                "case {case:?}: a refused create must mint no ContactArn, got {body:?}"
            );
        }
    }
}

/// A write serviced under an ACTIVE set is PERSISTED: the created contact reads back
/// through `GetContact`, takes a channel, and both delete again.
#[tokio::test]
async fn an_onboarded_account_round_trips_a_contact_and_its_channel() {
    let case = "create -> get -> channel -> delete under an active replication set";
    let emulator = Emulator::new();
    onboard(&emulator);

    let (status, body) = emulator
        .call(
            "CreateContact",
            json!({
                "Alias": "oncall-primary",
                "DisplayName": "On-call primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": [{"DurationInMinutes": 5, "Targets": []}]},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateContact to answer 200, got {status} with {body:?}"
    );
    let contact_arn = body["ContactArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ContactArn, got {body:?}"))
        .to_string();

    let (status, body) = emulator
        .call("GetContact", json!({"ContactId": contact_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContact to resolve the created contact, got {status} \
         with {body:?}"
    );
    assert_eq!(
        body["Alias"].as_str(),
        Some("oncall-primary"),
        "case {case:?}: expected the created Alias, got {body:?}"
    );

    let (status, body) = emulator
        .call(
            "CreateContactChannel",
            json!({
                "ContactId": contact_arn,
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateContactChannel to answer 200, got {status} with {body:?}"
    );
    let channel_arn = body["ContactChannelArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ContactChannelArn, got {body:?}"))
        .to_string();

    let (status, body) = emulator
        .call("DeleteContactChannel", json!({"ContactChannelId": channel_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteContactChannel to answer 200, got {status} with {body:?}"
    );
    let (status, body) = emulator
        .call("DeleteContact", json!({"ContactId": contact_arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteContact to answer 200, got {status} with {body:?}"
    );
}
