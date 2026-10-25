// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORGANIZATION RULE MODE an enrollment configuration
//! reports.
//!
//! The rule under test: a read of an account's enrollment reports a CONCRETE
//! `organizationRuleMode` for that account — the mode the service holds for it,
//! and the unassociated one (`NoneAllowed`) while it holds none — because the
//! member "Specifies whether the management account can create Automation rules
//! that implement optimization actions for this account", a permission that is
//! in one of the enum's two states for every account. The mode an account
//! reports therefore moves only when an association moves it, and the two
//! readers of one account's enrollment — `GetEnrollmentConfiguration` for the
//! caller and `ListAccounts` for any account — report the same mode for it.
//!
//! Every case drives the service the way a client does — an awsJson1_0 `POST /`
//! through the same [`MultiServiceRouter`](smithy_server_runtime::MultiServiceRouter)
//! the emulator binary mounts — using the harness `automation_rule_lifecycle`
//! owns. A configuration no operation of this API can establish (the two
//! `EnrollmentStatus` values a caller may not send) is seeded through the writer
//! `account_association` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::ops::types::ORGANIZATION_RULE_MODE_VALUES;
use service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::state;

use crate::account_association::seed_enrollment;
use crate::automation_rule_lifecycle::{
    call_ok, string_member, unenrolled_emulator_with_engine,
};

/// A member account id in the modelled `AccountId` form (`^[0-9]{12}$`) that is
/// not the calling account.
const MEMBER_A: &str = "111111111111";

/// How many accounts the enrollment store holds a configuration for, read where
/// the handlers write it: a read path may not grow it.
fn stored_account_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    let service_state =
        state::service_state(&**engine).expect("the engine bears this service's state");
    // The guard is bound to a local before the block ends: a tail expression's
    // temporary outlives the local the lock was taken from.
    let count = service_state.lock().enrollments.len();
    count
}

/// The rule mode `GetEnrollmentConfiguration` reports for the calling account.
async fn reported_rule_mode(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
) -> String {
    let body = call_ok(router, "GetEnrollmentConfiguration", &json!({}), case).await;
    assert!(
        body.get("organizationRuleMode").is_some(),
        "case {case:?}: the read must report a concrete organization rule mode, got {body}"
    );
    let reported = string_member(&body, "organizationRuleMode");
    assert!(
        ORGANIZATION_RULE_MODE_VALUES.contains(&reported.as_str()),
        "case {case:?}: expected one of the modelled modes {ORGANIZATION_RULE_MODE_VALUES:?}, \
         got {reported:?} in {body}"
    );
    reported
}

/// The `organizationRuleMode` `ListAccounts` reports for `account_id`, or `None`
/// when the listing does not report that account at all.
async fn listed_rule_mode(
    router: &smithy_server_runtime::MultiServiceRouter,
    account_id: &str,
    case: &str,
) -> Option<String> {
    let body = call_ok(router, "ListAccounts", &json!({}), case).await;
    body.get("accounts")
        .and_then(Value::as_array)
        .and_then(|accounts| {
            accounts
                .iter()
                .find(|account| string_member(account, "accountId") == account_id)
                .map(|account| string_member(account, "organizationRuleMode"))
        })
}

// ---------------------------------------------------------------------------
// What a stored configuration reports
// ---------------------------------------------------------------------------

/// Every combination of a stored `EnrollmentStatus` and a stored rule mode — the
/// mode absent included — reports a concrete mode: the stored one, and
/// `NoneAllowed` while the store holds none.
#[tokio::test]
async fn every_stored_configuration_reports_a_concrete_rule_mode() {
    // (case, stored status, stored reason, stored rule mode). The statuses are
    // the model's `EnrollmentStatus` value set and the modes its
    // `OrganizationRuleMode` value set plus "the store holds none"
    // (`smithy_query(query="shape",shape="EnrollmentStatus,OrganizationRuleMode")`),
    // so the rows span both enums and the unset case.
    let cases: [(&str, &str, Option<&str>, Option<&str>); 10] = [
        ("active-no-mode", state::ENROLLED_STATUS, None, None),
        (
            "active-any-allowed",
            state::ENROLLED_STATUS,
            None,
            Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        ),
        (
            "active-none-allowed",
            state::ENROLLED_STATUS,
            None,
            Some(state::ORGANIZATION_RULE_MODE_NONE_ALLOWED),
        ),
        ("inactive-no-mode", "Inactive", None, None),
        (
            "inactive-any-allowed",
            "Inactive",
            Some("the account opted out"),
            Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        ),
        ("pending-no-mode", "Pending", Some("opt-in in progress"), None),
        (
            "pending-none-allowed",
            "Pending",
            None,
            Some(state::ORGANIZATION_RULE_MODE_NONE_ALLOWED),
        ),
        ("failed-no-mode", "Failed", Some("the opt-in failed"), None),
        (
            "failed-any-allowed",
            "Failed",
            Some("the opt-in failed"),
            Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        ),
        (
            "failed-none-allowed",
            "Failed",
            None,
            Some(state::ORGANIZATION_RULE_MODE_NONE_ALLOWED),
        ),
    ];

    for (case, status, reason, mode) in cases {
        let (router, engine) = unenrolled_emulator_with_engine();
        let caller = state::caller_account_id();
        seed_enrollment(&engine, &caller, status, reason, mode);

        let expected = mode.unwrap_or(state::ORGANIZATION_RULE_MODE_NONE_ALLOWED);
        let reported = reported_rule_mode(&router, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the rule mode {expected:?} the store holds for the caller, \
             got {reported:?}"
        );

        // The same read again: a report changes nothing, so the mode and the
        // store behind it stand.
        let again = reported_rule_mode(&router, case).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second read must report {expected:?}, got {again:?}"
        );
        assert_eq!(
            stored_account_count(&engine),
            1,
            "case {case:?}: reading an enrollment must not write one, so the store still holds \
             only the seeded account"
        );

        // The other reader of the same fact. `ListAccounts` performs one of this
        // service's actions, so it is open only to an opted-in caller; the rows
        // that are not opted in are read through the one operation that answers
        // them.
        if status == state::ENROLLED_STATUS {
            let listed = listed_rule_mode(&router, &caller, case).await;
            assert_eq!(
                listed.as_deref(),
                Some(expected),
                "case {case:?}: the listing must report the caller the same mode the enrollment \
                 read does ({expected:?}), got {listed:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What moves the mode
// ---------------------------------------------------------------------------

/// Over the wire, from an account the service has never heard of: the mode is
/// reported before anything is stored, an enrollment update leaves it where it
/// is — opting in grants the management account nothing — and an association is
/// what moves it to `AnyAllowed`.
#[tokio::test]
async fn the_rule_mode_moves_only_when_an_association_moves_it() {
    let (router, engine) = unenrolled_emulator_with_engine();
    let caller = state::caller_account_id();

    let fresh = reported_rule_mode(&router, "never-configured").await;
    assert_eq!(
        fresh,
        state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
        "case \"never-configured\": an account no association has touched does not let the \
         management account create rules for it, got {fresh:?}"
    );
    assert_eq!(
        stored_account_count(&engine),
        0,
        "case \"never-configured\": the read must report the mode without storing a \
         configuration for the account"
    );

    // (case, the enrollment status to set, the mode expected afterwards)
    let updates = [
        ("opt-in", "Active", state::ORGANIZATION_RULE_MODE_NONE_ALLOWED),
        (
            "opt-out",
            "Inactive",
            state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
        ),
        (
            "opt-in-again",
            "Active",
            state::ORGANIZATION_RULE_MODE_NONE_ALLOWED,
        ),
    ];
    for (case, status, expected) in updates {
        call_ok(
            &router,
            "UpdateEnrollmentConfiguration",
            &json!({"status": status}),
            case,
        )
        .await;
        let reported = reported_rule_mode(&router, case).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: an enrollment update carries no rule mode, so the account's mode \
             stays {expected:?}, got {reported:?}"
        );
    }

    call_ok(
        &router,
        "AssociateAccounts",
        &json!({"accountIds": [caller.clone(), MEMBER_A]}),
        "associate",
    )
    .await;
    let associated = reported_rule_mode(&router, "associate").await;
    assert_eq!(
        associated,
        state::ORGANIZATION_RULE_MODE_ANY_ALLOWED,
        "case \"associate\": an association permits the management account to create rules for \
         the account, got {associated:?}"
    );
    let listed = listed_rule_mode(&router, &caller, "associate").await;
    assert_eq!(
        listed.as_deref(),
        Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        "case \"associate\": both readers report one account's mode identically, got {listed:?}"
    );
    let listed_member = listed_rule_mode(&router, MEMBER_A, "associate-member").await;
    assert_eq!(
        listed_member.as_deref(),
        Some(state::ORGANIZATION_RULE_MODE_ANY_ALLOWED),
        "case \"associate-member\": the member account named in the same request is associated \
         too, got {listed_member:?}"
    );

    // Opting out "stops all of your automation rules"; it dissociates nothing,
    // so the permission the association recorded stands.
    call_ok(
        &router,
        "UpdateEnrollmentConfiguration",
        &json!({"status": "Inactive"}),
        "opt-out-after-association",
    )
    .await;
    let after_opt_out = reported_rule_mode(&router, "opt-out-after-association").await;
    assert_eq!(
        after_opt_out,
        state::ORGANIZATION_RULE_MODE_ANY_ALLOWED,
        "case \"opt-out-after-association\": opting out does not dissociate the account, got \
         {after_opt_out:?}"
    );
}
