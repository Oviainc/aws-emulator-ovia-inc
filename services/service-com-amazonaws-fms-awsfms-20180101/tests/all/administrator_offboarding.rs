// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how a Firewall Manager administrator LEAVES the
//! organization.
//!
//! The rule asserted here: `DisassociateAdminAccount` carries no member, so the
//! administrator role it removes is the one the CALLING account holds — the
//! delegated administrator role `PutAdminAccount` onboarded for that account
//! when it has one, and otherwise the organization's default-administrator
//! association. A delegated administrator leaving goes `OFFBOARDING` while the
//! removal is under way and `OFFBOARDING_COMPLETE` once the clock has made it
//! due ("the account has been removed as an Firewall Manager administrator",
//! `AdminAccountSummary$Status`), after which that account is an administrator
//! of the organization no longer: it manages no account, it holds no role left
//! to remove, and `PutAdminAccount` onboards it again from the start. Removing
//! one administrator's role touches no other administrator — not the default
//! administrator's association and not another onboarded account's role.
//!
//! The class each rule is asserted over is the ADMINISTRATOR ROLE the caller
//! holds: the rows differ in the `AdminScope` the leaving administrator was
//! granted (every scope absent, all accounts, a listed account, an excluded
//! account, a policy-type-only scope), in whether the onboarding the clock owed
//! it had completed before it asked to leave, in the account id of the default
//! administrator it leaves behind (account ids differing in length and digits
//! within the model's `AWSAccountId` constraint) and in whether another
//! onboarded administrator is there to be left untouched.
//!
//! Every test drives the service the way a client does, through the shared
//! awsJson1_1 harness in [`crate::policy_lifecycle`], so the assertions are
//! about the WIRE response.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_scopes::{
    admin_scope, full_admin_scope, normalize_scope, organization_status, put_admin_account,
    wait_out_the_onboarding,
};
use crate::admin_accounts_for_organization::list_admins;
use crate::policy_lifecycle::{call, emulator, error_code, ACCOUNT, REGION};

/// How long an administrator stays `OFFBOARDING`, from the service's own
/// constant, plus a margin so a waiting test is not racing it.
///
/// A blocking `std::thread::sleep` on purpose, for the reason
/// [`crate::admin_account_scopes::wait_out_the_onboarding`] states: this
/// workspace's `tokio` is built without the `time` feature, so
/// `tokio::time::sleep` is not available here.
fn wait_out_the_offboarding() {
    sleep(OFFBOARDING_SPAN + Duration::from_millis(250));
}

/// How long an administrator stays `OFFBOARDING`, from the service's own
/// constant — the span a removal's due instant is set from.
const OFFBOARDING_SPAN: Duration =
    service_com_amazonaws_fms_awsfms_20180101::state::ADMIN_OFFBOARDING_SPAN;

/// A fraction of the removal span, after which a repeat of the request is still
/// a repeat of one under way.
fn part_way_in() -> Duration {
    OFFBOARDING_SPAN * 2 / 5
}

/// The rest of that span plus a margin, so a removal the first call made due is
/// complete — while one a repeat had restarted ([`part_way_in`] later) would
/// not be.
fn rest_of_the_span() -> Duration {
    OFFBOARDING_SPAN * 3 / 5 + Duration::from_millis(150)
}

/// One row of the table: a name for the assertion messages, the account the
/// organization's DEFAULT administrator association names, the `AdminScope` the
/// leaving administrator was onboarded with (`None` sends no `AdminScope` at
/// all), whether the test waits out the onboarding before asking it to leave,
/// and another onboarded administrator that must be left untouched.
struct Case {
    name: &'static str,
    default_admin: &'static str,
    scope: Option<Value>,
    onboarded: bool,
    co_administrator: Option<&'static str>,
}

/// The administrator roles the rule is asserted over.
///
/// The default-administrator account ids are `AWSAccountId`s (the model
/// constrains the shape only to `^[0-9]+$` with length 1..1024), so they differ
/// in length and digits — including the one-digit and the long forms, which the
/// issue did not cite — and none of them is the calling account, which is the
/// configuration in which the caller holds a DELEGATED administrator role.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "full scope, onboarding complete, one co-administrator",
            default_admin: "111111111111",
            scope: Some(full_admin_scope()),
            onboarded: true,
            co_administrator: Some("222222222222"),
        },
        Case {
            name: "the caller's own account listed, onboarding still in flight",
            default_admin: "9",
            scope: Some(json!({ "AccountScope": { "Accounts": [ACCOUNT] } })),
            onboarded: false,
            co_administrator: None,
        },
        Case {
            name: "no scope at all",
            default_admin: "99999999999999999999",
            scope: None,
            onboarded: true,
            co_administrator: None,
        },
        Case {
            name: "a scope that excludes the caller",
            default_admin: "000000000001",
            scope: Some(json!({
                "AccountScope": {
                    "Accounts": [ACCOUNT],
                    "ExcludeSpecifiedAccounts": true,
                },
                "RegionScope": { "Regions": ["eu-central-1"] },
            })),
            onboarded: true,
            co_administrator: Some("7"),
        },
        Case {
            name: "a policy-type-only scope, onboarding still in flight",
            default_admin: "123456789012",
            scope: Some(json!({ "PolicyTypeScope": { "AllPolicyTypesEnabled": true } })),
            onboarded: false,
            co_administrator: Some("8"),
        },
    ]
}

/// Make `account` the organization's default administrator.
async fn associate(router: &MultiServiceRouter, case: &str, account: &str) {
    let (status, body) = call(
        router,
        "AssociateAdminAccount",
        &json!({ "AdminAccount": account }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AssociateAdminAccount({account}) must answer 200, got {status} with {body}"
    );
}

/// The administrator accounts `ListAdminsManagingAccount` reports for the
/// calling account, in the order it reports them.
async fn managing_administrators(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, "ListAdminsManagingAccount", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAdminsManagingAccount must answer 200, got {status} with {body}"
    );
    body["AdminAccounts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|account| account.as_str().unwrap_or_default().to_string())
        .collect()
}

/// `GetAdminAccount`'s `(http status, parsed body)` — the default-administrator
/// association, which a delegated administrator's departure must not touch.
async fn admin_account(router: &MultiServiceRouter) -> (u16, Value) {
    call(router, "GetAdminAccount", &json!({})).await
}

/// The administrators the organization-wide enumeration reports, as
/// `(account, is default, OrganizationStatus)`.
async fn enumerated(router: &MultiServiceRouter, case: &str) -> Vec<(String, bool, String)> {
    let (status, code, reported, _) = list_admins(router, &json!({}), REGION).await;
    assert_eq!(
        (status, code.as_str()),
        (200, ""),
        "case {case:?}: ListAdminAccountsForOrganization must answer 200, got {status} / {code:?}"
    );
    reported
}

/// The administrator role a `DisassociateAdminAccount` removes is the CALLER's
/// own: an account `PutAdminAccount` onboarded leaves on its own call, goes
/// `OFFBOARDING` for a non-zero span, is `OFFBOARDING_COMPLETE` once the clock
/// has made the removal due, and is then an administrator no longer — while
/// every other administrator of the organization, the default one included, is
/// left exactly as it was.
#[tokio::test]
async fn a_delegated_administrator_leaves_on_its_own_call() {
    for case in cases() {
        let name = case.name;
        let router = emulator();
        associate(&router, name, case.default_admin).await;
        put_admin_account(&router, name, ACCOUNT, case.scope.as_ref()).await;
        if let Some(co) = case.co_administrator {
            put_admin_account(&router, name, co, Some(&full_admin_scope())).await;
        }
        if case.onboarded {
            wait_out_the_onboarding();
        }

        // Before: the caller holds a delegated administrator role, with the
        // scope it was granted and the onboarding status the clock has reached.
        let (http, code, granted, status_before) = admin_scope(&router, ACCOUNT).await;
        let expected_before = if case.onboarded {
            organization_status("ONBOARDING_COMPLETE")
        } else {
            organization_status("ONBOARDING")
        };
        assert_eq!(
            (http, code.as_str(), status_before.as_str()),
            (200, "", expected_before),
            "case {name:?}: before it leaves, the caller must hold an administrator role, \
             got {http} / {code:?} / {status_before:?}"
        );

        // The caller asks to leave, and the call is its own to make: nothing has
        // to leave before a delegated administrator may.
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the caller's own disassociation must be accepted, got {status} \
             with {body}"
        );

        // Asking again PART WAY THROUGH the removal is idempotent and does not
        // push the due instant out: the repeat is the FIRST call after the one
        // that started the removal, a fraction of the span in, and the
        // completion asserted below is read before a span starting afresh at the
        // repeat could have elapsed — so a due instant the repeat had reset
        // would still report the removal as under way there.
        sleep(part_way_in());
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: a repeated disassociation must be idempotent, got {status} with {body}"
        );

        // The removal is not instantaneous: the span is never zero, so a caller
        // reading while it is under way sees the transient status, and the scope
        // it was granted is untouched.
        let (http, code, scope_now, status_now) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            (http, code.as_str(), status_now.as_str()),
            (200, "", organization_status("OFFBOARDING")),
            "case {name:?}: while the removal is under way the administrator must be reported \
             as being removed, got {http} / {code:?} / {status_now:?}"
        );
        assert_eq!(
            scope_now, granted,
            "case {name:?}: leaving must not rewrite the scope the administrator was granted, \
             got {scope_now:?}"
        );

        // The enumeration reports the same phase, and the default
        // administrator's association is exactly as it was.
        let reported = enumerated(&router, name).await;
        assert!(
            reported.contains(&(
                ACCOUNT.to_string(),
                false,
                organization_status("OFFBOARDING").to_string()
            )),
            "case {name:?}: the enumeration must report the administrator being removed, \
             got {reported:?}"
        );
        assert!(
            reported.contains(&(
                case.default_admin.to_string(),
                true,
                organization_status("ONBOARDING_COMPLETE").to_string()
            )),
            "case {name:?}: the default administrator must be left onboarded, got {reported:?}"
        );
        let (status, got) = admin_account(&router).await;
        assert_eq!(
            (
                status,
                got["AdminAccount"].as_str(),
                got["RoleStatus"].as_str()
            ),
            (200, Some(case.default_admin), Some("READY")),
            "case {name:?}: a delegated administrator leaving must not touch the default \
             administrator's association, got {status} {got}"
        );

        // The clock completes the removal — the rest of the span the first call
        // started, which is less than a whole span from the repeat.
        sleep(rest_of_the_span());
        let (http, code, scope_after, status_after) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            (http, code.as_str(), status_after.as_str()),
            (200, "", organization_status("OFFBOARDING_COMPLETE")),
            "case {name:?}: once the span has passed the account must have been removed as an \
             administrator, got {http} / {code:?} / {status_after:?}"
        );
        assert_eq!(
            scope_after, granted,
            "case {name:?}: the completed removal must leave the granted scope readable, \
             got {scope_after:?}"
        );

        // Removed means removed: it manages no account any more, and it holds no
        // administrator role left to remove.
        let managing = managing_administrators(&router, name).await;
        assert!(
            !managing.contains(&ACCOUNT.to_string()),
            "case {name:?}: a removed administrator must manage no account, got {managing:?}"
        );
        assert!(
            managing.contains(&case.default_admin.to_string()),
            "case {name:?}: the default administrator manages every account of the organization, \
             got {managing:?}"
        );
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {name:?}: a caller that holds no administrator role must be refused, \
             got {status} {body}"
        );

        // Another onboarded administrator is left exactly as it was — its own
        // onboarding completed by the clock, not by this caller leaving.
        if let Some(co) = case.co_administrator {
            let (http, code, _, co_status) = admin_scope(&router, co).await;
            assert_eq!(
                (http, code.as_str(), co_status.as_str()),
                (200, "", organization_status("ONBOARDING_COMPLETE")),
                "case {name:?}: co-administrator {co} must be untouched, got {http} / {code:?} \
                 / {co_status:?}"
            );
        }
    }
}

/// An account that HAS BEEN REMOVED as an administrator is onboarded again by
/// `PutAdminAccount`: it is no administrator for that call to update, so it is
/// created — `ONBOARDING` from the start, with the scope that call sends and
/// nothing carried over from the role it used to hold — and the whole lifecycle
/// can be walked again.
#[tokio::test]
async fn a_removed_administrator_is_onboarded_again_from_the_start() {
    // Each row: the scope the re-onboarding call sends, and the scope the
    // administrator then holds — an absent `AdminScope` grants nothing, exactly
    // as it does on a first-time create.
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("the scope the new call sends", Some(full_admin_scope())),
        (
            "a narrower scope than before",
            Some(json!({ "AccountScope": { "AllAccountsEnabled": true } })),
        ),
        ("no scope at all", None),
    ];

    for (name, again) in rows {
        let router = emulator();
        associate(&router, name, "111111111111").await;
        // The role it held before: a scope no row re-sends, so a response that
        // carried it over would not match.
        let first = json!({
            "AccountScope": { "Accounts": [ACCOUNT] },
            "RegionScope": { "Regions": ["eu-central-1"] },
        });
        put_admin_account(&router, name, ACCOUNT, Some(&first)).await;
        wait_out_the_onboarding();

        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the caller's own disassociation must be accepted, got {status} \
             with {body}"
        );
        wait_out_the_offboarding();
        let (_, _, _, removed) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            removed.as_str(),
            organization_status("OFFBOARDING_COMPLETE"),
            "case {name:?}: the account must have been removed as an administrator, \
             got {removed:?}"
        );

        put_admin_account(&router, name, ACCOUNT, again.as_ref()).await;
        let (http, code, scope, status_now) = admin_scope(&router, ACCOUNT).await;
        let expected_scope = normalize_scope(&again.clone().unwrap_or_else(|| json!({})));
        assert_eq!(
            (http, code.as_str(), status_now.as_str()),
            (200, "", organization_status("ONBOARDING")),
            "case {name:?}: a removed account must onboard again from the start, got {http} / \
             {code:?} / {status_now:?}"
        );
        assert_eq!(
            scope, expected_scope,
            "case {name:?}: the re-onboarded administrator must hold the scope this call sent, \
             got {scope:?}"
        );

        // The onboarding the clock owes it completes as any other does, and the
        // role can be removed again: the lifecycle has an exit and an entrance.
        wait_out_the_onboarding();
        let (_, _, _, onboarded) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            onboarded.as_str(),
            organization_status("ONBOARDING_COMPLETE"),
            "case {name:?}: the re-onboarding must complete, got {onboarded:?}"
        );
        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the re-onboarded administrator must be able to leave again, \
             got {status} with {body}"
        );
        let (_, _, _, leaving) = admin_scope(&router, ACCOUNT).await;
        assert_eq!(
            leaving.as_str(),
            organization_status("OFFBOARDING"),
            "case {name:?}: the second departure must be reported too, got {leaving:?}"
        );
    }
}
