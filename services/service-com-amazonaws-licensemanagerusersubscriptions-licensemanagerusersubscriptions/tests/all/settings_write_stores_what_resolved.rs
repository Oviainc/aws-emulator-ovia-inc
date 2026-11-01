// SPDX-License-Identifier: CC0-1.0

//! WHAT an accepted `UpdateIdentityProviderSettings` may leave in the store: the very
//! configuration its network resolution covered, and nothing else.
//!
//! `Settings$Subnets` is "the ID of one or more subnets in which License Manager will create
//! a VPC endpoint for products that require connectivity to activation servers" and
//! `SecurityGroupId` "a security group ID that allows inbound TCP port 1688 communication
//! between resources in your VPC and the VPC endpoint for activation servers", so a stored
//! configuration names network resources the account holds in ONE VPC — the rule the
//! sibling `settings_network_resolution` suite asserts on a register and on an update.
//!
//! Resolving it is a call to the service that owns those resources, so this service's store
//! is released across it: the value an update would store is composed of the payload's own
//! ids and the ids the registration already holds, and the second half can move while the
//! resolution is in flight. The rule under test is what that must not cost:
//!
//! * a write whose base settings MOVED after they were resolved stores nothing at all — it
//!   reports the settings held now, because folding the payload onto them would store a
//!   composition no resolution ever saw;
//! * so the update resolves that composition before trying again, and is accepted only if
//!   the composition is one the account's network carries: no accepted write leaves a
//!   registration whose subnets span two VPCs, or whose security group is of neither.
//!
//! The rows below drive exactly that window — an update accepted between the resolution and
//! the write of another one, the interleaving the hop opens — and differ along what the rule
//! generalises over: which member the accepted update moved, how many subnets the payload in
//! flight names, whether the composition breaks the subnets' VPC agreement, the security
//! group's, or has no result at all, and which VPC the composition ends up disagreeing
//! about. Each row also proves the payload in flight was valid against the base it resolved
//! against (an identical registration no update lands on accepts it), so what refuses the
//! composition is the composition.
//!
//! Every wire call goes through the router's `dispatch`, so each request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be; the write step
//! is driven at the state seam the handler runs after its hop, which is what makes the
//! interleaving deterministic.

use serde_json::json;

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::SettingsWrite;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

// ONE wire harness per service, ONE catalog of the network the account holds, ONE builder
// for an `UpdateSettings` payload and ONE reader of the settings a registration holds: all
// from the sibling suites rather than a copy.
use crate::identity_provider_subscriptions::{
    as_str, list_providers, provider_union, register, Harness, UpdateSettingsPayload,
    ACCOUNT_SECURITY_GROUPS, ACCOUNT_SUBNETS, ACCOUNT_VPC, PRODUCTS,
};
use crate::settings_network_resolution::{
    assert_refused, harness_with_two_vpcs, update_settings, Expected, DIRECTORY, OTHER_VPC,
    OTHER_VPC_SECURITY_GROUP, OTHER_VPC_SUBNET,
};
use crate::settings_update_writes_at_write_time::{handle, held_settings, listed_settings};

/// The subnet lists the rows name. An [`UpdateSettingsPayload`] holds
/// `&'static [&'static str]`, so each list is named once here.
const FIRST_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[0]];
const SECOND_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[1]];
const THIRD_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[2]];
const FIRST_TWO_SUBNETS: &[&str] = &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1]];
const FIRST_THREE_SUBNETS: &[&str] = &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]];
const FOURTH_AND_FIFTH_SUBNETS: &[&str] = &[ACCOUNT_SUBNETS[3], ACCOUNT_SUBNETS[4]];
const THE_OTHER_VPCS_SUBNET: &[&str] = &[OTHER_VPC_SUBNET];

/// The security group every row's registration starts from, and the other group of the same
/// VPC a row switches to.
const BASE_GROUP: &str = ACCOUNT_SECURITY_GROUPS[0];
const OTHER_GROUP_SAME_VPC: &str = ACCOUNT_SECURITY_GROUPS[1];

/// What the service must answer for the COMPOSITION of the payload in flight with the
/// update accepted underneath it.
#[derive(Debug, Clone, Copy)]
enum Composed {
    /// The composition names resources of one VPC the account holds, so it is stored.
    Stored,
    /// The composition is refused by the network resolution, which the sibling suite's
    /// catalog words.
    RefusedNetwork(Expected<'static>),
    /// The payload has no result against the moved-on settings at all: it would leave the
    /// registration with no subnet, which `Settings$Subnets` (`@required`, `@length(min:
    /// 1)`) does not admit.
    RefusedPayload,
}

/// The sentence `Settings$Subnets`' minimum is refused with, spelled out here rather than
/// read from the implementation.
const NO_SUBNET_LEFT: &str = "UpdateSettings would leave the registration with no subnet, \
                              and Settings.Subnets requires at least one";

impl Composed {
    fn message(self) -> Option<String> {
        match self {
            Composed::Stored => None,
            Composed::RefusedNetwork(expected) => expected.message(),
            Composed::RefusedPayload => Some(NO_SUBNET_LEFT.to_string()),
        }
    }
}

/// One window: the settings a registration starts from, the update accepted while another
/// one is between its resolution and its write, that other one's payload, and what the
/// composition of the two must come to.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    base_subnets: &'static [&'static str],
    base_group: &'static str,
    /// Applied over the wire, as a whole accepted operation, before the write below.
    accepted_in_between: UpdateSettingsPayload,
    /// The payload of the update whose write step runs after that one. Valid against
    /// `base_subnets`/`base_group`, which the row proves.
    in_flight: UpdateSettingsPayload,
    composed: Composed,
    /// The settings every later read must report once the window has played out: the
    /// composition when it is stored, and what the accepted update left when it is not.
    settings_after: (&'static [&'static str], &'static str),
}

const ROWS: &[Row] = &[
    Row {
        case: "the accepted update moved the registration to the other VPC, the one in \
               flight adds a subnet of this one",
        base_subnets: FIRST_SUBNET,
        base_group: BASE_GROUP,
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: THE_OTHER_VPCS_SUBNET,
            remove_subnets: FIRST_SUBNET,
            security_group_id: Some(OTHER_VPC_SECURITY_GROUP),
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: SECOND_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        // The composition is [the other VPC's subnet, a subnet of this one]: the subnets no
        // longer agree on a VPC, and the first of them decides which one the refusal names.
        composed: Composed::RefusedNetwork(Expected::SubnetsDisagree(OTHER_VPC)),
        settings_after: (THE_OTHER_VPCS_SUBNET, OTHER_VPC_SECURITY_GROUP),
    },
    Row {
        case: "the accepted update moved the registration to the other VPC, the one in \
               flight only switches the security group",
        base_subnets: FIRST_TWO_SUBNETS,
        base_group: BASE_GROUP,
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: THE_OTHER_VPCS_SUBNET,
            remove_subnets: FIRST_TWO_SUBNETS,
            security_group_id: Some(OTHER_VPC_SECURITY_GROUP),
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[],
            security_group_id: Some(OTHER_GROUP_SAME_VPC),
        },
        // The subnets agree on the other VPC now, and the group this payload names is of
        // this one: the group cannot carry the traffic of the endpoint those subnets put it
        // in front of.
        composed: Composed::RefusedNetwork(Expected::GroupElsewhere(
            OTHER_GROUP_SAME_VPC,
            OTHER_VPC,
        )),
        settings_after: (THE_OTHER_VPCS_SUBNET, OTHER_VPC_SECURITY_GROUP),
    },
    Row {
        case: "the accepted update moved the registration to the other VPC, the one in \
               flight adds two subnets of this one and removes one",
        base_subnets: FIRST_THREE_SUBNETS,
        base_group: BASE_GROUP,
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: THE_OTHER_VPCS_SUBNET,
            remove_subnets: FIRST_THREE_SUBNETS,
            security_group_id: Some(OTHER_VPC_SECURITY_GROUP),
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: FOURTH_AND_FIFTH_SUBNETS,
            remove_subnets: FIRST_SUBNET,
            security_group_id: None,
        },
        composed: Composed::RefusedNetwork(Expected::SubnetsDisagree(OTHER_VPC)),
        settings_after: (THE_OTHER_VPCS_SUBNET, OTHER_VPC_SECURITY_GROUP),
    },
    Row {
        case: "the accepted update added a subnet of the same VPC, so the composition \
               resolves and is stored",
        base_subnets: FIRST_SUBNET,
        base_group: BASE_GROUP,
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: THIRD_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: SECOND_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        composed: Composed::Stored,
        settings_after: (
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[2], ACCOUNT_SUBNETS[1]],
            BASE_GROUP,
        ),
    },
    Row {
        case: "the accepted update left one subnet, which the one in flight removes: the \
               composition has no result",
        base_subnets: FIRST_TWO_SUBNETS,
        base_group: BASE_GROUP,
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: FIRST_SUBNET,
            security_group_id: None,
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: SECOND_SUBNET,
            security_group_id: None,
        },
        composed: Composed::RefusedPayload,
        settings_after: (SECOND_SUBNET, BASE_GROUP),
    },
];

/// `Vec<String>` from a row's `&[&str]`, for the state-seam call that takes what the
/// deserialized request carries.
fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

/// The comparison pair a row's expected settings make.
fn pair(subnets: &[&str], security_group_id: &str) -> (Vec<String>, String) {
    (owned(subnets), security_group_id.to_string())
}

/// Register one identity provider for `product` with `subnets` and `security_group_id`, and
/// answer the ARN it was minted.
async fn registered(
    h: &Harness,
    product: &str,
    subnets: &[&str],
    security_group_id: &str,
    case: &str,
) -> String {
    let reply = register(
        h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": product,
            "Settings": { "Subnets": subnets, "SecurityGroupId": security_group_id },
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

/// Assert the settings `settings` names are a configuration the account's network carries,
/// by registering them verbatim for `product`: the resolution a register runs is the same
/// one an update runs, so a configuration no request could have created is refused here.
async fn assert_registers_verbatim(
    h: &Harness,
    product: &str,
    settings: &(Vec<String>, String),
    case: &str,
) {
    let (subnets, security_group_id) = settings;
    let reply = register(
        h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": product,
            "Settings": { "Subnets": subnets, "SecurityGroupId": security_group_id },
        }),
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the stored configuration {settings:?} must be one the account's \
         network carries, so registering it verbatim is accepted, got HTTP {} with {:?}",
        reply.status, reply.body
    );
}

/// RULE: an update stores exactly the configuration its network resolution covered — a
/// write whose base settings moved after they were resolved stores nothing and reports the
/// settings held now, and the update is then accepted only if the composition of its payload
/// with those settings is itself one the account's network carries.
#[tokio::test]
async fn an_update_never_stores_a_composition_it_did_not_resolve() {
    for row in ROWS {
        // A fresh emulator per row, so the registration under test starts from exactly the
        // settings this row names.
        let h = harness_with_two_vpcs();
        let arn = registered(
            &h,
            PRODUCTS[0],
            row.base_subnets,
            row.base_group,
            row.case,
        )
        .await;

        // The payload in flight IS valid against the base it resolved against: the same
        // payload on an identical registration no other update lands on is accepted. So
        // whatever refuses it below is the composition, not the payload.
        let twin = registered(
            &h,
            PRODUCTS[1],
            row.base_subnets,
            row.base_group,
            row.case,
        )
        .await;
        let on_the_twin = update_settings(&h, &row.in_flight.request(&twin)).await;
        assert_eq!(
            on_the_twin.status, 200,
            "case {:?}: the payload in flight must resolve against the base it was read \
             from, got HTTP {} with {:?}",
            row.case, on_the_twin.status, on_the_twin.body
        );

        // The base the update in flight resolved its settings against, read before the
        // window opens — exactly what the handler carries across its hop.
        let resolved_from = held_settings(&h, &arn, row.case);

        // The whole update that is accepted while the one under test is between its
        // resolution and its write: a real operation over the wire.
        let accepted = update_settings(&h, &row.accepted_in_between.request(&arn)).await;
        accepted.ok(row.case);
        let moved_on = listed_settings(&h, &arn, row.case).await;

        // The write step of the update in flight, which is where the handler lands once its
        // resolution answers: the settings it resolved are not the settings held now, so it
        // stores nothing and reports the ones that are.
        let written = {
            let store = h.engine.state::<ServiceState>();
            let mut store = store.lock();
            store.update_live_settings(
                &arn,
                &resolved_from,
                &owned(row.in_flight.add_subnets),
                &owned(row.in_flight.remove_subnets),
                row.in_flight.security_group_id,
            )
        }
        .unwrap_or_else(|error| {
            panic!(
                "case {:?}: the write step must answer that the settings moved, it failed \
                 with {error:?}",
                row.case
            )
        });
        match written {
            SettingsWrite::Moved(held) => {
                let held = (held.subnets, held.security_group_id);
                assert_eq!(
                    held, moved_on,
                    "case {:?}: the write must report the settings held now ({moved_on:?}), \
                     got {held:?}",
                    row.case
                );
            }
            SettingsWrite::Stored(summary) => panic!(
                "case {:?}: the write must store nothing once its base moved, it stored {:?}",
                row.case,
                (summary.settings.subnets, summary.settings.security_group_id)
            ),
        }
        let after_the_write = listed_settings(&h, &arn, row.case).await;
        assert_eq!(
            after_the_write, moved_on,
            "case {:?}: a write that stored nothing must leave {moved_on:?} reported, got \
             {after_the_write:?}",
            row.case
        );

        // What the handler does next: resolve the composition of its payload with the
        // settings held now, and store it only if that whole configuration resolves. Driven
        // as the whole operation, now that the window has closed.
        let composed = update_settings(&h, &row.in_flight.request(&arn)).await;
        let expected = pair(row.settings_after.0, row.settings_after.1);
        match row.composed.message() {
            None => {
                let summary = &composed.ok(row.case)["IdentityProviderSummary"];
                let answered = (
                    summary["Settings"]["Subnets"]
                        .as_array()
                        .unwrap_or_else(|| {
                            panic!("case {:?}: no Subnets in {summary:?}", row.case)
                        })
                        .iter()
                        .map(|subnet| {
                            subnet
                                .as_str()
                                .unwrap_or_else(|| {
                                    panic!("case {:?}: a subnet is not a string", row.case)
                                })
                                .to_string()
                        })
                        .collect::<Vec<String>>(),
                    as_str(&summary["Settings"], "SecurityGroupId", row.case).to_string(),
                );
                assert_eq!(
                    answered, expected,
                    "case {:?}: the accepted composition must answer {expected:?}, got \
                     {answered:?}",
                    row.case
                );
            }
            Some(message) => assert_refused(&composed, &message, row.case),
        }

        // Accepted or refused, the settings the registration is left with are the ones this
        // row names — and they are a configuration the account's network carries, which is
        // what no interleaving may cost.
        let reported = listed_settings(&h, &arn, row.case).await;
        assert_eq!(
            reported, expected,
            "case {:?}: a later read must report {expected:?}, got {reported:?}",
            row.case
        );
        assert_registers_verbatim(&h, PRODUCTS[2], &reported, row.case).await;
    }
}

/// The two updates raced below: one moves the registration into the other VPC, the other
/// adds a subnet of the VPC it starts in. Each is valid on its own against the registered
/// settings, and their COMPOSITION never is, whichever order they land in.
const MOVE_TO_THE_OTHER_VPC: UpdateSettingsPayload = UpdateSettingsPayload {
    add_subnets: THE_OTHER_VPCS_SUBNET,
    remove_subnets: FIRST_TWO_SUBNETS,
    security_group_id: Some(OTHER_VPC_SECURITY_GROUP),
};
const ADD_A_SUBNET_OF_THIS_VPC: UpdateSettingsPayload = UpdateSettingsPayload {
    add_subnets: THIRD_SUBNET,
    remove_subnets: &[],
    security_group_id: None,
};

/// How many times the pair is raced. Bounded, because the assertion is about what EVERY
/// interleaving leaves behind rather than about hitting one of them.
const ROUNDS: usize = 4;

/// RULE: the same rule over the real window — two updates of one registration answered at
/// the same time leave a configuration the account's network carries, so the one whose
/// composition spans two VPCs is refused with the modelled `ValidationException` rather than
/// stored, whichever of the two wrote first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn racing_updates_never_leave_a_configuration_spanning_two_vpcs() {
    for round in 1..=ROUNDS {
        let case = format!("a move and an add answered at the same time (round {round})");
        let h = harness_with_two_vpcs();
        let arn = registered(&h, PRODUCTS[0], FIRST_TWO_SUBNETS, BASE_GROUP, &case).await;

        let (move_handle, add_handle) = (handle(&h), handle(&h));
        let (move_body, add_body) = (
            MOVE_TO_THE_OTHER_VPC.request(&arn),
            ADD_A_SUBNET_OF_THIS_VPC.request(&arn),
        );
        let moved = tokio::spawn(async move { update_settings(&move_handle, &move_body).await });
        let added = tokio::spawn(async move { update_settings(&add_handle, &add_body).await });
        let (moved, added) = (
            moved.await.expect("the move ran"),
            added.await.expect("the add ran"),
        );

        // Exactly one of the two can be accepted: whichever writes second composes its
        // payload with a configuration of the other VPC, which no resolution admits.
        let accepted_count = [&moved, &added]
            .iter()
            .filter(|reply| reply.status == 200)
            .count();
        assert_eq!(
            accepted_count, 1,
            "case {case:?}: exactly one of the two must be accepted, got move={:?} and \
             add={:?}",
            (moved.status, &moved.body),
            (added.status, &added.body)
        );

        // The refusal names the VPC the subnets the loser composed onto agree on: the other
        // VPC when the move landed first, this one when the add did.
        let (refused, expected_after, disagreed_on) = if moved.status == 200 {
            (
                &added,
                pair(THE_OTHER_VPCS_SUBNET, OTHER_VPC_SECURITY_GROUP),
                OTHER_VPC,
            )
        } else {
            (
                &moved,
                pair(
                    &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]],
                    BASE_GROUP,
                ),
                ACCOUNT_VPC,
            )
        };
        let message = Expected::SubnetsDisagree(disagreed_on)
            .message()
            .expect("a refusal row");
        assert_refused(refused, &message, &case);

        let reported = listed_settings(&h, &arn, &case).await;
        assert_eq!(
            reported, expected_after,
            "case {case:?}: the registration must carry what the accepted update stored \
             ({expected_after:?}), got {reported:?}"
        );
        assert_registers_verbatim(&h, PRODUCTS[1], &reported, &case).await;

        // And the registration is still exactly one: neither the refusal nor the race
        // minted or dropped anything.
        let listed = list_providers(&h, &json!({})).await;
        let body = listed.ok(&case);
        assert_eq!(
            body["IdentityProviderSummaries"].as_array().map(Vec::len),
            Some(2),
            "case {case:?}: the registration under test and the verbatim one must be the \
             two reported, got {body:?}"
        );
    }
}
