// SPDX-License-Identifier: CC0-1.0

//! What an UPDATE of a registration's `Settings` may leave it REPORTING: the network ids a
//! later read answers with are resolved against the account's own Amazon EC2 store at the
//! time of the call, never cached from the call that first stored them.
//!
//! `Settings$Subnets` is "the ID of one or more subnets in which License Manager will create
//! a VPC endpoint for products that require connectivity to activation servers" and
//! `SecurityGroupId` "a security group ID that allows inbound TCP port 1688 communication
//! between resources in your VPC and the VPC endpoints for activation servers", so the rule
//! under test is:
//!
//! * a call that would STORE a subnet or a security group the account's Amazon EC2 store
//!   does not hold *at that moment* is refused with the modelled `ValidationException`
//!   (HTTP 400) naming that resource, and the registration keeps exactly the settings it
//!   had — so no read ever reports a configuration that provisions nothing;
//! * what a payload merely REMOVES is not resolved: it leaves the settings rather than
//!   being provisioned, so a registration whose network was deleted underneath it can
//!   always be repaired;
//! * an accepted payload applied twice answers the same settings both times
//!   (`UpdateIdentityProviderSettings` is `@idempotent`).
//!
//! The sibling `settings_network_resolution` suite asserts that rule over ONE unusable
//! resource per request, on a register and on an update. The rows here differ along the
//! dimensions it does not reach: a payload naming SEVERAL unusable resources at once (which
//! of them the refusal names), a payload that removes while it adds, and resources the
//! account HELD when the registration resolved them and deleted afterwards — the
//! propagation direction, where the stored settings go stale rather than the payload being
//! wrong.
//!
//! Every call goes through the router's `dispatch`, so each request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be.

use serde_json::{json, Value};

// ONE wire harness per service, ONE catalog of ids the account does and does not hold, and
// ONE builder for an `UpdateSettings` payload: all of them come from the sibling suites
// rather than a second copy.
use crate::identity_provider_subscriptions::{
    as_str, delete_network, list_providers, provider_union, register, Harness,
    UpdateSettingsPayload, ACCOUNT_SECURITY_GROUPS, ACCOUNT_SUBNETS, ACCOUNT_VPC, PRODUCTS,
};
use crate::settings_network_resolution::{
    assert_refused, harness_with_two_vpcs, update_settings, Expected, DIRECTORY, INITIAL_SUBNETS,
    LONG_UNHELD_SUBNET, OTHER_VPC_SECURITY_GROUP, OTHER_VPC_SUBNET, UNHELD_SECURITY_GROUP,
    UNHELD_SUBNET,
};

/// The security group the registration every row here starts from carries — a group of
/// [`ACCOUNT_VPC`], the VPC [`INITIAL_SUBNETS`] are subnets of.
const INITIAL_SECURITY_GROUP: &str = ACCOUNT_SECURITY_GROUPS[0];

/// The subnet lists the rows and the steps below name. An [`UpdateSettingsPayload`] holds
/// `&'static [&'static str]`, so each list is named here rather than built per call.
const ADD_A_HELD_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[2]];
const ADD_ANOTHER_HELD_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[3]];
const ADD_A_FIFTH_HELD_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[4]];
const REMOVE_THE_FIRST_STORED_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[0]];
const REMOVE_THE_SECOND_STORED_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[1]];

/// Register one identity provider with [`INITIAL_SUBNETS`] and [`INITIAL_SECURITY_GROUP`],
/// and answer the ARN it was minted.
async fn registered(h: &Harness, product: &str, case: &str) -> String {
    let reply = register(
        h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": product,
            "Settings": {
                "Subnets": INITIAL_SUBNETS,
                "SecurityGroupId": INITIAL_SECURITY_GROUP,
            },
        }),
    )
    .await;
    as_str(
        &reply.ok(case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        case,
    )
    .to_string()
}

/// The `Settings` a later read reports for the one registration the harness holds.
async fn reported_settings(h: &Harness, case: &str) -> Value {
    let listed = list_providers(h, &json!({})).await;
    let body = listed.ok(case);
    body["IdentityProviderSummaries"]
        .as_array()
        .and_then(|summaries| summaries.first())
        .and_then(|summary| summary.get("Settings"))
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no registration with settings in {body:?}"))
}

/// The settings value a row expects to be reported.
fn settings(subnets: &[&str], security_group_id: &str) -> Value {
    json!({ "Subnets": subnets, "SecurityGroupId": security_group_id })
}

/// A payload naming more than one resource the account cannot provision in, and the one
/// resource the refusal has to name.
#[derive(Debug, Clone, Copy)]
struct UnusableRow {
    case: &'static str,
    payload: UpdateSettingsPayload,
    expected: Expected<'static>,
}

/// The subnets are resolved before the security group is, so a payload that is wrong about
/// both is refused for the subnet: a refusal names ONE resource, and which one does not
/// depend on how many of the others are unusable.
const UNUSABLE_ROWS: &[UnusableRow] = &[
    UnusableRow {
        case: "one unheld subnet added and an unheld security group set at once",
        payload: UpdateSettingsPayload {
            add_subnets: &[UNHELD_SUBNET],
            remove_subnets: &[],
            security_group_id: Some(UNHELD_SECURITY_GROUP),
        },
        expected: Expected::NoSuchSubnet(UNHELD_SUBNET),
    },
    UnusableRow {
        case: "a held subnet added before an unheld one, with an unheld security group",
        payload: UpdateSettingsPayload {
            add_subnets: &[ACCOUNT_SUBNETS[2], LONG_UNHELD_SUBNET],
            remove_subnets: &[],
            security_group_id: Some(UNHELD_SECURITY_GROUP),
        },
        expected: Expected::NoSuchSubnet(LONG_UNHELD_SUBNET),
    },
    UnusableRow {
        case: "only the security group is unusable, while the payload also removes a \
               stored subnet",
        payload: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[ACCOUNT_SUBNETS[1]],
            security_group_id: Some(UNHELD_SECURITY_GROUP),
        },
        expected: Expected::NoSuchGroup(UNHELD_SECURITY_GROUP),
    },
    UnusableRow {
        case: "both named resources are held, in the other VPC",
        payload: UpdateSettingsPayload {
            add_subnets: &[OTHER_VPC_SUBNET],
            remove_subnets: &[],
            security_group_id: Some(OTHER_VPC_SECURITY_GROUP),
        },
        expected: Expected::SubnetsDisagree(ACCOUNT_VPC),
    },
    UnusableRow {
        case: "an unheld subnet added and a held security group of the other VPC",
        payload: UpdateSettingsPayload {
            add_subnets: &[UNHELD_SUBNET],
            remove_subnets: &[],
            security_group_id: Some(OTHER_VPC_SECURITY_GROUP),
        },
        expected: Expected::NoSuchSubnet(UNHELD_SUBNET),
    },
    UnusableRow {
        case: "a held subnet of this VPC added while the security group switches to a held \
               one of the other VPC",
        payload: UpdateSettingsPayload {
            add_subnets: ADD_A_HELD_SUBNET,
            remove_subnets: &[],
            security_group_id: Some(OTHER_VPC_SECURITY_GROUP),
        },
        // The subnets all agree on this account's VPC, so it is the GROUP that cannot carry
        // the traffic of an endpoint in them.
        expected: Expected::GroupElsewhere(OTHER_VPC_SECURITY_GROUP, ACCOUNT_VPC),
    },
];

/// RULE: an update that would store any resource the account's Amazon EC2 store does not
/// hold is refused naming one of them and stores NOTHING — neither the subnets it would
/// have added, nor the ones it would have removed, nor the security group — and the
/// registration stays usable: the very next payload naming held resources is applied.
#[tokio::test]
async fn an_update_that_cannot_provision_stores_none_of_its_payload() {
    for row in UNUSABLE_ROWS {
        // A fresh emulator per row, so the registration under test starts from the same
        // settings whatever the previous row did.
        let h = harness_with_two_vpcs();
        let provider_arn = registered(&h, PRODUCTS[0], row.case).await;
        let before = settings(INITIAL_SUBNETS, INITIAL_SECURITY_GROUP);

        let message = row
            .expected
            .message()
            .expect("every row of this table is a refusal");
        let reply = update_settings(&h, &row.payload.request(&provider_arn)).await;
        assert_refused(&reply, &message, row.case);

        let reported = reported_settings(&h, row.case).await;
        assert_eq!(
            reported, before,
            "case {:?}: a refused update must leave the registration reporting {before}, got \
             {reported:?}",
            row.case
        );

        // Not wedged: the refusal rejected the payload, not the registration.
        let accepted = update_settings(
            &h,
            &UpdateSettingsPayload {
                add_subnets: ADD_ANOTHER_HELD_SUBNET,
                ..UpdateSettingsPayload::default()
            }
            .request(&provider_arn),
        )
        .await;
        let after = settings(
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[3]],
            INITIAL_SECURITY_GROUP,
        );
        assert_eq!(
            accepted.ok(row.case)["IdentityProviderSummary"].get("Settings"),
            Some(&after),
            "case {:?}: the update after a refused one must answer {after}, got {:?}",
            row.case,
            accepted.body
        );
        let reported = reported_settings(&h, row.case).await;
        assert_eq!(
            reported, after,
            "case {:?}: a later read must report {after}, got {reported:?}",
            row.case
        );
    }
}

/// RULE: the account's network is asked about at the time of the CALL, so a subnet or
/// security group deleted after the registration resolved it stops resolving at once — an
/// update that would keep reporting it is refused naming it — while a payload that REMOVES
/// the gone subnet, or replaces the gone group, is accepted, because what a registration no
/// longer names is not provisioned.
#[tokio::test]
async fn an_update_resolves_the_network_as_it_stands_at_the_call() {
    let case = "a subnet and a security group deleted after the registration resolved them";
    let h = harness_with_two_vpcs();
    let provider_arn = registered(&h, PRODUCTS[1], case).await;

    // The second of the two stored subnets is deleted from the account's EC2 store.
    delete_network(&h.engine, &[ACCOUNT_SUBNETS[1]], &[]);

    // Adding a subnet the account DOES hold would leave the gone one in the settings, so it
    // is refused naming the gone one — the payload is innocent, the configuration it would
    // store is not.
    let reply = update_settings(
        &h,
        &UpdateSettingsPayload {
            add_subnets: ADD_A_HELD_SUBNET,
            ..UpdateSettingsPayload::default()
        }
        .request(&provider_arn),
    )
    .await;
    assert_refused(
        &reply,
        &Expected::NoSuchSubnet(ACCOUNT_SUBNETS[1])
            .message()
            .expect("a refusal"),
        case,
    );
    let reported = reported_settings(&h, case).await;
    let before = settings(INITIAL_SUBNETS, INITIAL_SECURITY_GROUP);
    assert_eq!(
        reported, before,
        "case {case:?}: the refused update must leave {before} reported, got {reported:?}"
    );

    // REMOVING the gone subnet is accepted: nothing is provisioned in what the settings stop
    // naming, so the registration can be repaired.
    let repaired = update_settings(
        &h,
        &UpdateSettingsPayload {
            remove_subnets: REMOVE_THE_SECOND_STORED_SUBNET,
            ..UpdateSettingsPayload::default()
        }
        .request(&provider_arn),
    )
    .await;
    let after_removal = settings(&[ACCOUNT_SUBNETS[0]], INITIAL_SECURITY_GROUP);
    assert_eq!(
        repaired.ok(case)["IdentityProviderSummary"].get("Settings"),
        Some(&after_removal),
        "case {case:?}: removing the gone subnet must answer {after_removal}, got {:?}",
        repaired.body
    );

    // And the add that was refused a moment ago is accepted now, unchanged.
    let accepted = update_settings(
        &h,
        &UpdateSettingsPayload {
            add_subnets: ADD_A_HELD_SUBNET,
            ..UpdateSettingsPayload::default()
        }
        .request(&provider_arn),
    )
    .await;
    let after_add = settings(
        &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[2]],
        INITIAL_SECURITY_GROUP,
    );
    assert_eq!(
        accepted.ok(case)["IdentityProviderSummary"].get("Settings"),
        Some(&after_add),
        "case {case:?}: the add must answer {after_add} once the gone subnet is out, got {:?}",
        accepted.body
    );

    // The same propagation for the security group the registration carries: once it is gone
    // from the account's store, an update that would keep it is refused naming it, and one
    // that replaces it with a held group of the subnets' VPC is accepted.
    delete_network(&h.engine, &[], &[INITIAL_SECURITY_GROUP]);
    let reply = update_settings(
        &h,
        &UpdateSettingsPayload {
            add_subnets: ADD_ANOTHER_HELD_SUBNET,
            ..UpdateSettingsPayload::default()
        }
        .request(&provider_arn),
    )
    .await;
    assert_refused(
        &reply,
        &Expected::NoSuchGroup(INITIAL_SECURITY_GROUP)
            .message()
            .expect("a refusal"),
        case,
    );
    let reported = reported_settings(&h, case).await;
    assert_eq!(
        reported, after_add,
        "case {case:?}: the refused update must leave {after_add} reported, got {reported:?}"
    );

    let replaced = update_settings(
        &h,
        &UpdateSettingsPayload {
            add_subnets: ADD_ANOTHER_HELD_SUBNET,
            remove_subnets: &[],
            security_group_id: Some(ACCOUNT_SECURITY_GROUPS[2]),
        }
        .request(&provider_arn),
    )
    .await;
    let after_replacement = settings(
        &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[2], ACCOUNT_SUBNETS[3]],
        ACCOUNT_SECURITY_GROUPS[2],
    );
    assert_eq!(
        replaced.ok(case)["IdentityProviderSummary"].get("Settings"),
        Some(&after_replacement),
        "case {case:?}: replacing the gone group must answer {after_replacement}, got {:?}",
        replaced.body
    );
    let reported = reported_settings(&h, case).await;
    assert_eq!(
        reported, after_replacement,
        "case {case:?}: a later read must report {after_replacement}, got {reported:?}"
    );
}

/// RULE: the operation is `@idempotent`, so a payload naming resources the account holds
/// answers the same settings however often it is applied — the resolution it runs each time
/// does not change what is stored.
#[tokio::test]
async fn an_accepted_payload_applied_twice_answers_the_same_settings() {
    let case = "the same accepted payload, twice";
    let h = harness_with_two_vpcs();
    let provider_arn = registered(&h, PRODUCTS[2], case).await;
    let payload = UpdateSettingsPayload {
        add_subnets: ADD_A_FIFTH_HELD_SUBNET,
        remove_subnets: REMOVE_THE_FIRST_STORED_SUBNET,
        security_group_id: Some(ACCOUNT_SECURITY_GROUPS[1]),
    };
    let expected = settings(
        &[ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[4]],
        ACCOUNT_SECURITY_GROUPS[1],
    );

    for attempt in 1..=2 {
        let reply = update_settings(&h, &payload.request(&provider_arn)).await;
        assert_eq!(
            reply.ok(case)["IdentityProviderSummary"].get("Settings"),
            Some(&expected),
            "case {case:?}: attempt {attempt} must answer {expected}, got {:?}",
            reply.body
        );
        let reported = reported_settings(&h, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: after attempt {attempt} a later read must report {expected}, got \
             {reported:?}"
        );
    }
}
