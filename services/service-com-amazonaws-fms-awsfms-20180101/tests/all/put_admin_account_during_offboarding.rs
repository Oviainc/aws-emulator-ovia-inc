// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutAdminAccount` on an account whose administrator
//! role is GONE OR GOING.
//!
//! The rule asserted here: a writer that designates a resource makes the status
//! it starts the one the clock then completes — `AdminAccountSummary$Status` is
//! "the current status of the request to onboard a member account as an Firewall
//! Manager administrator", so after an accepted `PutAdminAccount` it is THIS
//! request's status — which means the designation SUPERSEDES whatever
//! transition the clock still owed that account, and no pending transition can
//! later undo what the accepted call created. So an account that is being
//! removed as an administrator, or has been removed as one, holds no
//! administrator role for the call to update: it is created again,
//! `ONBOARDING` from the start with the scope that call sends and nothing of the
//! role it used to hold carried over, and the removal that was still in flight
//! never completes.
//!
//! The class the rule is asserted over is HOW FAR the removal had got when the
//! account was designated again — not started settling yet, part way through the
//! span, and completed — crossed with the scope the designating call sends
//! (full, narrower, absent), with whether the first onboarding had completed
//! before the account left, and with the default-administrator account id left
//! beside it (ids differing in length and digits within the model's
//! `AWSAccountId` constraint).
//!
//! Every test drives the service the way a client does, through the shared
//! awsJson1_1 harness in [`crate::policy_lifecycle`], so the assertions are
//! about the WIRE response.

use std::thread::sleep;

use serde_json::{json, Value};

use crate::admin_account_scopes::{
    admin_scope, full_admin_scope, normalize_scope, organization_status, put_admin_account,
    wait_out_the_onboarding,
};
use crate::admin_accounts_for_organization::list_admins;
use crate::administrator_offboarding::{
    associate, managing_administrators, part_way_in, wait_out_the_offboarding,
};
use crate::policy_lifecycle::{call, emulator, ACCOUNT, REGION};

/// How far the removal of the administrator role had got when the account was
/// designated again.
#[derive(Debug, Clone, Copy)]
enum Stage {
    /// Straight after the call that asked for the removal: the span is never
    /// zero, so the removal is under way with its completion still owed.
    JustStarted,
    /// A fraction of the removal span in: still under way, still owed.
    PartWayIn,
    /// After the clock has completed the removal: the account has been removed
    /// as an administrator.
    Completed,
}

impl Stage {
    /// Wait until this stage of the removal has been reached, and answer the
    /// `OrganizationStatus` the account must be reported with there.
    fn reach(self) -> &'static str {
        match self {
            Stage::JustStarted => organization_status("OFFBOARDING"),
            Stage::PartWayIn => {
                sleep(part_way_in());
                organization_status("OFFBOARDING")
            }
            Stage::Completed => {
                wait_out_the_offboarding();
                organization_status("OFFBOARDING_COMPLETE")
            }
        }
    }
}

/// One row of the table: a name for the assertion messages, the account holding
/// the organization's DEFAULT administrator association (never the caller, so
/// the caller holds a delegated role of its own), the scope the caller was
/// onboarded with, whether that onboarding had completed before it asked to
/// leave, how far the removal had got when the designating call came in, the
/// scope that call sends (`None` sends no `AdminScope` at all) and whether that
/// scope covers the calling account.
struct Case {
    name: &'static str,
    default_admin: &'static str,
    first: Option<Value>,
    onboarded: bool,
    stage: Stage,
    again: Option<Value>,
    manages_caller: bool,
}

/// The removals the rule is asserted over.
///
/// The default-administrator account ids are `AWSAccountId`s (the model
/// constrains the shape only to `^[0-9]+$` with length 1..1024), so they differ
/// in length and digits — including the one-digit and the long forms — and none
/// of them is the calling account.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "removal just started, designated again with the full scope",
            default_admin: "111111111111",
            first: Some(json!({ "AccountScope": { "Accounts": [ACCOUNT] } })),
            onboarded: true,
            stage: Stage::JustStarted,
            again: Some(full_admin_scope()),
            manages_caller: true,
        },
        Case {
            name: "removal part way in, designated again with a Region-only scope",
            default_admin: "9",
            first: Some(full_admin_scope()),
            onboarded: true,
            stage: Stage::PartWayIn,
            again: Some(json!({ "RegionScope": { "Regions": ["eu-central-1"] } })),
            manages_caller: false,
        },
        Case {
            name: "removal just started on an onboarding that had not completed",
            default_admin: "99999999999999999999",
            first: None,
            onboarded: false,
            stage: Stage::JustStarted,
            again: Some(json!({ "AccountScope": { "AllAccountsEnabled": true } })),
            manages_caller: true,
        },
        Case {
            name: "removal part way in, designated again with no scope at all",
            default_admin: "000000000001",
            first: Some(full_admin_scope()),
            onboarded: false,
            stage: Stage::PartWayIn,
            again: None,
            manages_caller: false,
        },
        Case {
            name: "removal completed, designated again with a listed account",
            default_admin: "7",
            first: Some(json!({ "RegionScope": { "Regions": [REGION] } })),
            onboarded: true,
            stage: Stage::Completed,
            again: Some(json!({ "AccountScope": { "Accounts": [ACCOUNT] } })),
            manages_caller: true,
        },
        Case {
            name: "removal completed, designated again with no scope at all",
            default_admin: "123456789012",
            first: Some(full_admin_scope()),
            onboarded: true,
            stage: Stage::Completed,
            again: None,
            manages_caller: false,
        },
    ]
}

/// An account whose administrator role is gone or going is designated again by
/// `PutAdminAccount`: the call starts the onboarding it reports, superseding the
/// removal the clock still owed, so the account really is an administrator
/// afterwards — and the removal that was in flight never completes behind the
/// accepted call's back.
#[tokio::test]
async fn an_administrator_on_its_way_out_is_designated_again() {
    for case in cases() {
        let name = case.name;
        let router = emulator();
        associate(&router, name, case.default_admin).await;
        put_admin_account(&router, name, ACCOUNT, case.first.as_ref()).await;
        if case.onboarded {
            wait_out_the_onboarding();
        }

        // The caller asks for its own delegated role to be removed, and the
        // test waits until the removal has reached this row's stage.
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the caller's own disassociation must be accepted, got {status} \
             with {body}"
        );
        let leaving = case.stage.reach();
        let (http, code, _, status_before) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            (http, code.as_str(), status_before.as_str()),
            (200, "", leaving),
            "case {name:?}: before it is designated again the account must be reported as \
             leaving, got {http} / {code:?} / {status_before:?}"
        );

        // The designating call: accepted, and what it reports is its OWN
        // onboarding — `ONBOARDING`, with the scope it sent and nothing of the
        // role the account used to hold.
        put_admin_account(&router, name, ACCOUNT, case.again.as_ref()).await;
        let expected_scope = normalize_scope(&case.again.clone().unwrap_or_else(|| json!({})));
        let (http, code, scope, status_now) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            (http, code.as_str(), status_now.as_str()),
            (200, "", organization_status("ONBOARDING")),
            "case {name:?}: the designated account must onboard from the start, got {http} / \
             {code:?} / {status_now:?}"
        );
        assert_eq!(
            scope, expected_scope,
            "case {name:?}: the designated administrator must hold the scope this call sent, \
             got {scope:?}"
        );
        let (status, code, reported, _) = list_admins(&router, &json!({}), REGION).await;
        assert_eq!(
            (status, code.as_str()),
            (200, ""),
            "case {name:?}: ListAdminAccountsForOrganization must answer 200, got {status} / \
             {code:?}"
        );
        assert!(
            reported.contains(&(
                ACCOUNT.to_string(),
                false,
                organization_status("ONBOARDING").to_string()
            )),
            "case {name:?}: the enumeration must report the designated administrator as \
             onboarding, got {reported:?}"
        );

        // The clock completes the onboarding THIS call started — the removal it
        // superseded is gone, so no transition the clock owed earlier can undo
        // the designation.
        wait_out_the_onboarding();
        let (http, code, scope_after, status_after) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            (http, code.as_str(), status_after.as_str()),
            (200, "", organization_status("ONBOARDING_COMPLETE")),
            "case {name:?}: the onboarding the designating call started must be the one the \
             clock completes, got {http} / {code:?} / {status_after:?}"
        );
        assert_eq!(
            scope_after, expected_scope,
            "case {name:?}: completing the onboarding must leave the scope the call sent, \
             got {scope_after:?}"
        );

        // Onboarded means onboarded: the account manages exactly the accounts
        // its new scope covers, and it holds a role that can be removed again.
        let managing = managing_administrators(&router, name).await;
        assert_eq!(
            managing.contains(&ACCOUNT.to_string()),
            case.manages_caller,
            "case {name:?}: the designated administrator must manage the calling account \
             exactly when its scope covers it (expected {expected}), got {managing:?}",
            expected = case.manages_caller
        );
        assert!(
            managing.contains(&case.default_admin.to_string()),
            "case {name:?}: the default administrator manages every account of the \
             organization, got {managing:?}"
        );
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the designated administrator must be able to leave again, \
             got {status} with {body}"
        );
        let (_, _, _, leaving_again) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            leaving_again.as_str(),
            organization_status("OFFBOARDING"),
            "case {name:?}: the second departure must be reported too, got {leaving_again:?}"
        );
    }
}
