// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE rule every creating writer of this service obeys, driven
//! over the real wire path: every call goes through the router's `dispatch`, so the
//! request is deserialized, constraint-checked, handled and serialized exactly as a
//! client's would be.
//!
//! The RULE, asserted over a TABLE of inputs rather than one request: a record stored in a
//! non-terminal status is stored TOGETHER with the status transition it owes the clock —
//! the store's pending guard is exactly the earliest transition the records it holds owe,
//! whichever operation did the storing — and that transition is materialised on a later
//! read. The rows differ along which creating operation stores the record (and so which
//! writer schedules the transition: `RegisterIdentityProvider`, `AssociateUser`,
//! `CreateLicenseServerEndpoint`), the directory-id form the record hangs off, the
//! product, the instance and user name an association names, whether the user carries a
//! domain, and which license-server settings an endpoint carries.
//!
//! The guard itself is read directly from the store, because it is not a reportable value:
//! it is the O(1) field that decides whether a request sweeps for due transitions at all,
//! and the only wire-visible consequence of getting it wrong is a transition that falls due
//! and is never materialised until some unrelated record happens to fall due too. So each
//! row asserts both halves: the guard the writer wrote, and the status a later read reports.

use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::state::{
    seed_account_instances, OwedTransition,
};
use service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState as ServiceState;

// ONE wire harness per service: the dispatch plumbing and the register/associate/create
// helpers come from the sibling suites rather than a second copy.
use super::identity_provider_subscriptions::{
    as_str, call, harness, list_providers, provider_union, register, Harness, Reply,
    PAST_THE_OWED_TRANSITION, PRODUCTS,
};
use super::license_server_endpoints::{create_endpoint, list_endpoints, ENDPOINTS};
use super::resource_resolution_refusals::{associate, registration};

/// `ListUserAssociations` is bound to POST /user/ListUserAssociations (smithy_query:
/// operation ListUserAssociations, http.uri) — the reporting operation an association's
/// owed transition is observed through.
async fn list_associations(h: &Harness, body: &Value) -> Reply {
    call(h, "/user/ListUserAssociations", body).await
}

/// The subnet every registration in this suite provisions into — one the harness holds in
/// the account's own EC2 store.
const ROW_SUBNET: &str = "subnet-0123456789abcdef0";

/// Which creating operation a row drives, and so which of the one-writers-per-kind stores
/// its record.
#[derive(Debug, Clone, Copy)]
enum Create {
    /// `RegisterIdentityProvider`, stored by `state::insert_identity_provider`.
    Registration,
    /// `AssociateUser`, stored by `state::insert_instance_user`: the EC2 instance (an index
    /// into the account's seeded fleet, so the row names one the authorization gate holds
    /// without hard-coding its id), the user name, and the domain the user's information
    /// came from when the row carries one.
    Association {
        instance: usize,
        username: &'static str,
        domain: Option<&'static str>,
    },
    /// `CreateLicenseServerEndpoint`, stored by `state::insert_license_server_endpoint`:
    /// `settings` indexes the endpoint rows of [`super::license_server_endpoints`], whose
    /// `LicenseServerSettings` the create carries.
    Endpoint { settings: usize },
}

impl Create {
    /// The non-terminal status the create answers with — the status that makes the record
    /// one the clock owes a transition.
    ///
    /// Spelled out here rather than read from the production constants, so a change to them
    /// fails a test instead of silently changing what this suite accepts.
    fn pending_status(&self) -> &'static str {
        match self {
            Create::Registration => "REGISTERING",
            Create::Association { .. } => "ASSOCIATING",
            Create::Endpoint { .. } => "PROVISIONING",
        }
    }

    /// The status the clock owes that record, which a read past the span must report.
    fn settled_status(&self) -> &'static str {
        match self {
            Create::Registration => "REGISTERED",
            Create::Association { .. } => "ASSOCIATED",
            Create::Endpoint { .. } => "PROVISIONED",
        }
    }
}

/// One record to create.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// `DirectoryId` matches `^(d|sd)-[0-9a-f]{10}$`; both forms appear below.
    directory_id: &'static str,
    /// From `RegisterIdentityProvider`'s `Product` documentation, through
    /// [`PRODUCTS`].
    product: &'static str,
    create: Create,
}

const ROWS: &[Row] = &[
    Row {
        case: "a registration of an aws-managed directory",
        directory_id: "d-0123456789",
        product: PRODUCTS[0],
        create: Create::Registration,
    },
    Row {
        case: "a registration of a directory shared into the account",
        directory_id: "sd-abcdef0123",
        product: PRODUCTS[4],
        create: Create::Registration,
    },
    Row {
        case: "an association of a user with no domain, on the first instance",
        directory_id: "d-0123456789",
        product: PRODUCTS[1],
        create: Create::Association {
            instance: 0,
            username: "alice",
            domain: None,
        },
    },
    Row {
        case: "an association of a domain-qualified user, on a third instance",
        directory_id: "d-00000000ff",
        product: PRODUCTS[2],
        create: Create::Association {
            instance: 2,
            username: "CAROL",
            domain: Some("eu.corp.example"),
        },
    },
    Row {
        case: "an endpoint whose settings name the secret by name",
        directory_id: "d-123456789a",
        product: PRODUCTS[4],
        create: Create::Endpoint { settings: 0 },
    },
    Row {
        case: "an endpoint whose settings name the secret by ARN, tagged",
        directory_id: "d-00000000ff",
        product: PRODUCTS[3],
        create: Create::Endpoint { settings: 1 },
    },
];

/// One EC2 instance of this service's own fleet seam (`state::seed_account_instances`),
/// which is what authorizes an association in a harness that mounts no EC2 beside it.
fn held_instance(index: usize) -> String {
    seed_account_instances()[index].0.clone()
}

/// The instant the store currently owes the clock a transition at, or `None` when it owes
/// none — the O(1) guard every writer of a non-terminal record has to leave behind.
pub(crate) fn guard(h: &Harness) -> Option<SystemTime> {
    let store = h.engine.state::<ServiceState>();
    let held = store.lock();
    held.pending_transition_due_at
}

/// Every transition the records in the store owe the clock, as `(ARN, due instant)`.
///
/// Read off the RECORDS, through the same trait the writers schedule by, so the assertion
/// below compares the guard with what the stored records actually owe rather than with a
/// due instant this suite computed from a span of its own.
pub(crate) fn owed(h: &Harness) -> Vec<(String, SystemTime)> {
    let store = h.engine.state::<ServiceState>();
    let held = store.lock();
    let mut owed: Vec<(String, SystemTime)> = Vec::new();
    for (arn, record) in &held.identity_providers {
        owed.extend(record.owed_transition_due_at().map(|due| (arn.clone(), due)));
    }
    for (arn, record) in &held.instance_users {
        owed.extend(record.owed_transition_due_at().map(|due| (arn.clone(), due)));
    }
    for (arn, record) in &held.product_users {
        owed.extend(record.owed_transition_due_at().map(|due| (arn.clone(), due)));
    }
    for (arn, record) in &held.license_server_endpoints {
        owed.extend(record.owed_transition_due_at().map(|due| (arn.clone(), due)));
    }
    owed.sort();
    owed
}

/// Register the identity provider a row hangs off, answering the ARN it minted.
async fn register_row(h: &Harness, row: &Row) -> String {
    let reply = register(
        h,
        &registration(row.directory_id, row.product, &[ROW_SUBNET], None),
    )
    .await;
    as_str(
        &reply.ok(row.case)["IdentityProviderSummary"],
        "IdentityProviderArn",
        row.case,
    )
    .to_string()
}

/// A plain read, which is how a client reaches the sweep that materialises whatever the
/// clock has made due.
async fn read_through(h: &Harness, case: &str) {
    list_providers(h, &json!({})).await.ok(case);
}

/// Drive one row's create, answering the ARN of the record it stored and the status the
/// create reported for it.
async fn create(h: &Harness, row: &Row, provider_arn: &str) -> (String, String) {
    match row.create {
        Create::Registration => {
            // The registration IS this row's create, so it is made here rather than as a
            // prerequisite.
            let reply = register(
                h,
                &registration(row.directory_id, row.product, &[ROW_SUBNET], None),
            )
            .await;
            let summary = reply.ok(row.case)["IdentityProviderSummary"].clone();
            (
                as_str(&summary, "IdentityProviderArn", row.case).to_string(),
                as_str(&summary, "Status", row.case).to_string(),
            )
        }
        Create::Association {
            instance,
            username,
            domain,
        } => {
            let mut body = json!({
                "Username": username,
                "InstanceId": held_instance(instance),
                "IdentityProvider": provider_union(row.directory_id),
            });
            if let Some(domain) = domain {
                body["Domain"] = json!(domain);
            }
            let reply = associate(h, &body).await;
            let summary = reply.ok(row.case)["InstanceUserSummary"].clone();
            (
                as_str(&summary, "InstanceUserArn", row.case).to_string(),
                as_str(&summary, "Status", row.case).to_string(),
            )
        }
        Create::Endpoint { settings } => {
            let reply = create_endpoint(h, &ENDPOINTS[settings].request(provider_arn)).await;
            let arn = as_str(reply.ok(row.case), "LicenseServerEndpointArn", row.case).to_string();
            // The create answers the ARNs only, so the status it stored is read back off
            // the list — the status a client sees for a freshly created endpoint.
            let status = endpoint_status(h, &arn, row.case).await;
            (arn, status)
        }
    }
}

/// The provisioning status `ListLicenseServerEndpoints` reports for `arn`.
pub(crate) async fn endpoint_status(h: &Harness, arn: &str, case: &str) -> String {
    let reply = list_endpoints(h, &json!({})).await;
    let body = reply.ok(case);
    let endpoints = body["LicenseServerEndpoints"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no LicenseServerEndpoints array in {body:?}"));
    let listed = endpoints
        .iter()
        .find(|endpoint| as_str(endpoint, "LicenseServerEndpointArn", case) == arn)
        .unwrap_or_else(|| panic!("case {case:?}: {arn} is not reported in {body:?}"));
    as_str(listed, "LicenseServerEndpointProvisioningStatus", case).to_string()
}

/// The status a later read reports for the record `arn` names, read through the reporting
/// operation of that record's kind.
async fn reported_status(h: &Harness, row: &Row, arn: &str) -> String {
    match row.create {
        Create::Registration => {
            let reply = list_providers(h, &json!({})).await;
            summary_status(&reply, "IdentityProviderSummaries", "IdentityProviderArn", arn, row.case)
        }
        Create::Association { instance, .. } => {
            let reply = list_associations(
                h,
                &json!({
                    "InstanceId": held_instance(instance),
                    "IdentityProvider": provider_union(row.directory_id),
                }),
            )
            .await;
            summary_status(&reply, "InstanceUserSummaries", "InstanceUserArn", arn, row.case)
        }
        Create::Endpoint { .. } => endpoint_status(h, arn, row.case).await,
    }
}

/// The `Status` the summary addressed by `arn` carries in a list reply.
pub(crate) fn summary_status(
    reply: &Reply,
    list: &str,
    arn_member: &str,
    arn: &str,
    case: &str,
) -> String {
    let body = reply.ok(case);
    let summaries = body[list]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no {list} array in {body:?}"));
    let summary = summaries
        .iter()
        .find(|summary| as_str(summary, arn_member, case) == arn)
        .unwrap_or_else(|| panic!("case {case:?}: {arn} is not reported in {body:?}"));
    as_str(summary, "Status", case).to_string()
}

/// RULE: whichever operation stores a record in a non-terminal status, the writer that
/// stores it also schedules the transition that record owes the clock — the store's pending
/// guard is exactly the due instant the stored record carries — and the transition is
/// materialised on a later read.
#[tokio::test]
async fn every_creating_writer_schedules_the_transition_its_record_owes() {
    for row in ROWS {
        // A harness per row, so the guard this row's create leaves is this row's own.
        let h = harness();

        // The registration an association or an endpoint hangs off is a PREREQUISITE, not
        // the create under test: it is settled first, so the store owes nothing when the
        // create below runs and the guard that create leaves can only be its own.
        let provider_arn = match row.create {
            Create::Registration => String::new(),
            _ => {
                let arn = register_row(&h, row).await;
                tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
                read_through(&h, row.case).await;
                arn
            }
        };
        assert_eq!(
            guard(&h),
            None,
            "case {:?}: precondition — nothing is owed before the create, got {:?} with {:?} \
             owed",
            row.case,
            guard(&h),
            owed(&h)
        );

        let (arn, status) = create(&h, row, &provider_arn).await;
        assert_eq!(
            status,
            row.create.pending_status(),
            "case {:?}: the create must store the record in its non-terminal status, got \
             {status:?}",
            row.case
        );

        // THE RULE: the record the create stored owes the clock a transition, and the guard
        // the writer left is exactly that record's own due instant.
        let owed_now = owed(&h);
        assert_eq!(
            owed_now.len(),
            1,
            "case {:?}: exactly the record this create stored owes a transition, got \
             {owed_now:?}",
            row.case
        );
        assert_eq!(
            owed_now[0].0, arn,
            "case {:?}: the record that owes a transition is the one the create stored, got \
             {owed_now:?}",
            row.case
        );
        assert_eq!(
            guard(&h),
            Some(owed_now[0].1),
            "case {:?}: the writer must schedule the due instant the stored record carries, \
             got {:?} against {owed_now:?}",
            row.case,
            guard(&h)
        );

        // And the guard is what a later read sweeps on: the transition the clock owed is
        // materialised, and nothing is left owed.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let settled = reported_status(&h, row, &arn).await;
        assert_eq!(
            settled,
            row.create.settled_status(),
            "case {:?}: the transition the clock owed must be materialised on a later read, \
             got {settled:?}",
            row.case
        );
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

/// RULE: the guard is the EARLIEST transition the store owes, so the records of different
/// kinds stored by different writers share one guard — each writer contributes its record's
/// own due instant and none of them overwrites another's.
#[tokio::test]
async fn the_guard_is_the_earliest_transition_the_writers_scheduled() {
    let h = harness();

    // One record of each kind, each left pending: a registration, an association through it
    // and an endpoint for it, in that order, with nothing waited for in between.
    let directory = "d-0123456789";
    let product = PRODUCTS[0];
    let provider_arn = register_row(
        &h,
        &Row {
            case: "the registration every kind below hangs off",
            directory_id: directory,
            product,
            create: Create::Registration,
        },
    )
    .await;
    associate(
        &h,
        &json!({
            "Username": "dave",
            "InstanceId": held_instance(1),
            "IdentityProvider": provider_union(directory),
        }),
    )
    .await
    .ok("the association");
    let endpoint_arn = {
        let reply = create_endpoint(&h, &ENDPOINTS[0].request(&provider_arn)).await;
        as_str(
            reply.ok("the endpoint"),
            "LicenseServerEndpointArn",
            "the endpoint",
        )
        .to_string()
    };

    let owed_now = owed(&h);
    assert_eq!(
        owed_now.len(),
        3,
        "each of the three records is stored in a non-terminal status, so each owes a \
         transition, got {owed_now:?}"
    );
    let earliest = owed_now.iter().map(|(_, due)| *due).min();
    assert_eq!(
        guard(&h),
        earliest,
        "the guard must be the earliest transition the records owe, got {:?} against \
         {owed_now:?}",
        guard(&h)
    );

    // Every one of the three is then materialised on a later read, through the one guard.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let registration_status = {
        let reply = list_providers(&h, &json!({})).await;
        summary_status(
            &reply,
            "IdentityProviderSummaries",
            "IdentityProviderArn",
            &provider_arn,
            "the registration",
        )
    };
    assert_eq!(
        registration_status, "REGISTERED",
        "the registration's owed transition is materialised, got {registration_status:?}"
    );
    let association_status = {
        let reply = list_associations(
            &h,
            &json!({
                "InstanceId": held_instance(1),
                "IdentityProvider": provider_union(directory),
            }),
        )
        .await;
        let body = reply.ok("the association");
        let summaries = body["InstanceUserSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("no InstanceUserSummaries array in {body:?}"));
        as_str(
            summaries
                .first()
                .unwrap_or_else(|| panic!("the association is still reported in {body:?}")),
            "Status",
            "the association",
        )
        .to_string()
    };
    assert_eq!(
        association_status, "ASSOCIATED",
        "the association's owed transition is materialised, got {association_status:?}"
    );
    let endpoint = endpoint_status(&h, &endpoint_arn, "the endpoint").await;
    assert_eq!(
        endpoint, "PROVISIONED",
        "the endpoint's owed transition is materialised, got {endpoint:?}"
    );
    let left = owed(&h);
    assert!(
        left.is_empty() && guard(&h).is_none(),
        "once every transition is materialised nothing is owed and the guard is clear, got \
         guard {:?} with {left:?} owed",
        guard(&h)
    );
}
