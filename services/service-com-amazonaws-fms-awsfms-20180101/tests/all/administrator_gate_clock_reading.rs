// SPDX-License-Identifier: CC0-1.0

//! Tests for the instant an organisation-scoped Firewall Manager call is
//! authorized at.
//!
//! The rule asserted here, in one sentence: a request decides its administrator
//! gate and everything it reports and writes from ONE instant, so the gate
//! admits a call exactly when the default-administrator association is still an
//! administrator at that instant — never at a second, later instant of its own.
//!
//! Two classes the rule ranges over:
//!
//! * the INSTANT, relative to the completion instant of a requested tear-down:
//!   long before it, a nanosecond either side of it, exactly ON it, and long
//!   after — so the half-open boundary is pinned rather than only its interior;
//! * the ROLE the association carries, every value of the model's own
//!   `AccountRoleStatus` catalog
//!   (`com.amazonaws.fms#AccountRoleStatus`), crossed with where the completion
//!   instant sits.
//!
//! At every row the gate's decision, the predicate the record answers and the
//! `OrganizationStatus` an enumeration reports are read at the SAME instant and
//! must agree: an administrator the gate admits is one the response reports as
//! still being an administrator.
//!
//! The wire test below then drives every gated operation that takes its own
//! reading of the clock through [`crate::policy_lifecycle`]'s awsJson1_1
//! harness, so the assertions are about the WIRE response.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::ACCOUNT_ROLE_STATUS_VALUES;
use service_com_amazonaws_fms_awsfms_20180101::state;

use crate::admin_account_scopes::organization_status;
use crate::administrator_gated_operations::with_store;
use crate::policy_lifecycle::{call, emulator_with_engine, error_code, ACCOUNT, REGION};

/// The error code an organisation-scoped call answers when no administrator
/// association is active at the instant it decides from.
const ACCESS_DENIED: &str = "AccessDeniedException";

/// An account id that is not the caller, for the gated writes below. The model
/// constrains `AWSAccountId` to `^[0-9]+$`, length 1..1024.
const OTHER_ACCOUNT: &str = "123456789012";

/// A `Base62Id` (the model's `com.amazonaws.fms#Base62Id`: 22 characters
/// matching `^[a-z0-9A-Z]{22}$`) that names no resource set.
const UNKNOWN_RESOURCE_SET_ID: &str = "0123456789AbCdEfGhIjKl";

/// The default-administrator association as the store holds it: `role_status`
/// with `removal_due` as the completion instant of a requested tear-down (`None`
/// when no tear-down was asked for).
///
/// `pub(crate)` so a sibling suite in this test target seeds a tear-down in
/// flight through this ONE builder rather than a second copy of the record.
pub(crate) fn association(
    role_status: &str,
    removal_due: Option<SystemTime>,
) -> state::AdminAccountRecord {
    state::AdminAccountRecord {
        account_id: ACCOUNT.to_string(),
        role_status: role_status.to_string(),
        associated_at: SystemTime::now() - Duration::from_secs(3600),
        removal_due,
    }
}

/// The `OrganizationStatus` the enumeration reports for the DEFAULT
/// administrator at `now`, read off the store's own projection.
fn reported_status(
    store: &state::ComAmazonawsFmsAwsfms20180101State,
    now: SystemTime,
) -> Option<String> {
    store
        .admin_account_summaries(now)
        .into_values()
        .find(|summary| summary.default_admin)
        .and_then(|summary| summary.status)
}

/// Whether the gate admits an organisation-scoped call decided at `now`.
fn gate_admits(store: &state::ComAmazonawsFmsAwsfms20180101State, now: SystemTime) -> bool {
    state::require_administrator_at::<()>(store, REGION, now).is_ok()
}

/// At the completion instant of a requested tear-down the association stops
/// being an administrator, and the gate, the record's own predicate and the
/// status an enumeration reports all answer that from the SAME instant — on
/// either side of the boundary and exactly on it.
#[test]
fn the_gate_turns_over_at_the_completion_instant_the_response_reports_from() {
    let (_router, engine) = emulator_with_engine();
    let started = SystemTime::now();
    let due = started + state::DISASSOCIATION_SPAN;
    with_store(&engine, |store| {
        store.admin_account = Some(association(
            state::ROLE_STATUS_PENDING_DELETION,
            Some(due),
        ));
    });

    // Each row: the instant the request would decide from, whether the
    // association is still an administrator then, and the status reported for it.
    let probes: &[(&str, SystemTime, bool, &str)] = &[
        (
            "long before the completion instant",
            started,
            true,
            "OFFBOARDING",
        ),
        (
            "a nanosecond before the completion instant",
            due - Duration::from_nanos(1),
            true,
            "OFFBOARDING",
        ),
        (
            "exactly at the completion instant",
            due,
            false,
            "OFFBOARDING_COMPLETE",
        ),
        (
            "a nanosecond after the completion instant",
            due + Duration::from_nanos(1),
            false,
            "OFFBOARDING_COMPLETE",
        ),
        (
            "long after the completion instant",
            due + Duration::from_secs(3600),
            false,
            "OFFBOARDING_COMPLETE",
        ),
    ];

    with_store(&engine, |store| {
        for (case, probe, still_an_administrator, status) in probes {
            let admits = gate_admits(store, *probe);
            assert_eq!(
                admits, *still_an_administrator,
                "case {case:?}: expected the gate to admit the call {still_an_administrator}, \
                 got {admits}"
            );
            let record = store
                .admin_account
                .as_ref()
                .expect("the seeded association");
            assert_eq!(
                record.is_active_at(*probe),
                admits,
                "case {case:?}: the gate and the association's own predicate must answer the \
                 same at one instant, got {admits} from the gate"
            );
            let reported = reported_status(store, *probe);
            assert_eq!(
                reported.as_deref(),
                Some(organization_status(status)),
                "case {case:?}: expected the default administrator to be reported {status}, got \
                 {reported:?}"
            );
        }
    });
}

/// Whatever role the association carries and wherever the completion instant
/// sits, the gate's decision at an instant agrees with the status the response
/// reports at that same instant: a call the gate admits is answered with an
/// administrator that has not been removed, and one it refuses is an
/// administrator already removed.
#[test]
fn the_gate_agrees_with_the_reported_status_for_every_modelled_role() {
    let (_router, engine) = emulator_with_engine();
    let now = SystemTime::now();
    // Where the completion instant of a requested tear-down sits relative to the
    // instant the request decides from: none asked for, still ahead, already
    // passed.
    let completions: &[(&str, Option<SystemTime>)] = &[
        ("no tear-down requested", None),
        (
            "a tear-down completing later",
            Some(now + Duration::from_secs(3600)),
        ),
        (
            "a tear-down already completed",
            Some(now - Duration::from_millis(1)),
        ),
    ];

    // Every role the model's `AccountRoleStatus` carries, so a role the issue
    // did not cite is covered too.
    for role_status in ACCOUNT_ROLE_STATUS_VALUES {
        for (completion, removal_due) in completions {
            let case = format!("{role_status} / {completion}");
            with_store(&engine, |store| {
                store.admin_account = Some(association(role_status, *removal_due));
                let admits = gate_admits(store, now);
                let reported = reported_status(store, now);
                let removed = reported.as_deref()
                    == Some(organization_status("OFFBOARDING_COMPLETE"));
                assert_eq!(
                    admits, !removed,
                    "case {case}: the gate must admit the call exactly when the administrator it \
                     reports has not been removed, got admitted {admits} with {reported:?}"
                );
                let record = store
                    .admin_account
                    .as_ref()
                    .expect("the seeded association");
                assert_eq!(
                    record.is_active_at(now),
                    admits,
                    "case {case}: the gate and the association's own predicate must answer the \
                     same at one instant, got {admits} from the gate"
                );
            });
        }
    }
}

/// One tear-down state of the association, as a request sees it.
struct AssociationCase {
    name: &'static str,
    role_status: &'static str,
    /// Where the completion instant sits relative to the instant the request
    /// decides from, in milliseconds; `None` when no tear-down was requested.
    completes_in_millis: Option<i64>,
    /// Whether the association is still an administrator at that instant.
    still_an_administrator: bool,
    /// The `OrganizationStatus` the enumeration reports for it then.
    status: &'static str,
}

/// The class the wire rule is asserted over: the rows differ in the role the
/// association carries and in which side of the request's instant the
/// completion instant of a requested tear-down falls on.
fn association_cases() -> Vec<AssociationCase> {
    vec![
        AssociationCase {
            name: "no tear-down requested",
            role_status: state::ROLE_STATUS_READY,
            completes_in_millis: None,
            still_an_administrator: true,
            status: "ONBOARDING_COMPLETE",
        },
        AssociationCase {
            name: "a tear-down completing in an hour",
            role_status: state::ROLE_STATUS_PENDING_DELETION,
            completes_in_millis: Some(3_600_000),
            still_an_administrator: true,
            status: "OFFBOARDING",
        },
        AssociationCase {
            name: "a role being deleted, completing in half a minute",
            role_status: state::ROLE_STATUS_DELETING,
            completes_in_millis: Some(30_000),
            still_an_administrator: true,
            status: "OFFBOARDING",
        },
        AssociationCase {
            name: "a tear-down that completed a millisecond ago",
            role_status: state::ROLE_STATUS_PENDING_DELETION,
            completes_in_millis: Some(-1),
            still_an_administrator: false,
            status: "OFFBOARDING_COMPLETE",
        },
        AssociationCase {
            name: "a tear-down that completed an hour ago",
            role_status: state::ROLE_STATUS_DELETING,
            completes_in_millis: Some(-3_600_000),
            still_an_administrator: false,
            status: "OFFBOARDING_COMPLETE",
        },
        AssociationCase {
            name: "a role already deleted",
            role_status: state::ROLE_STATUS_DELETED,
            completes_in_millis: None,
            still_an_administrator: false,
            status: "OFFBOARDING_COMPLETE",
        },
        AssociationCase {
            name: "a role already deleted, with a completion instant still ahead",
            role_status: state::ROLE_STATUS_DELETED,
            completes_in_millis: Some(3_600_000),
            still_an_administrator: false,
            status: "OFFBOARDING_COMPLETE",
        },
    ]
}

/// Every gated operation that takes its own reading of the clock, with a
/// schema-valid request: these are the handlers whose gate must decide from the
/// SAME reading as the rest of the request. The payloads differ in form — no
/// members, an account id, a document, ARNs, a vendor enum — so the gate's
/// answer cannot depend on the shape of the request.
fn gated_calls() -> Vec<(&'static str, Value)> {
    vec![
        ("ListAdminAccountsForOrganization", json!({})),
        ("GetAdminScope", json!({ "AdminAccount": OTHER_ACCOUNT })),
        ("PutAdminAccount", json!({ "AdminAccount": OTHER_ACCOUNT })),
        (
            "PutPolicy",
            json!({
                "Policy": {
                    "PolicyName": "one-reading",
                    "SecurityServicePolicyData": { "Type": "WAFV2" },
                    "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
                    "ExcludeResourceTags": false,
                    "RemediationEnabled": false,
                },
            }),
        ),
        (
            "PutResourceSet",
            json!({
                "ResourceSet": {
                    "Name": "one-reading",
                    "ResourceTypeList": ["AWS::NetworkFirewall::Firewall"],
                },
            }),
        ),
        (
            "PutAppsList",
            json!({
                "AppsList": {
                    "ListName": "one-reading",
                    "AppsList": [{ "AppName": "https", "Protocol": "tcp", "Port": 443 }],
                },
            }),
        ),
        (
            "PutProtocolsList",
            json!({
                "ProtocolsList": { "ListName": "one-reading", "ProtocolsList": ["TCP"] },
            }),
        ),
        (
            "BatchAssociateResource",
            json!({
                "ResourceSetIdentifier": UNKNOWN_RESOURCE_SET_ID,
                "Items": [format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/x")],
            }),
        ),
        (
            "BatchDisassociateResource",
            json!({
                "ResourceSetIdentifier": UNKNOWN_RESOURCE_SET_ID,
                "Items": [format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/x")],
            }),
        ),
        (
            "AssociateThirdPartyFirewall",
            json!({ "ThirdPartyFirewall": "PALO_ALTO_NETWORKS_CLOUD_NGFW" }),
        ),
        (
            "DisassociateThirdPartyFirewall",
            json!({ "ThirdPartyFirewall": "PALO_ALTO_NETWORKS_CLOUD_NGFW" }),
        ),
        (
            "GetThirdPartyFirewallAssociationStatus",
            json!({ "ThirdPartyFirewall": "FORTIGATE_CLOUD_NATIVE_FIREWALL" }),
        ),
        (
            "ListThirdPartyFirewallFirewallPolicies",
            json!({
                "ThirdPartyFirewall": "PALO_ALTO_NETWORKS_CLOUD_NGFW",
                "MaxResults": 5,
            }),
        ),
    ]
}

/// Over the wire: a gated call is refused with `AccessDeniedException` exactly
/// when the association is no longer an administrator at the instant the request
/// decides from, and while it still is, every gated operation is answered from
/// its own lookup instead — and the enumeration reports the administrator the
/// gate admitted, with the status that holds at that instant.
#[tokio::test]
async fn a_gated_call_is_refused_exactly_when_the_association_is_no_longer_an_administrator() {
    for case in association_cases() {
        let name = case.name;
        let (router, engine) = emulator_with_engine();
        let now = SystemTime::now();
        let removal_due = case.completes_in_millis.map(|millis| {
            if millis >= 0 {
                now + Duration::from_millis(u64::try_from(millis).unwrap_or_default())
            } else {
                now - Duration::from_millis(u64::try_from(-millis).unwrap_or_default())
            }
        });
        with_store(&engine, |store| {
            store.admin_account = Some(association(case.role_status, removal_due));
        });

        for (operation, request) in gated_calls() {
            let (status, body) = call(&router, operation, &request).await;
            let code = error_code(&body);
            if case.still_an_administrator {
                assert_ne!(
                    code, ACCESS_DENIED,
                    "case {name:?} / {operation}: an administrator that is still associated at \
                     the request's instant must not be refused, got {status} with {body}"
                );
            } else {
                assert_eq!(
                    (status, code.as_str()),
                    (400, ACCESS_DENIED),
                    "case {name:?} / {operation}: an administrator already removed at the \
                     request's instant must be refused, got {status} with {body}"
                );
            }
        }

        // What the enumeration reports agrees with the decision the gate made:
        // the administrator is there, with the status that holds then, or the
        // call reported nothing at all.
        let (status, body) = call(&router, "ListAdminAccountsForOrganization", &json!({})).await;
        let default_admin: Vec<&Value> = body["AdminAccounts"]
            .as_array()
            .map(|summaries| {
                summaries
                    .iter()
                    .filter(|summary| summary["DefaultAdmin"].as_bool() == Some(true))
                    .collect()
            })
            .unwrap_or_default();
        if case.still_an_administrator {
            assert_eq!(
                status, 200,
                "case {name:?}: the enumeration must be served, got {status} with {body}"
            );
            assert_eq!(
                default_admin.len(),
                1,
                "case {name:?}: a served call must report the administrator its gate admitted, \
                 got {body}"
            );
            assert_eq!(
                default_admin[0]["Status"].as_str(),
                Some(organization_status(case.status)),
                "case {name:?}: expected the default administrator reported {}, got {body}",
                case.status
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, ACCESS_DENIED),
                "case {name:?}: the enumeration must be refused, got {status} with {body}"
            );
            assert!(
                default_admin.is_empty(),
                "case {name:?}: a refused call must report nothing, got {body}"
            );
        }

        // And the role the store holds afterwards is the one the request's
        // instant implied: a refused call is a completed tear-down, materialised
        // rather than re-derived; a served one left the role as it was.
        let (status, body) = call(&router, "GetAdminAccount", &json!({})).await;
        let expected_role = if case.still_an_administrator {
            case.role_status
        } else {
            state::ROLE_STATUS_DELETED
        };
        assert_eq!(
            (status, body["RoleStatus"].as_str()),
            (200, Some(expected_role)),
            "case {name:?}: expected the stored role to be {expected_role}, got {status} with \
             {body}"
        );
    }
}
