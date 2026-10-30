// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the sweep that materialises the transitions the clock has made due,
//! driven over the real wire path: every call goes through the router's `dispatch`, so the
//! request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The RULE, asserted over a TABLE of inputs rather than one request: which transition a
//! stored record owes the clock, and when, is a fact about the RECORD — so the sweep asks
//! the record (the same trait every writer schedules by) instead of re-deciding it per
//! record kind, and the store's pending guard after a sweep is exactly the earliest
//! transition the records left in the store carry. The rows differ along which record kind
//! is left pending while the sweep runs (a registration, a user association, a product
//! subscription, a license server endpoint), whether the transition it owes was scheduled by
//! a CREATING operation or by a TEARDOWN one, and — for the association rows — which
//! instance and user name the record hangs off.
//!
//! Every row is built the same way: a record of another kind is stored FIRST, so it is what
//! the clock owes first; the row's own record is stored a fraction of a span later, so it is
//! still pending when the first one falls due; then ONE reporting request runs the sweep —
//! the request that applies the first record's transition and has to recompute the guard
//! from the second. The guard is read directly from the store because it is not a reportable
//! value (its only wire-visible consequence is a transition that falls due and is never
//! materialised until some unrelated record happens to fall due too), and the due instant it
//! is compared against is read off the records themselves rather than computed from a span
//! this suite knows.

use std::time::{Duration, SystemTime};

use serde_json::json;

// ONE wire harness per service: the dispatch plumbing, the signing identity and the
// register/associate/subscribe/create helpers come from the sibling suites rather than a
// second copy, and so do the two store readers (`guard`, `owed`) the sibling suite reads the
// pending guard and the records' own owed instants with.
use super::associations_follow_their_registration::{deregistration, list_associations};
use super::identity_provider_subscriptions::{
    as_str, deregister, harness, list_providers, list_subscriptions, provider_union, register,
    start_subscription, stop_subscription, Harness, PAST_THE_OWED_TRANSITION, PRODUCTS,
};
use super::license_server_endpoint_deletion::delete_endpoint;
use super::license_server_endpoints::{create_endpoint, ENDPOINTS};
use super::pending_transition_comes_with_the_record::{
    endpoint_status, guard, owed, summary_status,
};
use super::resource_resolution_refusals::{associate, disassociate, held_instance, registration};

/// The directory every row's base registration is for — one the shared harness holds in the
/// account's directory fleet.
const BASE_DIRECTORY: &str = "d-0123456789";
/// The product that base registration is for.
const BASE_PRODUCT: &str = PRODUCTS[0];
/// The directory the rows that register or deregister act on, so neither of them touches the
/// base registration the other kinds hang off.
const OTHER_DIRECTORY: &str = "d-00000000ff";
/// The product those two rows register that directory for.
const OTHER_PRODUCT: &str = PRODUCTS[3];
/// The subnet every registration in this suite provisions into — one the harness holds in
/// the account's own Amazon EC2 store.
const ROW_SUBNET: &str = "subnet-0123456789abcdef0";
/// The user the record that triggers each row's sweep is associated for.
const TRIGGER_USERNAME: &str = "trigger.user";
/// The instance that record names — an index into the account's seeded fleet that no row
/// uses, so the trigger and the row's own record are never the same association.
const TRIGGER_INSTANCE: usize = 0;

/// How long after the record that triggers the sweep the row's own record is stored.
///
/// A transition in this service falls due a fixed span after the call that started it, so
/// storing the row's record this much later makes its due instant this much later too: that
/// is the window the sweep runs in, with the trigger due and the row's own record not. Well
/// inside the span, so the two are pending together, and wide enough that the one request
/// fired inside the window is not a race.
const STAGGER: Duration = Duration::from_millis(300);

/// Which operation leaves the row's record in a non-terminal status, and so which of the
/// per-kind branches of the sweep has to recompute the guard from it.
#[derive(Debug, Clone, Copy)]
enum Pending {
    /// `RegisterIdentityProvider`, which stores a registration still being provisioned.
    Registration,
    /// `DeregisterIdentityProvider`, which leaves a registration being torn down.
    Deregistration,
    /// `AssociateUser`, which stores an association whose instance is being configured, for
    /// `username` on the account instance `instance` indexes.
    Association {
        instance: usize,
        username: &'static str,
    },
    /// `DisassociateUser`, which leaves an association being torn down.
    Disassociation {
        instance: usize,
        username: &'static str,
    },
    /// `StopProductSubscription`, which leaves a product subscription being torn down.
    SubscriptionTeardown { username: &'static str },
    /// `CreateLicenseServerEndpoint`, which stores an endpoint still being provisioned.
    EndpointCreate,
    /// `DeleteLicenseServerEndpoint`, which leaves an endpoint being deleted.
    EndpointDeletion,
}

impl Pending {
    /// The non-terminal status the row's record carries while the sweep runs.
    ///
    /// Spelled out here rather than read from the production constants, so a change to them
    /// fails a test instead of silently changing what this suite accepts.
    fn status(self) -> &'static str {
        match self {
            Pending::Registration => "REGISTERING",
            Pending::Deregistration => "DEREGISTERING",
            Pending::Association { .. } => "ASSOCIATING",
            Pending::Disassociation { .. } => "DISASSOCIATING",
            Pending::SubscriptionTeardown { .. } => "UNSUBSCRIBED",
            Pending::EndpointCreate => "PROVISIONING",
            Pending::EndpointDeletion => "DELETING",
        }
    }
}

/// One record to leave pending across a sweep.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    pending: Pending,
}

const ROWS: &[Row] = &[
    Row {
        case: "a registration still being provisioned",
        pending: Pending::Registration,
    },
    Row {
        case: "a registration being torn down",
        pending: Pending::Deregistration,
    },
    Row {
        case: "an association whose instance is still being configured",
        pending: Pending::Association {
            instance: 1,
            username: "pending.user",
        },
    },
    Row {
        case: "an association being torn down, on a third instance",
        pending: Pending::Disassociation {
            instance: 2,
            username: "settled.user",
        },
    },
    Row {
        case: "a product subscription being torn down",
        pending: Pending::SubscriptionTeardown {
            username: "subscribed.user",
        },
    },
    Row {
        case: "an endpoint still being provisioned",
        pending: Pending::EndpointCreate,
    },
    Row {
        case: "an endpoint being deleted",
        pending: Pending::EndpointDeletion,
    },
];

/// Wait out whatever the clock owes and sweep it in through a plain read, leaving the store
/// owing nothing.
async fn settle(h: &Harness, case: &str) {
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    list_providers(h, &json!({})).await.ok(case);
}

/// Sleep until `instant` has passed, making NO request.
///
/// The store sweeps only when a request runs, so this is how a row reaches the window where
/// the trigger is due and its own record is not, with both still stored.
async fn wait_until(instant: SystemTime) {
    while let Ok(remaining) = instant.duration_since(SystemTime::now()) {
        tokio::time::sleep(remaining.max(Duration::from_millis(1))).await;
    }
}

/// A harness whose base registration is settled, answering it and its `IdentityProviderArn`.
async fn setup() -> (Harness, String) {
    let case = "the base registration every row hangs off";
    let h = harness();
    let reply = register(
        &h,
        &registration(BASE_DIRECTORY, BASE_PRODUCT, &[ROW_SUBNET], None),
    )
    .await;
    let arn = as_str(
        &reply.ok(case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        case,
    )
    .to_string();
    settle(&h, case).await;
    (h, arn)
}

/// Store, and settle, the resource the row's own call acts on — the row's PREREQUISITE, which
/// owes the clock nothing by the time the row's own record is stored.
async fn prepare(h: &Harness, row: &Row, base_provider: &str) -> Option<String> {
    match row.pending {
        Pending::Registration | Pending::Association { .. } | Pending::EndpointCreate => None,
        Pending::Deregistration => {
            let reply = register(
                h,
                &registration(OTHER_DIRECTORY, OTHER_PRODUCT, &[ROW_SUBNET], None),
            )
            .await;
            let arn = as_str(
                &reply.ok(row.case)["IdentityProviderSummary"],
                "IdentityProviderArn",
                row.case,
            )
            .to_string();
            settle(h, row.case).await;
            Some(arn)
        }
        Pending::Disassociation { instance, username } => {
            let reply = associate(h, &association_request(instance, username)).await;
            let arn = as_str(
                &reply.ok(row.case)["InstanceUserSummary"],
                "InstanceUserArn",
                row.case,
            )
            .to_string();
            settle(h, row.case).await;
            Some(arn)
        }
        Pending::SubscriptionTeardown { username } => {
            // A subscription is stored SUBSCRIBED, a terminal status that owes the clock
            // nothing, so there is nothing to settle before the stop below.
            let reply = start_subscription(
                h,
                &json!({
                    "Username": username,
                    "IdentityProvider": provider_union(BASE_DIRECTORY),
                    "Product": BASE_PRODUCT,
                }),
            )
            .await;
            let arn = as_str(
                &reply.ok(row.case)["ProductUserSummary"],
                "ProductUserArn",
                row.case,
            )
            .to_string();
            Some(arn)
        }
        Pending::EndpointDeletion => {
            let reply = create_endpoint(h, &ENDPOINTS[0].request(base_provider)).await;
            let arn = as_str(reply.ok(row.case), "LicenseServerEndpointArn", row.case).to_string();
            settle(h, row.case).await;
            Some(arn)
        }
    }
}

/// An `AssociateUser` body for `username` on the account instance `instance` indexes, through
/// the base registration.
fn association_request(instance: usize, username: &str) -> serde_json::Value {
    json!({
        "Username": username,
        "InstanceId": held_instance(instance),
        "IdentityProvider": provider_union(BASE_DIRECTORY),
    })
}

/// Store the record that TRIGGERS each row's sweep — a user association, stored before the
/// row's own record and so due before it — answering its `InstanceUserArn`.
async fn store_trigger(h: &Harness, case: &str) -> String {
    let reply = associate(h, &association_request(TRIGGER_INSTANCE, TRIGGER_USERNAME)).await;
    as_str(
        &reply.ok(case)["InstanceUserSummary"],
        "InstanceUserArn",
        case,
    )
    .to_string()
}

/// Make the row's own call — the one that leaves its record in a non-terminal status —
/// answering the ARN of the record it left pending.
async fn start_pending(
    h: &Harness,
    row: &Row,
    base_provider: &str,
    prepared: Option<String>,
) -> String {
    let prepared = || {
        prepared.clone().unwrap_or_else(|| {
            panic!("case {:?}: this row prepares the resource it acts on", row.case)
        })
    };
    match row.pending {
        Pending::Registration => {
            let reply = register(
                h,
                &registration(OTHER_DIRECTORY, OTHER_PRODUCT, &[ROW_SUBNET], None),
            )
            .await;
            as_str(
                &reply.ok(row.case)["IdentityProviderSummary"],
                "IdentityProviderArn",
                row.case,
            )
            .to_string()
        }
        Pending::Deregistration => {
            let arn = prepared();
            deregister(
                h,
                &deregistration(&provider_union(OTHER_DIRECTORY), OTHER_PRODUCT),
            )
            .await
            .ok(row.case);
            arn
        }
        Pending::Association { instance, username } => {
            let reply = associate(h, &association_request(instance, username)).await;
            as_str(
                &reply.ok(row.case)["InstanceUserSummary"],
                "InstanceUserArn",
                row.case,
            )
            .to_string()
        }
        Pending::Disassociation { .. } => {
            let arn = prepared();
            disassociate(h, &json!({ "InstanceUserArn": arn }))
                .await
                .ok(row.case);
            arn
        }
        Pending::SubscriptionTeardown { .. } => {
            let arn = prepared();
            stop_subscription(h, &json!({ "ProductUserArn": arn }))
                .await
                .ok(row.case);
            arn
        }
        Pending::EndpointCreate => {
            let reply = create_endpoint(h, &ENDPOINTS[0].request(base_provider)).await;
            as_str(reply.ok(row.case), "LicenseServerEndpointArn", row.case).to_string()
        }
        Pending::EndpointDeletion => {
            let arn = prepared();
            delete_endpoint(h, &arn).await.ok(row.case);
            arn
        }
    }
}

/// Read the row's record back through the reporting operation of its kind — the ONE request
/// that runs the sweep — answering the status it reports for it.
async fn report(h: &Harness, row: &Row, arn: &str) -> String {
    match row.pending {
        Pending::Registration | Pending::Deregistration => {
            let reply = list_providers(h, &json!({})).await;
            summary_status(
                &reply,
                "IdentityProviderSummaries",
                "IdentityProviderArn",
                arn,
                row.case,
            )
        }
        Pending::Association { instance, .. } | Pending::Disassociation { instance, .. } => {
            let reply =
                list_associations(h, &provider_union(BASE_DIRECTORY), &held_instance(instance))
                    .await;
            summary_status(
                &reply,
                "InstanceUserSummaries",
                "InstanceUserArn",
                arn,
                row.case,
            )
        }
        Pending::SubscriptionTeardown { .. } => {
            let reply = list_subscriptions(
                h,
                &json!({
                    "IdentityProvider": provider_union(BASE_DIRECTORY),
                    "Product": BASE_PRODUCT,
                }),
            )
            .await;
            summary_status(
                &reply,
                "ProductUserSummaries",
                "ProductUserArn",
                arn,
                row.case,
            )
        }
        Pending::EndpointCreate | Pending::EndpointDeletion => {
            endpoint_status(h, arn, row.case).await
        }
    }
}

/// RULE: the sweep that materialises the transitions the clock has made due recomputes the
/// store's pending guard from what the records it visited still owe, asked of each record
/// itself — so whichever kind of record, and whichever operation left it in a non-terminal
/// status, a sweep that advances an unrelated record leaves that record pending with its own
/// due instant as the guard, and the next sweep materialises it.
#[tokio::test]
async fn the_sweep_recomputes_the_guard_from_what_the_records_still_owe() {
    for row in ROWS {
        let (h, base_provider) = setup().await;
        let prepared = prepare(&h, row, &base_provider).await;
        assert_eq!(
            guard(&h),
            None,
            "case {:?}: precondition — the store owes nothing before the row's records are \
             stored, got {:?} with {:?} owed",
            row.case,
            guard(&h),
            owed(&h)
        );

        // The record that triggers the sweep goes in first, so the clock owes it first; the
        // row's own record follows a fraction of a span later, so it is still pending when
        // the trigger falls due. No request runs in between, so nothing sweeps yet.
        let trigger = store_trigger(&h, row.case).await;
        tokio::time::sleep(STAGGER).await;
        let pending_arn = start_pending(&h, row, &base_provider, prepared).await;

        let owed_now = owed(&h);
        let due_of = |arn: &str| {
            owed_now
                .iter()
                .find(|(held, _)| held == arn)
                .map(|(_, due)| *due)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: {arn} must owe a transition, got {owed_now:?}",
                        row.case
                    )
                })
        };
        assert_eq!(
            owed_now.len(),
            2,
            "case {:?}: exactly the trigger and the row's own record owe a transition, got \
             {owed_now:?}",
            row.case
        );
        let trigger_due = due_of(&trigger);
        let pending_due = due_of(&pending_arn);
        assert!(
            trigger_due < pending_due,
            "case {:?}: the trigger must fall due before the row's own record, got \
             {trigger_due:?} against {pending_due:?}",
            row.case
        );

        // The instant the trigger falls due is read off the trigger's own record, so the
        // window is the one the service really chose rather than one this suite computed.
        wait_until(trigger_due).await;
        assert!(
            SystemTime::now() < pending_due,
            "case {:?}: the row's own record must still be pending when the sweep runs — \
             reaching {trigger_due:?} took longer than the {STAGGER:?} stagger, leaving \
             {pending_due:?} already due",
            row.case
        );

        // THE RULE: one reporting request sweeps. It applies the transition the trigger
        // owed, and recomputes the guard from what the records it visited still owe — the
        // row's own record, whose status and due instant it reads off that record.
        let status = report(&h, row, &pending_arn).await;
        assert_eq!(
            status,
            row.pending.status(),
            "case {:?}: the sweep must leave the row's record in its non-terminal status, got \
             {status:?}",
            row.case
        );
        let left = owed(&h);
        assert_eq!(
            left,
            vec![(pending_arn.clone(), pending_due)],
            "case {:?}: after the sweep the trigger owes nothing and the row's own record owes \
             exactly what it carried, got {left:?}",
            row.case
        );
        assert_eq!(
            guard(&h),
            Some(pending_due),
            "case {:?}: the guard the sweep recomputed must be the transition the record left \
             in the store owes, got {:?} against {left:?}",
            row.case,
            guard(&h)
        );

        // And the guard it left is what the next sweep runs on: once the row's own span has
        // elapsed too, a read materialises its transition and the store owes nothing.
        settle(&h, row.case).await;
        let left = owed(&h);
        assert!(
            left.is_empty() && guard(&h).is_none(),
            "case {:?}: once every transition is materialised nothing is owed and the guard is \
             clear, got guard {:?} with {left:?} owed",
            row.case,
            guard(&h)
        );
    }
}
