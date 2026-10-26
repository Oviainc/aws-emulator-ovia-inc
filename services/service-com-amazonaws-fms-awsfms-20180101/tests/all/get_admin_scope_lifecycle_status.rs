// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `GetAdminScope` reports as the administrator
//! lifecycle advances.
//!
//! The rule asserted here: `GetAdminScope` answers, for the account the request
//! names, the administrative scope that account's administrator role decides —
//! the scope STORED for an account `PutAdminAccount` onboarded, the whole
//! organization for the DEFAULT administrator — together with the
//! `OrganizationStatus` that the CLOCK has reached on that account's onboarding
//! or removal; and because it only REPORTS that phase, two reads in a row answer
//! identically and neither moves the lifecycle on.
//!
//! The class the rule is asserted over is the LIFECYCLE PHASE the organization
//! is in, each phase read for every kind of account at once: the default
//! administrator, several onboarded administrators whose ids differ in length
//! and digits, and an account that is no administrator at all. Every phase is
//! reached the way a client reaches it — `AssociateAdminAccount`,
//! `PutAdminAccount`, `DisassociateAdminAccount` and then waiting out the span
//! the service's own constant names — and every read goes through the shared
//! awsJson1_1 harness in [`crate::policy_lifecycle`], so the assertions are
//! about the WIRE response.
//!
//! The sibling [`crate::admin_account_scopes`] suite asserts the SCOPE side of
//! the same operation over a table of scopes; this one asserts the STATUS side
//! over a table of lifecycle phases, through that suite's readers.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_disassociation::wait_out_the_disassociation;
use crate::admin_account_scopes::{
    admin_scope, full_admin_scope, normalize_scope, organization_status, put_admin_account,
    wait_out_the_onboarding,
};
use crate::administrator_gated_operations::associate_admin;
use crate::policy_lifecycle::{call, emulator, ACCOUNT};

/// The accounts onboarded as (non-default) administrators below. The model
/// constrains `AWSAccountId` only to `^[0-9]+$` with length 1..1024, so these
/// differ in length and digits, including the one-digit and the long forms.
const DELEGATED_ACCOUNTS: &[&str] = &["222233334444", "7", "99999999999999999999"];

/// An account that is onboarded as no administrator in any phase below, so the
/// table also says what this operation answers for an account it holds no
/// administrator record for.
const NO_ADMINISTRATOR: &str = "555566667777";

/// The scope each onboarded administrator is granted — a different one per
/// account, so a response that reported another administrator's scope, or a
/// scope derived from the role rather than read from the store, would not match.
fn delegated_scope(account: &str) -> Value {
    json!({ "AccountScope": { "Accounts": [account] } })
}

/// What `GetAdminScope` is expected to answer for one account in one phase.
#[derive(Debug, Clone)]
enum Expectation {
    /// Served: the scope reported, and the `OrganizationStatus` beside it.
    Served { scope: Value, status: &'static str },
    /// Refused with this modelled error code, at HTTP 400.
    Refused(&'static str),
}

/// Assert `GetAdminScope`'s answer for every row of `phase`, reading each
/// account TWICE: the operation reports the phase the clock has reached, so the
/// second read of a phase nothing else touched must answer exactly what the
/// first did.
async fn expect_phase(router: &MultiServiceRouter, phase: &str, rows: &[(&str, Expectation)]) {
    for (account, expectation) in rows {
        let mut answers = Vec::new();
        for read in 1..=2 {
            let (http, code, scope, status) = admin_scope(router, account).await;
            match expectation {
                Expectation::Served {
                    scope: expected_scope,
                    status: expected_status,
                } => {
                    assert_eq!(
                        (http, code.as_str()),
                        (200, ""),
                        "case {phase:?} / {account} read {read}: expected GetAdminScope to be \
                         served with 200, got {http} / {code:?}"
                    );
                    assert_eq!(
                        &scope, expected_scope,
                        "case {phase:?} / {account} read {read}: expected the scope the account's \
                         administrator role decides, got {scope:?}"
                    );
                    assert_eq!(
                        status.as_str(),
                        *expected_status,
                        "case {phase:?} / {account} read {read}: expected the OrganizationStatus \
                         the clock has reached, got {status:?}"
                    );
                }
                Expectation::Refused(expected_code) => {
                    assert_eq!(
                        (http, code.as_str()),
                        (400, *expected_code),
                        "case {phase:?} / {account} read {read}: expected the call to be refused, \
                         got {http} / {code:?}"
                    );
                }
            }
            answers.push((http, code, scope, status));
        }
        let (first, second) = (&answers[0], &answers[1]);
        assert_eq!(
            first, second,
            "case {phase:?} / {account}: a read must report the lifecycle phase without moving \
             it, so two reads in a row must answer identically, got {first:?} then {second:?}"
        );
    }
}

/// `GetAdminScope` reports the `OrganizationStatus` the clock has reached for
/// the account it names, in every phase of the administrator lifecycle: a fresh
/// onboarding in flight, the onboarding the clock completed, a default
/// administrator on its way out, and the organization left with no
/// administrator association at all.
#[tokio::test]
async fn get_admin_scope_reports_the_lifecycle_phase_the_clock_has_reached() {
    let router = emulator();
    associate_admin(&router).await;
    for account in DELEGATED_ACCOUNTS {
        put_admin_account(
            &router,
            "onboard",
            account,
            Some(&delegated_scope(account)),
        )
        .await;
    }

    let full = normalize_scope(&full_admin_scope());
    let stored = |account: &str| normalize_scope(&delegated_scope(account));

    // The onboarding each `PutAdminAccount` started is still in flight — the
    // span is never zero, so a caller reading straight after the call sees the
    // transient status — while the default administrator, whose association is
    // returned ready, is already onboarded.
    let mut rows: Vec<(&str, Expectation)> = vec![
        (
            ACCOUNT,
            Expectation::Served {
                scope: full.clone(),
                status: organization_status("ONBOARDING_COMPLETE"),
            },
        ),
        (
            NO_ADMINISTRATOR,
            Expectation::Refused("ResourceNotFoundException"),
        ),
    ];
    rows.extend(DELEGATED_ACCOUNTS.iter().map(|account| {
        (
            *account,
            Expectation::Served {
                scope: stored(account),
                status: organization_status("ONBOARDING"),
            },
        )
    }));
    expect_phase(&router, "an onboarding in flight", &rows).await;

    // Once the onboarding span has passed every onboarded administrator is
    // onboarded, with the scope it was granted untouched.
    wait_out_the_onboarding();
    for row in rows.iter_mut() {
        if let Expectation::Served { status, .. } = &mut row.1 {
            *status = organization_status("ONBOARDING_COMPLETE");
        }
    }
    expect_phase(&router, "the onboarding the clock completed", &rows).await;

    // The default administrator asks to be disassociated. It comes out LAST —
    // every other Firewall Manager administrator of the organization has to
    // disassociate before it may — so this phase is driven on an organization
    // that holds only the default administrator, and the accounts the earlier
    // phases onboarded are accounts this one holds no administrator record for.
    // The refusal the precondition states is asserted by the
    // `admin_disassociation_first_in_last_out` suite.
    let offboarding = emulator();
    associate_admin(&offboarding).await;
    let (status, body) = call(&offboarding, "DisassociateAdminAccount", &json!({})).await;
    assert_eq!(
        status, 200,
        "the holder's disassociation must be accepted, got {status} with {body:?}"
    );
    let mut rows: Vec<(&str, Expectation)> = vec![
        (
            ACCOUNT,
            Expectation::Served {
                scope: full,
                status: organization_status("OFFBOARDING"),
            },
        ),
        (
            NO_ADMINISTRATOR,
            Expectation::Refused("ResourceNotFoundException"),
        ),
    ];
    rows.extend(
        DELEGATED_ACCOUNTS
            .iter()
            .map(|account| (*account, Expectation::Refused("ResourceNotFoundException"))),
    );
    expect_phase(&offboarding, "a default administrator being removed", &rows).await;

    // Once the tear-down has completed the organization holds no administrator
    // association, so this organisation-scoped call is refused for every
    // account alike — including the one that is no administrator, which the
    // refusal must not distinguish.
    wait_out_the_disassociation();
    for row in rows.iter_mut() {
        row.1 = Expectation::Refused("AccessDeniedException");
    }
    expect_phase(&offboarding, "no administrator association left", &rows).await;
}
