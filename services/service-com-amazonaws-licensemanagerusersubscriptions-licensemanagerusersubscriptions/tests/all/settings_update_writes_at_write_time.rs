// SPDX-License-Identifier: CC0-1.0

//! WHAT an `UpdateIdentityProviderSettings` write is decided against: the value the store
//! holds when the write happens, never one read before it.
//!
//! The operation resolves the network its payload would store against the account's own
//! Amazon EC2 (`Settings$Subnets` are "the ID of one or more subnets in which License
//! Manager will create a VPC endpoint", `SecurityGroupId` the group in front of those
//! endpoints), which is a call to the service that owns those resources: this service's
//! store is released across it and taken again to write. The rule under test is the one
//! every such write obeys:
//!
//! * the resource is resolved AGAIN under the lock that writes, by the same liveness rule
//!   the read before the hop used — a registration whose deregistration was accepted is
//!   REPORTED so a client can watch it go, not so work can be built on it, so an update
//!   that reaches the write once the teardown was accepted is refused with the modelled
//!   `ValidationException` (HTTP 400) and `Identity provider is not valid` rather than
//!   applied to a registration on its way out;
//! * the payload is folded onto the settings held AT THAT MOMENT, so a write accepted in
//!   between is part of what this update adds to — no accepted update is silently
//!   discarded, and no value the store has moved past is written back.
//!
//! The table below drives the WRITE STEP (`update_live_settings`, the one the handler runs
//! after its hop) with another update already accepted over the wire in between, which is
//! exactly the window the hop opens, and with the settings held then as the base the
//! resolution was made against — the base each of the handler's attempts re-reads. Its rows
//! differ along what the rule generalises over: which member the update in flight names,
//! which member the accepted update changed, and whether folding onto the moved-on value
//! still has a result at all. The contention test then drives two whole operations over the
//! wire in parallel, where the window is real. That a write whose base moved stores nothing
//! at all is the sibling `settings_write_stores_what_resolved` suite's rule.
//!
//! Every wire call goes through the router's `dispatch`, so each request is deserialized,
//! constraint-checked, handled and serialized exactly as a client's would be.

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ops::types::Settings;
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::{
    SettingsWrite, StateError, IDENTITY_PROVIDER_NOT_VALID,
};
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

// ONE wire harness per service, ONE catalog of the network the account holds and ONE
// builder for an `UpdateSettings` payload: all from the sibling suites rather than a copy.
use crate::identity_provider_subscriptions::{
    as_str, deregister, harness, list_providers, provider_union, register, Harness,
    UpdateSettingsPayload, ACCOUNT_SECURITY_GROUPS, ACCOUNT_SUBNETS, PAST_THE_OWED_TRANSITION,
    PRODUCTS,
};
use crate::settings_network_resolution::{assert_refused, update_settings, DIRECTORY};

/// The settings every registration in this suite starts from: two subnets of the account's
/// VPC and a security group of it.
const BASE_SUBNETS: &[&str] = &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1]];
const BASE_SECURITY_GROUP: &str = ACCOUNT_SECURITY_GROUPS[0];

/// The subnet lists the rows name. An [`UpdateSettingsPayload`] holds
/// `&'static [&'static str]`, so each list is named once here.
const THIRD_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[2]];
const FOURTH_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[3]];
const FIFTH_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[4]];
const FIRST_BASE_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[0]];
const SECOND_BASE_SUBNET: &[&str] = &[ACCOUNT_SUBNETS[1]];

/// `Vec<String>` from a row's `&[&str]`, for the state-seam call that takes what the
/// deserialized request carries.
fn owned(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

/// The settings a value carries, as the pair this suite compares on (`Settings` is a wire
/// shape with no equality of its own).
fn pair(subnets: &[&str], security_group_id: &str) -> (Vec<String>, String) {
    (owned(subnets), security_group_id.to_string())
}

/// Register one identity provider for `product` with [`BASE_SUBNETS`] and
/// [`BASE_SECURITY_GROUP`], and answer the ARN it was minted.
async fn registered(h: &Harness, product: &str, case: &str) -> String {
    let reply = register(
        h,
        &json!({
            "IdentityProvider": provider_union(DIRECTORY),
            "Product": product,
            "Settings": { "Subnets": BASE_SUBNETS, "SecurityGroupId": BASE_SECURITY_GROUP },
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

/// The `Settings` the store holds for the registration `arn` names — the base the write
/// step is given, which is the one the handler's resolution is made against.
pub(crate) fn held_settings(h: &Harness, arn: &str, case: &str) -> Settings {
    let store = h.engine.state::<ServiceState>();
    let store = store.lock();
    store
        .identity_providers
        .get(arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no registration {arn}"))
        .settings
        .clone()
}

/// The settings the store holds for the registration `arn` names, as the comparison pair.
fn stored_settings(h: &Harness, arn: &str, case: &str) -> (Vec<String>, String) {
    let store = h.engine.state::<ServiceState>();
    let store = store.lock();
    let record = store
        .identity_providers
        .get(arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no registration {arn}"));
    (
        record.settings.subnets.clone(),
        record.settings.security_group_id.clone(),
    )
}

/// The settings `ListIdentityProviders` REPORTS for the registration `arn` names, as the
/// comparison pair — the read-back every accepted write is proved through.
pub(crate) async fn listed_settings(h: &Harness, arn: &str, case: &str) -> (Vec<String>, String) {
    let listed = list_providers(h, &json!({})).await;
    let body = listed.ok(case);
    let summary = body["IdentityProviderSummaries"]
        .as_array()
        .and_then(|summaries| {
            summaries
                .iter()
                .find(|summary| summary.get("IdentityProviderArn") == Some(&json!(arn)))
        })
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {arn} is not reported in {body:?}"));
    settings_pair(&summary["Settings"], case)
}

/// The comparison pair of a `Settings` as it arrives on the wire.
fn settings_pair(settings: &Value, case: &str) -> (Vec<String>, String) {
    let subnets = settings["Subnets"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no Subnets in {settings:?}"))
        .iter()
        .map(|subnet| {
            subnet
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a subnet is not a string"))
                .to_string()
        })
        .collect();
    (
        subnets,
        as_str(settings, "SecurityGroupId", case).to_string(),
    )
}

/// What the write step must do with the payload in flight, once another update has been
/// accepted underneath it.
#[derive(Debug, Clone, Copy)]
enum WriteTime {
    /// The settings it must store: the subnets in order, and the security group.
    Stored(&'static [&'static str], &'static str),
    /// Folded onto the value the store holds now, the payload has no result — it would
    /// leave the registration with no subnet, which `Settings$Subnets` (`@required`,
    /// `@length(min: 1)`) does not admit — so the write stores nothing.
    Refused,
}

/// One window: the update accepted while the resolution is in flight, the payload whose
/// write lands after it, and what that write must do.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// Applied over the wire, as a whole accepted operation, before the write below.
    accepted_in_between: UpdateSettingsPayload,
    /// The payload of the update whose write step runs after that one.
    in_flight: UpdateSettingsPayload,
    expected: WriteTime,
    /// What a write folding onto the snapshot read BEFORE the window would have stored:
    /// the lost add, the resurrected subnet or the stale group this row is about. Never
    /// what the store may hold afterwards.
    stale: (&'static [&'static str], &'static str),
}

const ROWS: &[Row] = &[
    Row {
        case: "the accepted update added a subnet, the one in flight adds another",
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: THIRD_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: FOURTH_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        expected: WriteTime::Stored(
            &[
                ACCOUNT_SUBNETS[0],
                ACCOUNT_SUBNETS[1],
                ACCOUNT_SUBNETS[2],
                ACCOUNT_SUBNETS[3],
            ],
            BASE_SECURITY_GROUP,
        ),
        stale: (
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[3]],
            BASE_SECURITY_GROUP,
        ),
    },
    Row {
        case: "the accepted update removed a stored subnet, the one in flight adds one",
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: FIRST_BASE_SUBNET,
            security_group_id: None,
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: THIRD_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        expected: WriteTime::Stored(
            &[ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]],
            BASE_SECURITY_GROUP,
        ),
        stale: (
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]],
            BASE_SECURITY_GROUP,
        ),
    },
    Row {
        case: "the accepted update replaced the security group, the one in flight adds a \
               subnet",
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[],
            security_group_id: Some(ACCOUNT_SECURITY_GROUPS[1]),
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: THIRD_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        expected: WriteTime::Stored(
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]],
            ACCOUNT_SECURITY_GROUPS[1],
        ),
        stale: (
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]],
            BASE_SECURITY_GROUP,
        ),
    },
    Row {
        case: "the accepted update added a subnet, the one in flight removes one and sets \
               the security group",
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: FIFTH_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: SECOND_BASE_SUBNET,
            security_group_id: Some(ACCOUNT_SECURITY_GROUPS[2]),
        },
        expected: WriteTime::Stored(
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[4]],
            ACCOUNT_SECURITY_GROUPS[2],
        ),
        stale: (&[ACCOUNT_SUBNETS[0]], ACCOUNT_SECURITY_GROUPS[2]),
    },
    Row {
        case: "both updates add and remove at once",
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: THIRD_SUBNET,
            remove_subnets: FIRST_BASE_SUBNET,
            security_group_id: None,
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: FOURTH_SUBNET,
            remove_subnets: SECOND_BASE_SUBNET,
            security_group_id: None,
        },
        expected: WriteTime::Stored(
            &[ACCOUNT_SUBNETS[2], ACCOUNT_SUBNETS[3]],
            BASE_SECURITY_GROUP,
        ),
        stale: (
            &[ACCOUNT_SUBNETS[0], ACCOUNT_SUBNETS[3]],
            BASE_SECURITY_GROUP,
        ),
    },
    Row {
        case: "the accepted update left one subnet, which the one in flight removes",
        accepted_in_between: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: FIRST_BASE_SUBNET,
            security_group_id: None,
        },
        in_flight: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: SECOND_BASE_SUBNET,
            security_group_id: None,
        },
        // Against the one subnet that is left the payload empties the settings, so there is
        // no resulting configuration to store — against the snapshot it would have stored
        // the first subnet, which the accepted update had already taken out.
        expected: WriteTime::Refused,
        stale: (FIRST_BASE_SUBNET, BASE_SECURITY_GROUP),
    },
];

/// RULE: the write step of an update folds the payload onto the settings the store holds AT
/// WRITE TIME, so an update accepted while the network resolution is in flight is added to
/// rather than overwritten, and a payload with no result against the moved-on value is
/// refused instead of being applied to a value the store has moved past.
#[tokio::test]
async fn an_update_is_folded_onto_the_settings_held_when_it_writes() {
    for (index, row) in ROWS.iter().enumerate() {
        // A fresh emulator per row, so every row starts from the same settings. The product
        // varies with the row only to keep the registrations distinguishable in a failure.
        let h = harness();
        let product = PRODUCTS[index % PRODUCTS.len()];
        let arn = registered(&h, product, row.case).await;

        // The whole update that is accepted while the one under test is between its
        // resolution and its write: a real operation over the wire.
        let accepted = update_settings(&h, &row.accepted_in_between.request(&arn)).await;
        accepted.ok(row.case);
        let moved_on = stored_settings(&h, &arn, row.case);

        // The write step of the update in flight, which is where the handler lands once its
        // resolution answers.
        let base = held_settings(&h, &arn, row.case);
        let result = {
            let store = h.engine.state::<ServiceState>();
            let mut store = store.lock();
            store.update_live_settings(
                &arn,
                &base,
                &owned(row.in_flight.add_subnets),
                &owned(row.in_flight.remove_subnets),
                row.in_flight.security_group_id,
            )
        };

        let stale = pair(row.stale.0, row.stale.1);
        match row.expected {
            WriteTime::Stored(subnets, security_group_id) => {
                let expected = pair(subnets, security_group_id);
                let written = result.unwrap_or_else(|error| {
                    panic!("case {:?}: expected the write to store {expected:?}, it was refused with {error:?}", row.case)
                });
                let summary = match written {
                    SettingsWrite::Stored(summary) => *summary,
                    SettingsWrite::Moved(held) => panic!(
                        "case {:?}: expected the write to store {expected:?} onto the base it \
                         was resolved against, it reported the settings moved to {:?}",
                        row.case,
                        (held.subnets, held.security_group_id)
                    ),
                };
                let answered = (
                    summary.settings.subnets.clone(),
                    summary.settings.security_group_id.clone(),
                );
                assert_eq!(
                    answered, expected,
                    "case {:?}: the write must answer {expected:?}, got {answered:?}",
                    row.case
                );
                assert_ne!(
                    answered, stale,
                    "case {:?}: the write must not store the value folded onto the \
                     pre-resolution snapshot {stale:?}",
                    row.case
                );
                let reported = listed_settings(&h, &arn, row.case).await;
                assert_eq!(
                    reported, expected,
                    "case {:?}: a later read must report {expected:?}, got {reported:?}",
                    row.case
                );
            }
            WriteTime::Refused => {
                let error = result.err().unwrap_or_else(|| {
                    panic!(
                        "case {:?}: the payload has no result against {moved_on:?}, so the \
                         write must be refused",
                        row.case
                    )
                });
                assert!(
                    matches!(error, StateError::Validation(_)),
                    "case {:?}: expected a validation refusal, got {error:?}",
                    row.case
                );
                let reported = listed_settings(&h, &arn, row.case).await;
                assert_eq!(
                    reported, moved_on,
                    "case {:?}: a refused write must leave {moved_on:?} reported, got \
                     {reported:?}",
                    row.case
                );
                assert_ne!(
                    reported, stale,
                    "case {:?}: a refused write must not store the value folded onto the \
                     pre-resolution snapshot {stale:?}",
                    row.case
                );
            }
        }
    }
}

/// RULE: a registration on its way out is reported, not acted on — so the write step
/// resolves it by the same liveness rule the read before the resolution did, and refuses an
/// update whose registration's deregistration was accepted while that resolution was in
/// flight, exactly as it refuses one whose registration is already gone.
#[tokio::test]
async fn an_update_does_not_write_a_registration_that_is_being_torn_down() {
    let case = "deregistered while the network resolution is in flight";
    let h = harness();
    let arn = registered(&h, PRODUCTS[0], case).await;

    // The deregistration lands in the window: the record stays in the store, in the
    // non-terminal DEREGISTERING status, which is what a caller polls to watch the teardown.
    let accepted = deregister(&h, &json!({ "IdentityProviderArn": arn })).await;
    assert_eq!(
        as_str(&accepted.ok(case)["IdentityProviderSummary"], "Status", case),
        "DEREGISTERING",
        "case {case:?}: the deregistration answers the non-terminal status it tears down in"
    );
    let before = stored_settings(&h, &arn, case);
    // The base the resolution was made against: unchanged here, so the refusal below is the
    // liveness rule's and nothing else.
    let base = held_settings(&h, &arn, case);

    // The write step the handler reaches once its resolution answers: refused, and nothing
    // of the payload is stored on the registration being torn down.
    let payload = UpdateSettingsPayload {
        add_subnets: THIRD_SUBNET,
        remove_subnets: &[],
        security_group_id: Some(ACCOUNT_SECURITY_GROUPS[1]),
    };
    {
        let store = h.engine.state::<ServiceState>();
        let mut store = store.lock();
        let error = store
            .update_live_settings(
                &arn,
                &base,
                &owned(payload.add_subnets),
                &owned(payload.remove_subnets),
                payload.security_group_id,
            )
            .err()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the write must refuse a deregistering registration")
            });
        assert!(
            matches!(error, StateError::ProviderNotRegistered),
            "case {case:?}: expected the registration not to resolve, got {error:?}"
        );
    }
    let after = stored_settings(&h, &arn, case);
    assert_eq!(
        after, before,
        "case {case:?}: the registration being torn down must keep {before:?}, got {after:?}"
    );

    // And over the wire, whichever side of the resolution the deregistration lands on, the
    // caller reads the one refusal this surface words for a reference that does not resolve.
    let refused = update_settings(&h, &payload.request(&arn)).await;
    assert_refused(&refused, IDENTITY_PROVIDER_NOT_VALID, case);

    // Once the teardown the clock owed has completed the record is gone, and the write step
    // answers the very same refusal.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let gone = update_settings(&h, &payload.request(&arn)).await;
    assert_refused(&gone, IDENTITY_PROVIDER_NOT_VALID, "the teardown has completed");
}

/// A second handle on the SAME emulator: the router and the engine behind it are shared, so
/// two tasks driving two handles are two clients of one emulator rather than two emulators.
pub(crate) fn handle(h: &Harness) -> Harness {
    Harness {
        router: h.router.clone(),
        engine: Arc::clone(&h.engine),
    }
}

/// Two payloads applied to one registration at the same time, and what the registration
/// must carry once both have been answered — a composition that does not depend on which
/// of the two wrote last, so the assertion holds for every interleaving.
#[derive(Debug, Clone, Copy)]
struct ContendingPair {
    case: &'static str,
    first: UpdateSettingsPayload,
    second: UpdateSettingsPayload,
    /// The subnets both updates leave, compared as a SET: which order they end up in does
    /// depend on who wrote last, which subnets are there does not.
    subnets: &'static [&'static str],
    security_group_id: &'static str,
}

const CONTENDING: &[ContendingPair] = &[
    ContendingPair {
        case: "each update adds a subnet of its own",
        first: UpdateSettingsPayload {
            add_subnets: THIRD_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        second: UpdateSettingsPayload {
            add_subnets: FOURTH_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        subnets: &[
            ACCOUNT_SUBNETS[0],
            ACCOUNT_SUBNETS[1],
            ACCOUNT_SUBNETS[2],
            ACCOUNT_SUBNETS[3],
        ],
        security_group_id: BASE_SECURITY_GROUP,
    },
    ContendingPair {
        case: "one adds a subnet while the other removes a stored one",
        first: UpdateSettingsPayload {
            add_subnets: THIRD_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        second: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: FIRST_BASE_SUBNET,
            security_group_id: None,
        },
        subnets: &[ACCOUNT_SUBNETS[1], ACCOUNT_SUBNETS[2]],
        security_group_id: BASE_SECURITY_GROUP,
    },
    ContendingPair {
        case: "one adds a subnet while the other replaces the security group",
        first: UpdateSettingsPayload {
            add_subnets: FIFTH_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        },
        second: UpdateSettingsPayload {
            add_subnets: &[],
            remove_subnets: &[],
            security_group_id: Some(ACCOUNT_SECURITY_GROUPS[1]),
        },
        subnets: &[
            ACCOUNT_SUBNETS[0],
            ACCOUNT_SUBNETS[1],
            ACCOUNT_SUBNETS[4],
        ],
        security_group_id: ACCOUNT_SECURITY_GROUPS[1],
    },
];

/// How many times each pair is raced. Bounded, because the assertion is about what EVERY
/// interleaving leaves behind rather than about hitting one of them: a handful of rounds is
/// enough for two tasks on two worker threads to overlap somewhere.
const ROUNDS: usize = 8;

/// RULE: two updates of one registration answered at the same time both land — the one that
/// writes second folds its payload onto what the first stored — so no accepted update is
/// lost, whichever order the two writes happen in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_overlapping_updates_both_land() {
    for pair_under_test in CONTENDING {
        for round in 1..=ROUNDS {
            let case = format!("{} (round {round})", pair_under_test.case);
            let h = harness();
            let arn = registered(&h, PRODUCTS[0], &case).await;

            let (first_handle, second_handle) = (handle(&h), handle(&h));
            let (first_body, second_body) = (
                pair_under_test.first.request(&arn),
                pair_under_test.second.request(&arn),
            );
            let first = tokio::spawn(async move {
                update_settings(&first_handle, &first_body).await
            });
            let second = tokio::spawn(async move {
                update_settings(&second_handle, &second_body).await
            });
            let (first, second) = (
                first.await.expect("the first update ran"),
                second.await.expect("the second update ran"),
            );
            first.ok(&case);
            second.ok(&case);

            let (mut subnets, security_group_id) = listed_settings(&h, &arn, &case).await;
            subnets.sort();
            let mut expected = owned(pair_under_test.subnets);
            expected.sort();
            assert_eq!(
                subnets, expected,
                "case {case:?}: both updates must be in the stored subnets {expected:?}, got \
                 {subnets:?}"
            );
            assert_eq!(
                security_group_id, pair_under_test.security_group_id,
                "case {case:?}: the stored security group must be {:?}, got \
                 {security_group_id:?}",
                pair_under_test.security_group_id
            );
        }
    }
}

/// RULE: the same liveness rule, over the real window — a `DeregisterIdentityProvider`
/// answered at the same time as an update is either before the update's write, and then the
/// update is refused with the modelled `ValidationException` and stores nothing, or after
/// it, and then the update was applied to a registration that was still live: an accepted
/// update never answers a registration that is being torn down.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_racing_a_deregistration_is_never_applied_to_it() {
    let added = ACCOUNT_SUBNETS[2];
    for round in 1..=ROUNDS {
        let case = format!("a deregistration answered alongside the update (round {round})");
        let h = harness();
        let arn = registered(&h, PRODUCTS[0], &case).await;
        let payload = UpdateSettingsPayload {
            add_subnets: THIRD_SUBNET,
            remove_subnets: &[],
            security_group_id: None,
        };

        let (update_handle, deregister_handle) = (handle(&h), handle(&h));
        let (update_body, deregister_body) = (
            payload.request(&arn),
            json!({ "IdentityProviderArn": arn }),
        );
        let update =
            tokio::spawn(async move { update_settings(&update_handle, &update_body).await });
        let torn_down =
            tokio::spawn(async move { deregister(&deregister_handle, &deregister_body).await });
        let (update, torn_down) = (
            update.await.expect("the update ran"),
            torn_down.await.expect("the deregistration ran"),
        );
        torn_down.ok(&case);

        if update.status == 200 {
            let summary = &update.body["IdentityProviderSummary"];
            let status = as_str(summary, "Status", &case);
            assert_ne!(
                status, "DEREGISTERING",
                "case {case:?}: an accepted update must have been applied to a live \
                 registration, got {summary:?}"
            );
            let (subnets, _) = settings_pair(&summary["Settings"], &case);
            assert!(
                subnets.iter().any(|subnet| subnet == added),
                "case {case:?}: an accepted update answers the settings it stored, which \
                 carry {added:?}, got {subnets:?}"
            );
        } else {
            assert_refused(&update, IDENTITY_PROVIDER_NOT_VALID, &case);
            // The record is still in the store, in DEREGISTERING, for the teardown a client
            // polls — and the refused update left nothing of its payload on it.
            let (subnets, _) = stored_settings(&h, &arn, &case);
            assert!(
                !subnets.iter().any(|subnet| subnet == added),
                "case {case:?}: a refused update must store nothing, got {subnets:?}"
            );
        }
    }
}
