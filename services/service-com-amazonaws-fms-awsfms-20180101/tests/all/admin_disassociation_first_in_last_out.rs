// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the first-in-last-out precondition on disassociating
//! the Firewall Manager DEFAULT administrator account.
//!
//! The rule asserted here: while the organization still holds a Firewall
//! Manager administrator other than its default one, the default
//! administrator's own disassociation is not possible and is refused with the
//! modelled `InvalidOperationException`, leaving every administrator record it
//! holds exactly as it was; the tear-down is accepted only once the
//! organization holds no other administrator. The precondition is read at the
//! call that STARTS the tear-down, so a repeat of one already recorded stays
//! idempotent.
//!
//! The class the rule is asserted over is the SET of other administrators the
//! organization holds: the rows differ in COUNT (none, one, several), in
//! account-id length and digits, in the `AdminScope` each administrator was
//! given (full, narrowed to foreign accounts, none at all) and in whether the
//! onboarding the clock owes has completed — none of which may change the
//! answer, since what the rule counts is only that an administrator is there.
//!
//! Every test drives the service the way a client does, through the shared
//! awsJson1_1 harness in [`crate::policy_lifecycle`], so the assertions are
//! about the WIRE response.

use serde_json::{json, Value};

use crate::admin_account_disassociation::wait_out_the_disassociation;
use crate::admin_account_scopes::{organization_status, wait_out_the_onboarding};
use crate::administrator_gated_operations::{associate_admin, error_message};
use crate::policy_lifecycle::{call, emulator, error_code};

/// The model's own `AccountRoleStatus` catalog
/// (`com.amazonaws.fms#AccountRoleStatus`), the source of every role status
/// asserted below.
const ROLE_STATUSES: &[&str] =
    service_com_amazonaws_fms_awsfms_20180101::ops::types::ACCOUNT_ROLE_STATUS_VALUES;

/// `name` as the model's catalog spells it, so an expectation names a value
/// `AccountRoleStatus` actually carries instead of a free-standing string.
fn role_status(name: &str) -> &'static str {
    ROLE_STATUSES
        .iter()
        .copied()
        .find(|value| *value == name)
        .unwrap_or_else(|| {
            panic!("{name} must be a modelled AccountRoleStatus, catalog is {ROLE_STATUSES:?}")
        })
}

/// One other administrator the organization is holding: the account id and the
/// `AdminScope` it was onboarded with (`None` sends no `AdminScope` at all).
struct Delegated {
    account: &'static str,
    scope: Option<Value>,
}

/// One row of the table: a name for the assertion messages, the other
/// administrators the organization holds, and whether the test waits for the
/// onboarding the clock owes them before it asks for the tear-down.
struct Case {
    name: &'static str,
    others: Vec<Delegated>,
    onboarded: bool,
}

/// An `AdminScope` covering the whole organization — every "all … enabled"
/// flag of the four scopes `AdminScope` declares.
fn full_scope() -> Value {
    json!({
        "AccountScope": { "AllAccountsEnabled": true },
        "OrganizationalUnitScope": { "AllOrganizationalUnitsEnabled": true },
        "RegionScope": { "AllRegionsEnabled": true },
        "PolicyTypeScope": { "AllPolicyTypesEnabled": true },
    })
}

/// The administrator sets the rule is asserted over.
///
/// The account ids are `AWSAccountId`s (the model constrains the shape only to
/// `^[0-9]+$` with length 1..1024), so they differ in length and digits —
/// including the one-digit and the long forms, which the issue did not cite.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no other administrator",
            others: Vec::new(),
            onboarded: false,
        },
        Case {
            name: "one administrator, still onboarding",
            others: vec![Delegated {
                account: "222222222222",
                scope: Some(json!({ "AccountScope": { "AllAccountsEnabled": true } })),
            }],
            onboarded: false,
        },
        Case {
            name: "one administrator, onboarding complete",
            others: vec![Delegated {
                account: "222222222222",
                scope: Some(full_scope()),
            }],
            onboarded: true,
        },
        Case {
            name: "one administrator with no scope at all",
            others: vec![Delegated {
                account: "7",
                scope: None,
            }],
            onboarded: true,
        },
        Case {
            name: "one administrator scoped to foreign accounts only",
            others: vec![Delegated {
                account: "000000000001",
                scope: Some(json!({
                    "AccountScope": {
                        "Accounts": ["333344445555"],
                        "AllAccountsEnabled": false,
                        "ExcludeSpecifiedAccounts": false,
                    },
                    "RegionScope": { "Regions": ["eu-central-1"] },
                })),
            }],
            onboarded: true,
        },
        Case {
            name: "three administrators",
            others: vec![
                Delegated {
                    account: "222222222222",
                    scope: Some(full_scope()),
                },
                Delegated {
                    account: "99999999999999999999",
                    scope: None,
                },
                Delegated {
                    account: "8",
                    scope: Some(json!({ "PolicyTypeScope": { "AllPolicyTypesEnabled": true } })),
                },
            ],
            onboarded: true,
        },
    ]
}

/// `GetAdminAccount`'s `(http status, parsed body)`.
async fn admin_account(router: &smithy_server_runtime::MultiServiceRouter) -> (u16, Value) {
    call(router, "GetAdminAccount", &json!({})).await
}

/// The account ids `ListAdminAccountsForOrganization` reports, in the order it
/// reports them.
async fn listed_admins(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<String> {
    let (status, body) = call(router, "ListAdminAccountsForOrganization", &json!({})).await;
    assert_eq!(
        status, 200,
        "ListAdminAccountsForOrganization must answer 200, got {status} with {body}"
    );
    body["AdminAccounts"]
        .as_array()
        .unwrap_or_else(|| panic!("AdminAccounts must be a list, got {body}"))
        .iter()
        .filter_map(|summary| summary["AdminAccount"].as_str().map(str::to_string))
        .collect()
}

/// The organization's default administrator may disassociate only while the
/// organization holds no other Firewall Manager administrator; with one or more
/// still there the call is refused with `InvalidOperationException` and changes
/// nothing.
#[tokio::test]
async fn the_default_administrator_disassociates_last() {
    for case in cases() {
        let name = case.name;
        let router = emulator();
        associate_admin(&router).await;

        for other in &case.others {
            let mut request = json!({ "AdminAccount": other.account });
            if let Some(scope) = other.scope.clone() {
                request["AdminScope"] = scope;
            }
            let (status, body) = call(&router, "PutAdminAccount", &request).await;
            assert_eq!(
                status, 200,
                "case {name:?}: onboarding administrator {} must answer 200, got {status} \
                 with {body}",
                other.account
            );
        }
        if case.onboarded && !case.others.is_empty() {
            wait_out_the_onboarding();
        }

        let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;

        if case.others.is_empty() {
            // Nothing has to leave first, so the tear-down is accepted and the
            // role moves to the non-terminal status it passes through.
            assert_eq!(
                status, 200,
                "case {name:?}: with no other administrator the disassociation must be \
                 accepted, got {status} with {body}"
            );
            let (status, got) = admin_account(&router).await;
            assert_eq!(
                (status, got["RoleStatus"].as_str()),
                (200, Some(role_status("PENDING_DELETION"))),
                "case {name:?}: the accepted disassociation must leave the role pending \
                 deletion, got {status} {got}"
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case {name:?}: the default administrator must not disassociate while other \
             administrators are associated, got {status} {body}"
        );
        // The refusal says WHICH administrators still have to disassociate.
        let message = error_message(&body);
        for other in &case.others {
            assert!(
                message.contains(other.account),
                "case {name:?}: the refusal must name administrator {}, got {message:?}",
                other.account
            );
        }

        // It changed nothing: the association is still READY, and every other
        // administrator is still an administrator and still readable.
        let (status, got) = admin_account(&router).await;
        assert_eq!(
            (status, got["RoleStatus"].as_str()),
            (200, Some(role_status("READY"))),
            "case {name:?}: the refused disassociation must have left the association READY, \
             got {status} {got}"
        );
        let expected_status = if case.onboarded {
            organization_status("ONBOARDING_COMPLETE")
        } else {
            organization_status("ONBOARDING")
        };
        for other in &case.others {
            let (status, got) = call(
                &router,
                "GetAdminScope",
                &json!({ "AdminAccount": other.account }),
            )
            .await;
            assert_eq!(
                (status, got["Status"].as_str()),
                (200, Some(expected_status)),
                "case {name:?}: administrator {} must still be readable, got {status} {got}",
                other.account
            );
        }
        let listed = listed_admins(&router).await;
        for other in &case.others {
            assert!(
                listed.iter().any(|account| account == other.account),
                "case {name:?}: administrator {} must still be enumerated, got {listed:?}",
                other.account
            );
        }
    }
}

/// The precondition is read at the call that STARTS the tear-down: an
/// administrator onboarded while the default administrator's disassociation is
/// already pending does not retroactively refuse it, so a repeat of the
/// recorded request is still idempotent and the tear-down still completes.
#[tokio::test]
async fn an_administrator_onboarded_after_the_request_does_not_refuse_the_repeat() {
    let router = emulator();
    associate_admin(&router).await;

    let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
    assert_eq!(
        status, 200,
        "with no other administrator the disassociation must be accepted, got {status} \
         with {body}"
    );

    let (status, body) = call(
        &router,
        "PutAdminAccount",
        &json!({
            "AdminAccount": "222222222222",
            "AdminScope": { "AccountScope": { "AllAccountsEnabled": true } },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "onboarding an administrator must answer 200, got {status} with {body}"
    );

    let (status, body) = call(&router, "DisassociateAdminAccount", &json!({})).await;
    assert_eq!(
        status, 200,
        "repeating the recorded disassociation must stay idempotent, got {status} with {body}"
    );
    let (status, got) = admin_account(&router).await;
    assert_eq!(
        (status, got["RoleStatus"].as_str()),
        (200, Some(role_status("PENDING_DELETION"))),
        "the repeat must not have changed the pending status, got {status} {got}"
    );

    wait_out_the_disassociation();
    let (status, got) = admin_account(&router).await;
    assert_eq!(
        (status, got["RoleStatus"].as_str()),
        (200, Some(role_status("DELETED"))),
        "the pending tear-down must still complete, got {status} {got}"
    );
}
