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
//! * a status a caller already carried past the unverified one is KEPT, with its reason, by
//!   a later completed validation — over the statuses a caller can carry a plane to and
//!   both planes;
//! * each call of the flow is one version of the resource: the send that issues the code
//!   and the verify that completes the validation each write it once, whether or not a
//!   plane moved.

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::refused_procurement_portal_writes::{
    accepted_update_round, arn_of, emulator_with_engine, get_preference, issue_code, ok,
    without_write_markers, PORTAL_INSTANCE_ENDPOINT, PORTAL_NAMES,
};

/// The status an enabled plane holds while nothing has verified its portal connection, and
/// the status it reports once something has: two of the values
/// `ProcurementPortalPreferenceStatus` admits (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")`).
const PENDING_VERIFICATION: &str = "PENDING_VERIFICATION";
const VALIDATED: &str = "VALIDATED";

/// The statuses a CALLER can carry a plane to, in the order the lifecycle reaches them:
/// the validated status a completed validation leaves, then the connection test a caller
/// asks for and the activation that follows it. The rest of the enum — the unverified
/// status, the two test failures and the suspension — is what the service assigns itself,
/// so no status update can put a plane there (`smithy_query(query="shape",
/// shape="ProcurementPortalPreferenceStatus")` lists all seven).
const CALLER_REACHABLE_STATUSES: &[&str] = &[VALIDATED, "TEST_INITIALIZED", "ACTIVE"];

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

/// A create request for one identity, stating the two enable flags under test — and the
/// endpoint the test e-invoice of a portal validation is delivered to, so the send that
/// issues a code can confirm that delivery.
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
        "ProcurementPortalInstanceEndpoint": PORTAL_INSTANCE_ENDPOINT,
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

        // The status write is one version of the resource, and so is the code the send
        // issued: two calls of the flow, two writes, whether or not a plane moved.
        let expected_version = version(&before) + 2;
        assert_eq!(
            version(&after),
            expected_version,
            "case {case}: expected Version {expected_version} after the verify, got {after}"
        );
        assert!(
            last_update_date(&after) >= last_update_date(&before),
            "case {case}: the calls of the flow date the resource at or after the last \
             write, got {:?} against {:?}",
            last_update_date(&after),
            last_update_date(&before)
        );
    }
}

/// RULE: a completed verification carries a plane off the UNVERIFIED status only — a plane
/// a caller already carried further along the lifecycle keeps that status and the reason
/// stated with it, so completing a verification never walks a plane's state backwards.
///
/// The rows are each plane crossed with every status a CALLER can carry it to
/// ([`CALLER_REACHABLE_STATUSES`]), its partner plane left disabled at the create and
/// enabled again afterwards so that it IS awaiting verification when the second validation
/// completes — what the verify moves and what it keeps are therefore both asserted on every
/// row.
#[tokio::test]
async fn a_completed_validation_keeps_a_status_moved_past_the_unverified_one() {
    let (router, engine) = emulator_with_engine();

    for (index, (plane, moved_to)) in (0..PLANES.len())
        .flat_map(|plane| {
            CALLER_REACHABLE_STATUSES
                .iter()
                .map(move |status| (plane, *status))
        })
        .enumerate()
    {
        let (label, _, status_member, reason_member) = PLANES[plane];
        let other = 1 - plane;
        let portal = PORTAL_NAMES[index % PORTAL_NAMES.len()];
        // Only the row's plane is enabled to start with, so the first validation moves it
        // alone and its partner has a status only once a configuration write enables it.
        let arn = arn_of(
            &ok(
                &router,
                "CreateProcurementPortalPreference",
                &create_request(
                    portal,
                    &format!("BUYER-MOVED-{index}"),
                    &format!("SUPPLIER-MOVED-{index}"),
                    plane == 0,
                    plane == 1,
                ),
            )
            .await,
        );
        let case = format!("{label} at {moved_to} on {portal}");
        let reason = format!("{label} was moved to {moved_to} for this case");

        // The validated status is where a completed validation leaves the plane; the
        // statuses past it are the moves a caller asks for, one update each.
        complete_validation(&router, &engine, &arn).await;
        let target = CALLER_REACHABLE_STATUSES
            .iter()
            .position(|status| *status == moved_to)
            .unwrap_or_else(|| panic!("case {case}: the row's status has to be reachable"));
        let walk = &CALLER_REACHABLE_STATUSES[1..target + 1];
        for status in walk {
            let mut update = json!({"ProcurementPortalPreferenceArn": arn});
            update[status_member] = json!(status);
            if *status == moved_to {
                update[reason_member] = json!(reason);
            }
            ok(&router, "UpdateProcurementPortalPreferenceStatus", &update).await;
        }
        // The reason a reader reports is the one stated with the update that set the
        // current status, so a plane the validation alone carried reports none.
        let expected_reason = (!walk.is_empty()).then_some(reason.as_str());

        // Enabling the partner plane gives it the unverified status, which is what the
        // next completed validation has to move while the row's plane keeps what it holds.
        ok(
            &router,
            "PutProcurementPortalPreference",
            &accepted_update_round(&arn, index as i64),
        )
        .await;
        let before = get_preference(&router, &arn).await;
        assert_plane_state(
            &case,
            &before,
            "read before the second verify",
            other,
            (Some(PENDING_VERIFICATION), None),
        );
        let version_before = version(&before);

        complete_validation(&router, &engine, &arn).await;

        let after = get_preference(&router, &arn).await;
        let summary = summary_of(&router, &arn).await;
        for (reader, reported) in [("read", &after), ("listing", &summary)] {
            assert_plane_state(&case, reported, reader, plane, (Some(moved_to), expected_reason));
            // The partner plane was awaiting verification, so this verification moved it.
            assert_plane_state(&case, reported, reader, other, (Some(VALIDATED), None));
        }

        // Two calls of the flow, two writes of the resource.
        assert_eq!(
            version(&after),
            version_before + 2,
            "case {case}: expected Version {} after the second verify, got {after}",
            version_before + 2
        );

        // A THIRD completed validation finds no plane awaiting verification, so it moves
        // nothing a reader reports beyond the two members its own writes advance.
        complete_validation(&router, &engine, &arn).await;
        let after_again = get_preference(&router, &arn).await;
        assert_eq!(
            without_write_markers(&after_again),
            without_write_markers(&after),
            "case {case}: a validation completed with no plane awaiting verification moves \
             nothing, got {after_again}"
        );
    }
}
