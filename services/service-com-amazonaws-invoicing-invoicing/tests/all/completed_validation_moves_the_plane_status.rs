// SPDX-License-Identifier: CC0-1.0

//! What a COMPLETED procurement-portal validation leaves for a reader to report.
//!
//! `VerifyProcurementPortalValidation` "submits a validation code to complete the
//! validation of a procurement portal preference … to confirm ownership and connectivity
//! of the configured procurement portal endpoint" (`smithy_query(query="operation",
//! operation="VerifyProcurementPortalValidation")`), and a plane's status is "the current
//! status of the e-invoice delivery preference" / of the purchase order retrieval
//! preference (`smithy_query(query="shape", shape="ProcurementPortalPreference")`) — so
//! completing the verification is what carries a plane off the status it holds while that
//! connection is unverified, and a reader has to see it.
//!
//! Every call goes through the same `MultiServiceRouter` the production router uses, with
//! the shared harness of `refused_procurement_portal_writes` rather than a copy of it. The
//! one thing read out of state is the issued CODE, which no modelled response carries.
//!
//! The rules under test, each spanned by a table rather than one request:
//!
//! * a completed validation moves every plane that was awaiting verification to the
//!   validated status and reports no reason for it, leaves a plane the configuration
//!   disables without a status at all, and writes no configuration — over every
//!   combination of the two enable flags, in the single-resource read and in the listing;
//! * a status an explicit status update already carried past the unverified one is KEPT,
//!   with its reason, by a later completed validation — over every status the enum admits
//!   and both planes;
//! * the write a completed validation performs is one version of the resource exactly when
//!   a plane moved: a validation completed with no plane awaiting verification advances
//!   neither `Version` nor `LastUpdateDate`.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    arn_of, emulator_with_engine, get_preference, issue_code, ok, PORTAL_NAMES,
};

/// The status an enabled plane holds while nothing has verified its portal connection, and
/// the status it reports once something has: two of the values
/// `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";
const VALIDATED: &str = "VALIDATED";

/// Every value `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`), so the rule is asserted on statuses this
/// flow does not itself produce as well as on the two it moves between.
const STATUS_VALUES: &[&str] = &[
    "PENDING_VERIFICATION",
    "VALIDATED",
    "TEST_INITIALIZED",
    "TEST_INITIALIZATION_FAILED",
    "TEST_FAILED",
    "ACTIVE",
    "SUSPENDED",
];

/// The two planes of a preference, each as the members the model spells for it: a label,
/// the flag that enables the plane, its status and its status reason.
const PLANES: &[(&str, &str, &str, &str)] = &[
    (
        "e-invoice delivery",
        "EinvoiceDeliveryEnabled",
        "EinvoiceDeliveryPreferenceStatus",
        "EinvoiceDeliveryPreferenceStatusReason",
    ),
    (
        "purchase order retrieval",
        "PurchaseOrderRetrievalEnabled",
        "PurchaseOrderRetrievalPreferenceStatus",
        "PurchaseOrderRetrievalPreferenceStatusReason",
    ),
];

/// Every configuration the two enable flags can state.
const FLAG_COMBINATIONS: &[(bool, bool)] = &[(true, true), (true, false), (false, true), (false, false)];

/// One contact, the single element `Contacts` (`@length(min: 1, max: 1)`) admits.
fn contact(name: &str) -> Value {
    json!([{ "Name": name, "Email": format!("{}@example.com", name.to_lowercase().replace(' ', ".")) }])
}

/// A create request for one identity, stating the two enable flags under test.
fn create_request(
    portal: &str,
    buyer: &str,
    supplier: &str,
    einvoice: bool,
    purchase_order: bool,
) -> Value {
    json!({
        "ProcurementPortalName": portal,
        "BuyerDomain": "NetworkID",
        "BuyerIdentifier": buyer,
        "SupplierDomain": "NetworkID",
        "SupplierIdentifier": supplier,
        "EinvoiceDeliveryEnabled": einvoice,
        "PurchaseOrderRetrievalEnabled": purchase_order,
        "Contacts": contact("Initial Contact"),
    })
}

/// The summary `ListProcurementPortalPreferences` reports for `arn`, so the rule is
/// asserted against both readers of a preference rather than only the single-resource one.
async fn summary_of(router: &MultiServiceRouter, arn: &str) -> Value {
    let body = ok(router, "ListProcurementPortalPreferences", &json!({})).await;
    body.get("ProcurementPortalPreferences")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected ProcurementPortalPreferences in {body}"))
        .iter()
        .find(|summary| {
            summary
                .get("ProcurementPortalPreferenceArn")
                .and_then(Value::as_str)
                == Some(arn)
        })
        .cloned()
        .unwrap_or_else(|| panic!("expected a summary for {arn} in {body}"))
}

/// What a reader reports for a preference APART from the members a status write moves, so
/// a configuration can be compared across a call that moves a status.
fn configuration(preference: &Value) -> Value {
    let mut configuration = preference.clone();
    if let Some(members) = configuration.as_object_mut() {
        for (_, _, status, reason) in PLANES {
            members.remove(*status);
            members.remove(*reason);
        }
        members.remove("Version");
        members.remove("LastUpdateDate");
    }
    configuration
}

/// The `Version` a reader reports for a preference.
fn version(preference: &Value) -> i64 {
    preference
        .get("Version")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected a Version in {preference}"))
}

/// The `LastUpdateDate` a reader reports for a preference.
fn last_update_date(preference: &Value) -> f64 {
    preference
        .get("LastUpdateDate")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("expected a LastUpdateDate in {preference}"))
}

/// The status and status reason a reader reports for one plane.
fn plane_state(reported: &Value, plane: usize) -> (Option<String>, Option<String>) {
    let (_, _, status, reason) = PLANES[plane];
    let read = |member: &str| {
        reported
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    (read(status), read(reason))
}

/// Complete the validation of `arn`: the send issues the code, the verify submits it.
async fn complete_validation(
    router: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    arn: &str,
) {
    let code = issue_code(router, engine, arn).await;
    let answered = ok(
        router,
        "VerifyProcurementPortalValidation",
        &json!({"ProcurementPortalPreferenceArn": arn, "Code": code}),
    )
    .await;
    assert_eq!(
        arn_of(&answered),
        arn,
        "case complete the validation of {arn}: the verify answers with the ARN it \
         validated, got {answered}"
    );
}

/// Assert one plane's reported state in the single-resource read and in the listing alike.
fn assert_plane_state(
    case: &str,
    reported: &Value,
    reader: &str,
    plane: usize,
    expected: (Option<&str>, Option<&str>),
) {
    let (label, flag, status_member, reason_member) = PLANES[plane];
    let (status, reason) = plane_state(reported, plane);
    let enabled = reported.get(flag).and_then(Value::as_bool);
    assert_eq!(
        status.as_deref(),
        expected.0,
        "case {case}: the {label} plane reports {flag} {enabled:?}, so the {reader} expected \
         {status_member} {:?}, got {status:?}",
        expected.0
    );
    assert_eq!(
        reason.as_deref(),
        expected.1,
        "case {case}: the {reader} expected {reason_member} {:?}, got {reason:?}",
        expected.1
    );
}

/// RULE: completing the verification of a resource's optional planes carries every plane
/// that was AWAITING verification to the verified status and leaves no reason stated for
/// it, while a plane the configuration disables keeps no status at all and no member of the
/// configuration is written — so every reader of the resource reports the completed
/// verification.
///
/// The rows are the four combinations of the two enable flags, each on a portal of its own,
/// so neither plane is asserted only while the other is enabled.
#[tokio::test]
async fn completing_a_validation_validates_every_plane_awaiting_verification() {
    let (router, engine) = emulator_with_engine();

    for (index, (einvoice, purchase_order)) in FLAG_COMBINATIONS.iter().enumerate() {
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_request(
                    portal,
                    &format!("BUYER-VERIFIED-{index}"),
                    &format!("SUPPLIER-VERIFIED-{index}"),
                    *einvoice,
                    *purchase_order,
                ),
            )
            .await,
        );
        let case = format!("({einvoice}, {purchase_order}) on {portal}");

        // Nothing has verified the connection yet, so an enabled plane is at the
        // unverified status and a disabled one has no status to be about.
        let before = get_preference(&router, &arn).await;
        let enabled_flags = [*einvoice, *purchase_order];
        for (plane, enabled) in enabled_flags.iter().enumerate() {
            let expected = enabled.then_some(PENDING_VERIFICATION);
            assert_plane_state(
                &case,
                &before,
                "read before the verify",
                plane,
                (expected, None),
            );
        }

        complete_validation(&router, &engine, &arn).await;

        let after = get_preference(&router, &arn).await;
        let summary = summary_of(&router, &arn).await;
        for (reader, reported) in [("read", &after), ("listing", &summary)] {
            for (plane, enabled) in enabled_flags.iter().enumerate() {
                let expected = enabled.then_some(VALIDATED);
                assert_plane_state(&case, reported, reader, plane, (expected, None));
            }
        }

        // The validation wrote no configuration: the members a status write does not touch
        // are exactly what the create left.
        assert_eq!(
            configuration(&after),
            configuration(&before),
            "case {case}: completing a validation writes no configuration, got {after}"
        );

        // The status write is one version of the resource, and only when a plane moved:
        // the configuration that enables neither plane has nothing to verify.
        let moved = *einvoice || *purchase_order;
        let expected_version = version(&before) + i64::from(moved);
        assert_eq!(
            version(&after),
            expected_version,
            "case {case}: expected Version {expected_version} after the verify, got {after}"
        );
        if moved {
            assert!(
                last_update_date(&after) >= last_update_date(&before),
                "case {case}: a status the verify moved dates the resource at or after the \
                 last write, got {:?} against {:?}",
                last_update_date(&after),
                last_update_date(&before)
            );
        } else {
            assert_eq!(
                last_update_date(&after),
                last_update_date(&before),
                "case {case}: a validation that moved no plane is no update of the resource, \
                 got {after}"
            );
        }
    }
}

/// RULE: a completed verification carries a plane off the UNVERIFIED status only — a plane
/// whose status an explicit status update already carried elsewhere keeps that status and
/// the reason stated with it, so completing a verification never walks a plane's state
/// backwards.
///
/// The rows are each plane crossed with every status the modelled enum admits, the plane's
/// partner left awaiting verification, so what the verify moves and what it keeps are both
/// asserted on every row.
#[tokio::test]
async fn a_completed_validation_keeps_a_status_moved_past_the_unverified_one() {
    let (router, engine) = emulator_with_engine();

    for (index, (plane, moved_to)) in (0..PLANES.len())
        .flat_map(|plane| STATUS_VALUES.iter().map(move |status| (plane, *status)))
        .enumerate()
    {
        let (label, _, status_member, reason_member) = PLANES[plane];
        let other = 1 - plane;
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_request(
                    portal,
                    &format!("BUYER-MOVED-{index}"),
                    &format!("SUPPLIER-MOVED-{index}"),
                    true,
                    true,
                ),
            )
            .await,
        );
        let case = format!("{label} at {moved_to} on {portal}");
        let reason = format!("{label} was moved to {moved_to} for this case");

        let mut update = json!({"ProcurementPortalPreferenceArn": arn});
        update[status_member] = json!(moved_to);
        update[reason_member] = json!(reason);
        ok(
            &router,
            "UpdateProcurementPortalPreferenceStatus",
            &update,
        )
        .await;
        let before = get_preference(&router, &arn).await;
        let version_before = version(&before);

        complete_validation(&router, &engine, &arn).await;

        // The plane the update moved keeps what the update stated, unless that update left
        // it at the unverified status — in which case this verification is what carries it
        // off it, and the reason stated for the status it left does not follow it.
        let kept = moved_to != PENDING_VERIFICATION;
        let expected = if kept {
            (moved_to, Some(reason.as_str()))
        } else {
            (VALIDATED, None)
        };
        let after = get_preference(&router, &arn).await;
        let summary = summary_of(&router, &arn).await;
        for (reader, reported) in [("read", &after), ("listing", &summary)] {
            assert_plane_state(&case, reported, reader, plane, (Some(expected.0), expected.1));
            // The partner plane was awaiting verification, so this verification moved it.
            assert_plane_state(&case, reported, reader, other, (Some(VALIDATED), None));
        }

        // One write per completed validation that moved a plane, whether it moved one plane
        // or both.
        assert_eq!(
            version(&after),
            version_before + 1,
            "case {case}: expected Version {} after the verify, got {after}",
            version_before + 1
        );

        // A SECOND completed validation finds no plane awaiting verification, so it moves
        // nothing a reader reports.
        complete_validation(&router, &engine, &arn).await;
        let after_again = get_preference(&router, &arn).await;
        assert_eq!(
            after_again, after,
            "case {case}: a validation completed with no plane awaiting verification moves \
             nothing, got {after_again}"
        );
    }
}
