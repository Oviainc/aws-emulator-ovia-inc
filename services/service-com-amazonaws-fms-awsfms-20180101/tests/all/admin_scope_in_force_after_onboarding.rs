// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN an onboarded administrator's `AdminScope` starts
//! and stops governing the resources of the organisation.
//!
//! The rule asserted here: the `AdminScope` `PutAdminAccount` records for an
//! account governs what that account may manage exactly while the account's
//! `OrganizationStatus` is `ONBOARDING_COMPLETE` — the status in which the
//! account "is onboarded to Firewall Manager as an administrator, and can
//! perform actions on the resources defined in their AdminScope"
//! (`AdminAccountSummary$Status`). While the onboarding the call started is
//! still in flight (`ONBOARDING`, "the account is onboarding to Firewall
//! Manager as an administrator") and once the account is on its way out
//! (`OFFBOARDING` / `OFFBOARDING_COMPLETE`, "is being removed" / "has been
//! removed as an Firewall Manager administrator"), the scope it carries governs
//! nothing: the caller administers the organisation as its default
//! administrator, whose scope "allows administration of all policy types,
//! accounts, organizational units, and Regions", so every stored policy is
//! `ACTIVE`, every stored resource set is `ACTIVE`, and the edits and deletes
//! those statuses allow are performed.
//!
//! The class the rule is asserted over is the ADMINISTRATOR LIFECYCLE PHASE,
//! each phase driven the way a client drives it (`PutAdminAccount`,
//! `DisassociateAdminAccount`, and waiting out the span the service's own
//! constant names), crossed with the administrative-scope table the sibling
//! derivation suite uses ([`crate::policy_status_derivation::scope_cases`] —
//! all-enabled and listed forms of the policy-type and Region scopes, and each
//! of them absent), every policy type of
//! [`crate::policy_status_derivation::POLICY_TYPES`], both Regions the suites
//! use, and both kinds of resource whose status an administrative scope decides
//! (a policy and a resource set). Each row's expectation is DERIVED from the
//! rule — the scope's own coverage while it is in force, and "in scope"
//! otherwise — rather than written down.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response.
//!
//! What the two statuses REPORT for an onboarded administrator, and what they
//! let a caller do, are the sibling suites
//! [`crate::policy_status_derivation`], [`crate::resource_set_status_derivation`],
//! [`crate::policy_mutation_admin_scope`] and
//! [`crate::resource_set_mutation_admin_scope`]; this one asserts from which
//! instant their scope table applies at all.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_scopes::{
    admin_scope as read_admin_scope, organization_status, wait_out_the_onboarding,
};
use crate::administrator_offboarding::wait_out_the_offboarding;
use crate::policy_lifecycle::{call, call_in, error_code, ACCOUNT, REGION};
use crate::policy_status_derivation::{
    document, emulator_with_another_default_administrator, expected_status,
    put_admin_scope_without_waiting, scope_cases, ScopeCase, OTHER_REGION, POLICY_TYPES,
};

/// The status of a policy or resource set the governing administrative scope
/// covers: "the administrator can manage and delete" it.
const ACTIVE: &str = "ACTIVE";

/// The status of one it does not cover: "the administrator can view … but they
/// can't edit or delete" it.
const OUT_OF_ADMIN_SCOPE: &str = "OUT_OF_ADMIN_SCOPE";

/// The modelled error a refused edit or delete of a view-only resource answers.
const REFUSAL: &str = "InvalidOperationException";

/// The status the rule says a policy of `security_service_type` created in
/// `region` has, given whether the caller's own recorded scope is IN FORCE.
///
/// In force, the scope's own coverage decides it — the dimension the derivation
/// suite's table states. Not in force, nothing narrows what the caller may
/// manage, so the policy is in the default administrator's full scope.
fn want_policy_status(
    case: &ScopeCase,
    security_service_type: &str,
    region: &str,
    in_force: bool,
) -> &'static str {
    if in_force {
        expected_status(case, security_service_type, region)
    } else {
        ACTIVE
    }
}

/// The same for a resource set, whose status follows the REGION scope alone:
/// `ResourceSet$ResourceSetStatus` "indicates whether the resource set is in or
/// out of an admin's Region scope".
fn want_resource_set_status(case: &ScopeCase, region: &str, in_force: bool) -> &'static str {
    if in_force && !(case.manages_region)(region) {
        OUT_OF_ADMIN_SCOPE
    } else {
        ACTIVE
    }
}

/// A minimal `ResourceSet` document, named for the row so a failure says which
/// one it was. The resource type is one the emulator's own discoverable-resource
/// catalog carries.
fn resource_set_document(name: &str) -> Value {
    json!({
        "Name": name,
        "ResourceTypeList": ["AWS::EC2::VPC"],
    })
}

/// Assert the `OrganizationStatus` `GetAdminScope` reports for the CALLING
/// account, which is the lifecycle phase the rule is keyed to.
async fn assert_organization_status(router: &MultiServiceRouter, phase: &str, want: &str) {
    let (http, code, _, status) = read_admin_scope(router, ACCOUNT).await;
    assert_eq!(
        (http, code.as_str()),
        (200, ""),
        "case {phase:?}: expected GetAdminScope for the calling account to be served with 200, \
         got {http} / {code:?}"
    );
    assert_eq!(
        status.as_str(),
        organization_status(want),
        "case {phase:?}: expected the calling administrator to be {want}, got {status:?}"
    );
}

/// Create one policy of `security_service_type` in `region`, assert the status
/// it is reported with by the write and by the read, and then DELETE it —
/// asserting the delete is performed exactly when that status allows it, and
/// refused with the modelled error otherwise.
async fn assert_policy_phase(
    router: &MultiServiceRouter,
    row: &str,
    security_service_type: &str,
    resource_type: &str,
    region: &str,
    want: &str,
) {
    let (status, body) = call_in(
        router,
        "PutPolicy",
        &json!({ "Policy": document(security_service_type, resource_type, None) }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected PutPolicy to create the policy with 200, got {status} with {body}"
    );
    let stored = body["Policy"].clone();
    let policy_id = stored["PolicyId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {row:?}: no minted PolicyId in {stored}"))
        .to_string();
    assert_eq!(
        stored["PolicyStatus"],
        json!(want),
        "case {row:?}: expected PutPolicy to report PolicyStatus {want}, got {}",
        stored["PolicyStatus"]
    );

    let (status, body) =
        call_in(router, "GetPolicy", &json!({ "PolicyId": policy_id }), region).await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected GetPolicy to answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["Policy"]["PolicyStatus"],
        json!(want),
        "case {row:?}: expected GetPolicy to report PolicyStatus {want}, got {}",
        body["Policy"]["PolicyStatus"]
    );

    let (status, body) = call_in(
        router,
        "DeletePolicy",
        &json!({ "PolicyId": policy_id }),
        region,
    )
    .await;
    if want == OUT_OF_ADMIN_SCOPE {
        assert_eq!(
            status, 400,
            "case {row:?}: expected the delete of a view-only policy to be refused with 400, got \
             {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            REFUSAL,
            "case {row:?}: expected the refusal {REFUSAL}, got {body}"
        );
    } else {
        assert_eq!(
            status, 200,
            "case {row:?}: expected the delete of a policy the governing scope covers to answer \
             200, got {status} with {body}"
        );
    }
}

/// The resource-set reading of [`assert_policy_phase`]: create a resource set in
/// `region`, assert the status the write and the read report, then delete it and
/// assert the delete follows that status.
async fn assert_resource_set_phase(
    router: &MultiServiceRouter,
    row: &str,
    region: &str,
    want: &str,
) {
    let (status, body) = call_in(
        router,
        "PutResourceSet",
        &json!({ "ResourceSet": resource_set_document("in-force") }),
        region,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected PutResourceSet to create the resource set with 200, got {status} \
         with {body}"
    );
    let stored = body["ResourceSet"].clone();
    let id = stored["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {row:?}: no minted Id in {stored}"))
        .to_string();
    assert_eq!(
        stored["ResourceSetStatus"],
        json!(want),
        "case {row:?}: expected PutResourceSet to report ResourceSetStatus {want}, got {}",
        stored["ResourceSetStatus"]
    );

    let (status, body) =
        call_in(router, "GetResourceSet", &json!({ "Identifier": id }), region).await;
    assert_eq!(
        status, 200,
        "case {row:?}: expected GetResourceSet to answer 200, got {status} with {body}"
    );
    assert_eq!(
        body["ResourceSet"]["ResourceSetStatus"],
        json!(want),
        "case {row:?}: expected GetResourceSet to report ResourceSetStatus {want}, got {}",
        body["ResourceSet"]["ResourceSetStatus"]
    );

    let (status, body) = call_in(
        router,
        "DeleteResourceSet",
        &json!({ "Identifier": id }),
        region,
    )
    .await;
    if want == OUT_OF_ADMIN_SCOPE {
        assert_eq!(
            status, 400,
            "case {row:?}: expected the delete of a view-only resource set to be refused with \
             400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            REFUSAL,
            "case {row:?}: expected the refusal {REFUSAL}, got {body}"
        );
    } else {
        assert_eq!(
            status, 200,
            "case {row:?}: expected the delete of a resource set the governing scope covers to \
             answer 200, got {status} with {body}"
        );
    }
}

/// Drive every row of the resource table through one lifecycle phase: both
/// kinds of resource, every policy type and both Regions, with the expectation
/// derived from whether the caller's recorded scope is in force in this phase.
async fn assert_phase(
    router: &MultiServiceRouter,
    phase: &str,
    case: &ScopeCase,
    in_force: bool,
) {
    for (security_service_type, resource_type) in POLICY_TYPES {
        for region in [REGION, OTHER_REGION] {
            let row = format!("{phase}/{}/{security_service_type}/{region}", case.name);
            let want = want_policy_status(case, security_service_type, region, in_force);
            assert_policy_phase(
                router,
                &row,
                security_service_type,
                resource_type,
                region,
                want,
            )
            .await;
        }
    }
    for region in [REGION, OTHER_REGION] {
        let row = format!("{phase}/{}/resource-set/{region}", case.name);
        let want = want_resource_set_status(case, region, in_force);
        assert_resource_set_phase(router, &row, region, want).await;
    }
}

/// An administrator whose onboarding is still in flight governs nothing: in the
/// window between `PutAdminAccount` and the `ONBOARDING_COMPLETE` the clock
/// makes due, every policy and every resource set is `ACTIVE` and the deletes
/// are performed — and once that onboarding completes, the very same recorded
/// scope decides both.
#[tokio::test]
async fn an_onboarding_administrators_scope_governs_nothing_until_the_clock_completes_it() {
    for case in scope_cases() {
        let router = emulator_with_another_default_administrator().await;
        put_admin_scope_without_waiting(&router, case.name, &case.scope).await;

        // The onboarding span is never zero, so the scope recorded by the call
        // just made is not in force yet. The phase is asserted on both sides of
        // the reads, so a window that had elapsed during them is a failure of
        // this guard rather than a silently different phase.
        assert_organization_status(&router, "an onboarding in flight", "ONBOARDING").await;
        assert_phase(&router, "an onboarding in flight", &case, false).await;
        assert_organization_status(&router, "an onboarding still in flight", "ONBOARDING").await;

        // The clock completes the onboarding, and from that instant the account
        // "can perform actions on the resources defined in their AdminScope":
        // the scope governs.
        wait_out_the_onboarding();
        assert_organization_status(
            &router,
            "the onboarding the clock completed",
            "ONBOARDING_COMPLETE",
        )
        .await;
        assert_phase(&router, "the onboarding the clock completed", &case, true).await;
    }
}

/// An administrator on its way out stops governing: from the instant
/// `DisassociateAdminAccount` starts the removal, and once the clock has
/// completed it, the scope the record still carries decides nothing — the
/// organisation's default administrator governs, so every policy and every
/// resource set is `ACTIVE` again and the deletes are performed.
#[tokio::test]
async fn the_scope_of_an_administrator_being_removed_stops_governing() {
    for case in scope_cases() {
        let router = emulator_with_another_default_administrator().await;
        put_admin_scope_without_waiting(&router, case.name, &case.scope).await;
        wait_out_the_onboarding();
        assert_organization_status(&router, "onboarded", "ONBOARDING_COMPLETE").await;
        assert_phase(&router, "onboarded", &case, true).await;

        // The caller asks to be removed as an administrator. It holds a
        // DELEGATED role, so this removes that role and leaves the
        // organisation's default administrator — another account — in place.
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {}: expected the delegated administrator's removal to be accepted, got {status} \
             with {body}",
            case.name
        );

        assert_organization_status(&router, "a removal in flight", "OFFBOARDING").await;
        assert_phase(&router, "a removal in flight", &case, false).await;
        assert_organization_status(&router, "a removal still in flight", "OFFBOARDING").await;

        wait_out_the_offboarding();
        assert_organization_status(
            &router,
            "the removal the clock completed",
            "OFFBOARDING_COMPLETE",
        )
        .await;
        assert_phase(&router, "the removal the clock completed", &case, false).await;
    }
}
