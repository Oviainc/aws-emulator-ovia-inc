// SPDX-License-Identifier: CC0-1.0

//! Which ACCOUNTS one `UpdateEnrollmentStatus` write moves, driven end-to-end
//! over the wire (`awsJson1_0`) through the same multi-service router the
//! emulator boots.
//!
//! The rule under test: the write addresses the calling account alone, widened
//! to every account of the organization where the caller is its management
//! account and the request asks for the organization's member accounts to be
//! enrolled — `smithy_query(query="doc", shape="UpdateEnrollmentStatus")`: "If
//! the account is a management account of an organization, this action can also
//! be used to enroll member accounts of the organization", and
//! `smithy_query(query="doc", shape="UpdateEnrollmentStatusRequest",
//! member="includeMemberAccounts")`: "Indicates whether to enroll member
//! accounts of the organization if the account is the management account or
//! delegated administrator".
//!
//! So the accounts a write reaches are the same set the organization-scoped
//! READERS answer for, and what the write moved is what those readers then
//! report: an account's status, and — because recommendations are reported for
//! an enrolled account only — its catalog rows.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;

use crate::cost_optimization_hub::{
    catalog_rows_for, ids, listed, management_account_router_with_store, ok, record_enrollment,
    Record, Store, ANALYSIS_WAIT,
};

/// The statuses `UpdateEnrollmentStatus` accepts on the wire, read from the
/// service rather than copied here, so this suite cannot drift from the set the
/// handler admits (`state::ENROLLMENT_STATUS_WIRE_VALUES`).
use cost_optimization_hub::state::ENROLLMENT_STATUS_WIRE_VALUES;

/// Member accounts of the organization the store records, each starting at a
/// DIFFERENT status, so a case that moves them is distinguishable from one that
/// leaves them alone. Twelve digits, which is what the `AccountId` shape's
/// `^[0-9]{12}$` pattern admits, and none of them the caller.
const MEMBERS: [(&str, &str); 3] = [
    ("111122223333", "Inactive"),
    ("444455556666", "Active"),
    ("777788889999", "Failed"),
];

/// Record the organization's member accounts at their starting statuses.
fn record_members(store: &Store) {
    for (account, status) in MEMBERS {
        // An `Active` member whose analysis is already due, so a case that
        // moves it out of `Active` is observable in the catalog too.
        record_enrollment(store, account, status, status == "Active");
    }
}

/// The records the whole organization must read as, sorted the way the
/// organization view pages them.
fn organization(caller_status: &str, member_status: Option<&str>) -> Vec<Record> {
    let caller = engine::emulator_account_id();
    let mut all: Vec<Record> = vec![(caller.to_string(), caller_status.to_string())];
    for (account, started_at) in MEMBERS {
        all.push((
            account.to_string(),
            member_status.unwrap_or(started_at).to_string(),
        ));
    }
    all.sort();
    all
}

/// A management account's `UpdateEnrollmentStatus` moves every account of the
/// organization exactly when the request asks for its member accounts.
///
/// The table spans what the rule generalises over: every status the service
/// accepts on the wire, crossed with each form the member-account flag takes
/// (asked for, declined, and the request that omits it altogether). The members
/// start at three different statuses, so "moved to the requested status" and
/// "left where it was" are told apart per account.
#[tokio::test]
async fn enrolling_with_member_accounts_moves_every_account_of_the_organization() {
    for status in ENROLLMENT_STATUS_WIRE_VALUES {
        // (the member-account flag as the request carries it, whether the
        // organization's members are addressed by it)
        let flags: [(Option<bool>, bool); 3] =
            [(Some(true), true), (Some(false), false), (None, false)];
        for (flag, addresses_members) in flags {
            let case = format!("status {status}, includeMemberAccounts {flag:?}");
            let (router, store) = management_account_router_with_store();
            record_members(&store);

            let mut request = json!({"status": status});
            if let Some(flag) = flag {
                request["includeMemberAccounts"] = json!(flag);
            }
            let updated = ok(&router, "UpdateEnrollmentStatus", &request).await;
            assert_eq!(
                updated["status"].as_str(),
                Some(*status),
                "case {case}: expected the answered status {status}, got {updated}"
            );

            let expected = organization(status, addresses_members.then_some(status));
            let payload = ok(
                &router,
                "ListEnrollmentStatuses",
                &json!({"includeOrganizationInfo": true}),
            )
            .await;
            let answered = listed(&payload);
            assert_eq!(
                answered, expected,
                "case {case}: expected {expected:?}, got {answered:?} in {payload}"
            );
            assert_eq!(
                payload["includeMemberAccounts"].as_bool(),
                Some(flag.unwrap_or(false)),
                "case {case}: expected the caller's stored member-account flag, got {payload}"
            );
        }
    }
}

/// An account id the store knows nothing about — twelve digits, as the
/// `AccountId` shape's `^[0-9]{12}$` pattern admits, and none of [`MEMBERS`] nor
/// the caller — so a write that invented records would be observable as a record
/// held for it.
const UNKNOWN_ACCOUNT: &str = "999988887777";

/// The organization's write reaches exactly the accounts the store records,
/// however many they are, and creates a record for no other account.
///
/// The table spans the COUNT of member accounts the organization holds: none,
/// where the write addresses the caller alone, through all three.
#[tokio::test]
async fn the_write_reaches_every_account_the_organization_holds_and_no_other() {
    for members in 0..=MEMBERS.len() {
        let case = format!("{members} member accounts recorded");
        let (router, store) = management_account_router_with_store();
        for (account, status) in &MEMBERS[..members] {
            record_enrollment(&store, account, status, *status == "Active");
        }

        ok(
            &router,
            "UpdateEnrollmentStatus",
            &json!({"status": "Active", "includeMemberAccounts": true}),
        )
        .await;

        let mut expected: Vec<Record> = vec![(
            engine::emulator_account_id().to_string(),
            "Active".to_string(),
        )];
        for (account, _) in &MEMBERS[..members] {
            expected.push((account.to_string(), "Active".to_string()));
        }
        expected.sort();
        let payload = ok(
            &router,
            "ListEnrollmentStatuses",
            &json!({"includeOrganizationInfo": true}),
        )
        .await;
        let answered = listed(&payload);
        assert_eq!(
            answered, expected,
            "case {case}: expected {expected:?}, got {answered:?} in {payload}"
        );

        let unknown = ok(
            &router,
            "ListEnrollmentStatuses",
            &json!({"accountId": UNKNOWN_ACCOUNT}),
        )
        .await;
        let answered = listed(&unknown);
        assert!(
            answered.is_empty(),
            "case {case}: expected no record for an account the store never knew, got \
             {answered:?} in {unknown}"
        );
    }
}

/// A member account the organization's write MOVES keeps the instant its record
/// was created, and carries the instant of that write as its last update: the
/// write updates the organization's existing records rather than replacing them.
#[tokio::test]
async fn a_member_the_organization_moves_keeps_its_creation_instant() {
    for status in ENROLLMENT_STATUS_WIRE_VALUES {
        let case = format!("status {status}");
        let (router, store) = management_account_router_with_store();
        let (member, started_at) = MEMBERS[0];
        record_enrollment(&store, member, started_at, false);
        let (created_before, updated_before) = timestamps(&router, member, &case).await;

        // The write has to land at a later instant than the one the record was
        // created at for "the last update moved" to be observable at all.
        std::thread::sleep(std::time::Duration::from_millis(20));
        ok(
            &router,
            "UpdateEnrollmentStatus",
            &json!({"status": status, "includeMemberAccounts": true}),
        )
        .await;

        let (created_after, updated_after) = timestamps(&router, member, &case).await;
        assert_eq!(
            created_before, created_after,
            "case {case}: expected the creation instant {created_before} kept, got {created_after}"
        );
        assert!(
            updated_after > updated_before,
            "case {case}: expected the last update to move past {updated_before}, got \
             {updated_after}"
        );
        assert!(
            updated_after >= created_after,
            "case {case}: expected the last update at or after the creation instant \
             {created_after}, got {updated_after}"
        );
    }
}

/// The `(createdTimestamp, lastUpdatedTimestamp)` pair `ListEnrollmentStatuses`
/// reports for one named account, as epoch seconds.
async fn timestamps(
    router: &smithy_server_runtime::MultiServiceRouter,
    account: &str,
    case: &str,
) -> (f64, f64) {
    let payload: Value = ok(
        router,
        "ListEnrollmentStatuses",
        &json!({"accountId": account}),
    )
    .await;
    let item = &payload["items"][0];
    let read = |member: &str| {
        item[member]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: expected {member} on {payload}"))
    };
    (read("createdTimestamp"), read("lastUpdatedTimestamp"))
}

/// The member-account flag addresses the organization for the organization's
/// MANAGEMENT account only: any other caller's write moves that caller's own
/// record and nobody else's, whatever the flag says.
#[tokio::test]
async fn the_member_account_flag_moves_nobody_else_for_a_standalone_account() {
    for status in ENROLLMENT_STATUS_WIRE_VALUES {
        for flag in [true, false] {
            let case = format!("status {status}, includeMemberAccounts {flag}");
            let (router, store) = management_account_router_with_store();
            record_members(&store);
            // The caller is no longer the organization's management account, so
            // it has no organization view to write through.
            store.lock().organization_management_account_id = None;

            ok(
                &router,
                "UpdateEnrollmentStatus",
                &json!({"status": status, "includeMemberAccounts": flag}),
            )
            .await;

            // The organization view is refused to this caller, so the members
            // are read straight out of the store.
            let store = store.lock();
            for (account, started_at) in MEMBERS {
                let stored = &store
                    .enrollments
                    .get(account)
                    .unwrap_or_else(|| panic!("case {case}: {account} has an enrollment record"))
                    .status;
                assert_eq!(
                    stored, started_at,
                    "case {case}: expected {account} left at {started_at}, got {stored}"
                );
            }
            let caller = engine::emulator_account_id();
            let own = &store
                .enrollments
                .get(caller)
                .expect("the caller's enrollment record")
                .status;
            assert_eq!(
                own, status,
                "case {case}: expected the caller moved to {status}, got {own}"
            );
        }
    }
}

/// A member account the organization enrolled is enrolled for every purpose:
/// its catalog rows become readable once the analysis that enrollment starts is
/// due, and opting the organization back out makes them unreadable again.
///
/// This is the round trip the write owes — the state it leaves is read back
/// through the organization-scoped readers, not inspected in the store.
#[tokio::test]
async fn a_member_the_organization_enrolled_has_its_recommendations_reported() {
    let (router, store) = management_account_router_with_store();
    let (member, _) = MEMBERS[0];
    record_enrollment(&store, member, "Inactive", false);
    let member_rows = catalog_rows_for(&store, member, 2);
    assert!(
        !member_rows.is_empty(),
        "case setup: expected the member to own catalog rows, got {member_rows:?}"
    );

    assert!(
        member_rows_read(&router, member).await.is_empty(),
        "case before enrollment: expected an unenrolled member to report no rows"
    );

    ok(
        &router,
        "UpdateEnrollmentStatus",
        &json!({"status": "Active", "includeMemberAccounts": true}),
    )
    .await;
    // The analysis the enrollment starts has not come due yet, so the member is
    // enrolled and still has nothing to report.
    assert!(
        member_rows_read(&router, member).await.is_empty(),
        "case just enrolled: expected the member's analysis not to be due yet"
    );
    assert_member_status(&router, member, "Active", "case just enrolled").await;

    std::thread::sleep(ANALYSIS_WAIT);
    let answered = member_rows_read(&router, member).await;
    assert_eq!(
        answered, member_rows,
        "case analysis due: expected {member_rows:?}, got {answered:?}"
    );

    // Opting the organization out moves the member too, and its rows go away
    // with its enrollment.
    ok(
        &router,
        "UpdateEnrollmentStatus",
        &json!({"status": "Inactive", "includeMemberAccounts": true}),
    )
    .await;
    assert_member_status(&router, member, "Inactive", "case opted back out").await;
    let answered = member_rows_read(&router, member).await;
    assert!(
        answered.is_empty(),
        "case opted back out: expected no rows for a member that left the service, got {answered:?}"
    );
}

/// The recommendation ids `ListRecommendations` reports for one named account.
async fn member_rows_read(
    router: &smithy_server_runtime::MultiServiceRouter,
    member: &str,
) -> Vec<String> {
    ids(&ok(
        router,
        "ListRecommendations",
        &json!({
            "includeAllRecommendations": true,
            "filter": {"accountIds": [member]},
        }),
    )
    .await)
}

/// Assert the status `ListEnrollmentStatuses` reports for one named account.
async fn assert_member_status(
    router: &smithy_server_runtime::MultiServiceRouter,
    account: &str,
    expected: &str,
    case: &str,
) {
    let payload: Value = ok(
        router,
        "ListEnrollmentStatuses",
        &json!({"accountId": account}),
    )
    .await;
    let answered = listed(&payload);
    assert_eq!(
        answered,
        vec![(account.to_string(), expected.to_string())],
        "{case}: expected {account} at {expected}, got {answered:?} in {payload}"
    );
}
